//! NetCDF-4 (also HDF5): a measured variable → a raster.
//!
//! A file is a set of named variables, and showing it means picking the one
//! that is a measurement (CF rules, see [`preferred`]), finding where its
//! samples lie on Earth, and stretching the values into brightness. It is read
//! on demand through the host resource ([`Resource`]): HDF5 is addressed by
//! absolute offsets. The variable feeds the chunk grid driver (`grid.rs`): its
//! grid is row windows across the full width ([`rows_of`]), and what a level
//! gets — a window or a pass — is decided by the level table over that grid.
//! Variable selection and the stretch follow a sample of windows ([`describe`]);
//! the whole plane is never held.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use hdf5_pure::{
    AttrValue, ChunkCacheConfig, DType, Dataset, DatasetAccessProperties, File, FileAccessProperties,
    FormatError, MetadataCacheConfig,
};

use super::super::budget::Peak;
use super::super::cascade::Emit;
use super::super::pyramid::TILE;
use super::grid::{self, Chunked, Grid};
use super::radiometry::{self, percentile_stretch, Mapping, Pixel, Samples, STRETCH_SAMPLES};
use super::{placed, Info, Kind, Tie};

/// The HDF5 signature — every NetCDF-4 starts with it. Classic NetCDF-3
/// (`CDF\x01`) does not belong here: it is a different format and is not
/// supported here.
pub const MAGIC: &[u8] = b"\x89HDF\r\n\x1a\x0a";

/// Signatures of classic NetCDF-3 — the format that is not supported here.
///
/// They are named so that the refusal does not end up a lie: both formats share
/// one file name (`.nc`) and one overlay path, and a generic header refusal
/// lists NetCDF among the opened ones — that is, it would answer "this is not
/// an image" over a file of a format it itself called readable. Recognizing
/// them costs four bytes.
pub const CLASSIC: [&[u8]; 2] = [b"CDF\x01", b"CDF\x02"];

/// How much memory is given to the reader's metadata cache.
///
/// HDF5 headers are read as scattered small pieces — the superblock, object
/// headers, B-tree nodes, heaps — and every such read here costs a host call.
/// The cache removes that cost; the crate calls eight megabytes a reasonable
/// figure for streaming reads. Raw chunks do not go into it — the reader
/// deliberately keeps them out, otherwise the metadata cache would hold the
/// data.
const METADATA_CACHE: usize = 8 * 1024 * 1024;

/// How many row windows the probe sample reads — the TIFF and JPEG 2000
/// stretches take the same number of scattered chunks (`tiff::mapping`,
/// `jp2::Chunks::mapping`).
const SAMPLE_WINDOWS: u32 = 4;

/// The host resource as a byte source for HDF5.
///
/// The reader needs exactly two answers from a file — its length and bytes at
/// an absolute offset — and the resource gives both. That is what all on-demand
/// reading lives on: opening costs the headers, not the file.
///
/// `reached` — the farthest read position reached, the same measure as
/// `Metered` (`adapters::Metered`): pass progress grows from it. An atomic
/// rather than `Rc<Cell>`: `Source` must be `Send + Sync`, even though the
/// instance is single-threaded.
struct Resource {
    id: u64,
    len: u64,
    reached: Arc<AtomicU64>,
}

impl hdf5_pure::Source for Resource {
    fn len(&self) -> u64 {
        self.len
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), FormatError> {
        let got = veldsdk::abi::resource_read(self.id, offset, buf.len() as u64)
            .map_err(|why| FormatError::Source(why.to_string()))?;
        // A short answer is not "read as much as there was", but the end of
        // the file in the middle of a structure: the reader asks for exactly
        // what must lie there.
        if got.len() != buf.len() {
            return Err(FormatError::UnexpectedEof {
                expected: buf.len(),
                available: got.len(),
            });
        }
        buf.copy_from_slice(&got);
        self.reached.fetch_max(offset + buf.len() as u64, Ordering::Relaxed);
        Ok(())
    }
}

/// A file over the resource, read on demand; `reached` — how far reading got.
fn opened(resource_id: u64, len: u64, reached: Arc<AtomicU64>) -> Result<File, String> {
    let properties =
        FileAccessProperties::new().with_metadata_cache(MetadataCacheConfig::new(METADATA_CACHE));
    File::from_source_with_options(Resource { id: resource_id, len, reached }, properties)
        .map_err(|why| format!("NetCDF: {}", why))
}

/// Rows of the coordinate lattice on demand — as a one-row region.
///
/// Tie nodes stand on few rows ([`nodes`]), and there is no point reading the
/// whole plane for them: on the half-kilometre SLSTR grid that is 2400 rows of
/// 3000 samples for latitude and as many for longitude, while the nodes need
/// thirty-eight rows. A row is read as a region along the plane's rows axis
/// ([`Item`] knows its shape and axes), unpacked from its packing and kept here
/// in case of a repeated question: the lattice footing ([`footing`]) and the
/// assembly ask for the same nodes twice, and a footing on a ragged edge asks
/// for the neighbours too. It costs read rows: usually dozens, for a wholly
/// ragged lattice — up to the plane.
struct Rows<'a> {
    item: &'a Item,
    /// The dataset is opened once for all rows: the crate's chunk cache lives
    /// in the dataset, and a row is a slice of the chunks beneath it, so a
    /// dataset opened per row would unpack them again. Empty — it failed to
    /// open, as recorded in the log.
    dataset: Option<Dataset>,
    /// Rows that were read and those that failed (`None`): the lattice footing
    /// asks about the ragged edge again and again, and a failure must not cost
    /// a repeated trip.
    read: RefCell<HashMap<u32, Option<Rc<Vec<f32>>>>>,
    /// Any row failed to read — the binding failed because of reading, not
    /// because of the nodes, and it must be reported in those words.
    failed: Cell<bool>,
}

/// Chunk cache ceiling for the coordinate lattice. The cache holds a row of
/// chunks across the whole plane — what neighbouring node rows share; on the
/// half-kilometre SLSTR grid that is two chunks of 1.6 MB. A lattice with
/// chunks larger than the ceiling still reads correctly, but slowly: every node
/// row unpacks its own chunks again.
const ROW_CACHE_CAP: usize = 64 * 1024 * 1024;

impl<'a> Rows<'a> {
    fn new(file: &File, item: &'a Item) -> Self {
        let mut rows = Self {
            item,
            dataset: None,
            read: RefCell::new(HashMap::new()),
            failed: Cell::new(false),
        };
        match file.dataset(&item.path) {
            Ok(dataset) => rows.dataset = Some(rows.cached(file, dataset)),
            Err(why) => {
                veldsdk::log::warn!(target: "decode", "NetCDF: lattice '{}' failed to open: {}", item.path, why);
                rows.failed.set(true);
            }
        }
        rows
    }

    /// The same dataset with the chunk cache sized for one row of chunks
    /// across the plane; a contiguous dataset asks for no cache and stays as it
    /// is.
    fn cached(&self, file: &File, dataset: Dataset) -> Dataset {
        let Some(chunk) = dataset.chunk_shape().ok().flatten() else { return dataset };
        let element = dataset.element_size().unwrap_or(8);
        let (_, columns_axis) = self.item.axes;
        let chunk_bytes = chunk.iter().product::<u64>().saturating_mul(element);
        let across = chunk
            .get(columns_axis)
            .filter(|&&side| side > 0)
            .map_or(1, |&side| self.item.shape[columns_axis].div_ceil(side));
        let budget = usize::try_from(chunk_bytes.saturating_mul(across)).unwrap_or(usize::MAX).min(ROW_CACHE_CAP);
        let config = ChunkCacheConfig::new()
            .with_max_bytes(budget)
            .with_max_slots(usize::try_from(across).unwrap_or(usize::MAX).max(1));
        file.dataset_with_options(&self.item.path, DatasetAccessProperties::new().with_chunk_cache(config))
            .unwrap_or(dataset)
    }

    /// Row `row` of the plane in full, read or from memory.
    fn row(&self, row: u32) -> Option<Rc<Vec<f32>>> {
        if let Some(known) = self.read.borrow().get(&row) {
            return known.clone();
        }
        let values = self.dataset.as_ref().and_then(|dataset| {
            let (rows_axis, columns_axis) = self.item.axes;
            let mut start = vec![0u64; self.item.shape.len()];
            let mut count = vec![1u64; self.item.shape.len()];
            start[rows_axis] = u64::from(row);
            count[columns_axis] = self.item.shape[columns_axis];
            match dataset.read_f32_region(&start, &count) {
                Ok(values) => Some(Rc::new(unpacked(self.item, values))),
                Err(why) => {
                    // The first failure goes to the log; the rest land in the
                    // cache silently.
                    if !self.failed.replace(true) {
                        veldsdk::log::warn!(target: "decode", "NetCDF: row {} of '{}' failed to read: {}", row, self.item.path, why);
                    }
                    None
                }
            }
        });
        self.read.borrow_mut().insert(row, values.clone());
        values
    }

    /// The sample in row `row` and column `column`; `None` — beyond the edge or
    /// not read.
    fn value(&self, row: u32, column: u32) -> Option<f64> {
        self.row(row)?.get(column as usize).map(|value| f64::from(*value))
    }

    /// Any row failed to read or the dataset failed to open.
    fn failed(&self) -> bool {
        self.failed.get()
    }

    /// How many rows were read — for the log.
    fn read_rows(&self) -> usize {
        self.read.borrow().values().filter(|row| row.is_some()).count()
    }
}

/// A binding refusal in words about reading: the coordinate rows failed to
/// read, and the nodes are not at fault — the snapshot would land as our guess
/// rather than the file's.
fn unread(lat: &Item, lon: &Item) -> String {
    format!("coordinates '{}' and '{}' failed to read", lat.path, lon.path)
}

/// The smallest side of the tie-point lattice. As much as a Sentinel-1 granule
/// carries; it is enough for the short side, too few for the long one — see
/// [`count`].
const TIE_GRID: u32 = 21;

/// Samples between lattice nodes. A measure of the binding error: linear
/// interpolation inside a cell cuts off the track's curvature, and cuts more
/// the longer the cell is. On the fifteen-thousand-row OLCI tie grid, a node
/// every 64 samples leaves a median error of 73 metres against 2386 for a
/// 21-node lattice. Twice as often buys nothing: on the same grid a step of 32
/// hits the ceiling ([`TIE_CAP`]) and buys four metres out of seventy-three.
const NODE_STEP: u32 = 64;

/// The largest side of the lattice. Nodes travel to the consumer as a list and
/// land in memory there, so the lattice is not free: 256 per axis is 65
/// thousand points and two and a half megabytes of description.
const TIE_CAP: u32 = 256;

/// How much can lie in a file in total — a traversal limit, not an expectation.
/// The headers of all variables are traversed, and for a file with thousands of
/// them the traversal would cost more than the display.
const MAX_DATASETS: usize = 512;
const MAX_DEPTH: usize = 8;

/// The layout of the displayed variable — what the probe leaves to production.
///
/// The file is not held: opening it costs the headers, and holding it costs the
/// metadata cache for every probe in memo. Samples are not held either: they
/// are read in windows on demand, like TIFF chunks — and the stretch is
/// therefore computed here as well, from the probe's sample, so that
/// neighbouring tiles do not split at the seams (`Mapping`).
pub struct Layout {
    /// The grid of row windows — what the driver lives on (see [`rows_of`]).
    pub grid: Grid,
    /// The variable's shape in the file and its plane axes in that shape —
    /// (rows, columns): a row window is read as a region along the rows axis,
    /// wherever in the file it is kept, and unit axes are taken as zero (see
    /// [`read_rows`]).
    shape: Vec<u64>,
    axes: (usize, usize),
    /// The path of the displayed variable inside the file.
    path: String,
    /// The sample type as recorded in the file: it selects the read ladder
    /// (see [`read_rows`]).
    dtype: DType,
    /// What marks "no data" in it (`_FillValue`).
    fill: Option<f32>,
    /// What the variable is called in the file in human words — for the log.
    said: String,
    /// The display stretch — one per file.
    mapping: Mapping,
}

impl Layout {
    /// Layout of a `width`×`height` variable with a window of `rows` rows (no
    /// taller than the variable itself) and a `dtype` sample; the stretch is the
    /// identity, with no name or label. A grid is enough here, without a file;
    /// the probe fills in its own ([`probed`]).
    pub fn of(width: u32, height: u32, rows: u32, dtype: DType) -> Self {
        let rows = rows.clamp(1, height.max(1));
        Self {
            grid: Grid { tiled: false, chunk: (width, rows), overviews: Vec::new(), depth: depth_of(&dtype) },
            shape: vec![u64::from(height), u64::from(width)],
            axes: (0, 1),
            path: String::new(),
            dtype,
            fill: None,
            said: String::new(),
            mapping: Mapping::identity(None),
        }
    }

    /// Rows in the window — the height of the grid chunk.
    fn rows(&self) -> u32 {
        self.grid.chunk.1
    }

    /// The row window starting at `top`, `rows` rows, as a file region: along
    /// the rows axis — the window, no longer than the variable; along the
    /// columns axis — the full width; along the other axes — a zero sample.
    fn region(&self, top: u64, rows: u32) -> (Vec<u64>, Vec<u64>) {
        let (rows_axis, columns_axis) = self.axes;
        let mut start = vec![0u64; self.shape.len()];
        let mut count = vec![1u64; self.shape.len()];
        start[rows_axis] = top;
        count[rows_axis] = u64::from(rows).min(self.shape[rows_axis].saturating_sub(top));
        count[columns_axis] = self.shape[columns_axis];
        (start, count)
    }

    /// The path of the displayed variable inside the file.
    pub fn path(&self) -> &str {
        &self.path
    }
}

/// Rows in the window of a variable `height` rows tall whose file chunk along
/// the rows axis is `chunk_rows` (`None` — a contiguous layout).
///
/// The window is cut along the plane's rows axis, wherever the file keeps it:
/// unit axes (time in Sentinel-5P, `[1, scanline, ground_pixel]`) do not affect
/// the window. A variable written as one chunk of the full height (SYNERGY)
/// gets a window of `height` rows: less than the plane cannot be read anyway —
/// the chunk is unpacked whole — and the level table then honestly says the
/// level costs the plane.
///
/// Otherwise the window is a run of the file's chunks along the rows: the
/// largest multiple of their height not exceeding [`TILE`], or the chunk itself
/// if it is taller than a tile. A window is never smaller than a chunk (a
/// half-chunk window costs a whole one), and no larger than a tile is needed —
/// a level tile is satisfied by two neighbours. A contiguous layout has no
/// chunks, and its window is the tile.
fn rows_of(height: u32, chunk_rows: Option<u64>) -> u32 {
    let chunk = match chunk_rows {
        None => return TILE.min(height),
        Some(rows) => u32::try_from(rows).unwrap_or(height).clamp(1, height),
    };
    if chunk >= TILE {
        return chunk;
    }
    (chunk * (TILE / chunk)).min(height)
}

/// Bytes of memory per sample in a fresh chunk — before the RGBA expansion that
/// the driver adds itself (`Grid::chunk_bytes`).
///
/// It is computed over the two read phases of the crate and the larger is
/// taken. **Window assembly** (`read_raw_region`): the window itself, the
/// compressed file piece and the chunk unpacked from it all live at once — for
/// a single-chunk variable both grow with the window, and compressed
/// incompressible data is no smaller than the unpacked one. **Expansion** of
/// the assembled: the raw window and its copy — typed of the same size for the
/// samples the display parses itself (`Samples`), f32 for the rest; byte-sized
/// ones need no copy, the raw window is that. An unrecognized type counts as
/// eight bytes: erring this way means refusing, the other way — crashing.
fn depth_of(dtype: &DType) -> u32 {
    let element = width_of(dtype).unwrap_or(8);
    let assembled = 3 * element;
    let unpacked = match dtype {
        DType::U8 => element,
        DType::U16 | DType::I16 | DType::F32 => 2 * element,
        _ => element + 4,
    };
    assembled.max(unpacked)
}

/// A row window of the variable as typed samples — one ladder for the probe's
/// sample and for the driver's chunks.
///
/// The window is read as a file region ([`Layout::region`]): along the plane's
/// rows axis, however many axes the file keeps before it. Bytes, 16-bit
/// integers and f32 the display parses itself (`Samples`); the rest are
/// expanded into f32 — there is no expansion of their own for them, and no file
/// with them has been seen in the catalogue. That copy is counted in
/// [`depth_of`].
fn read_rows<T>(
    dataset: &Dataset,
    layout: &Layout,
    top: u64,
    rows: u32,
    with: impl FnOnce(&Samples<'_>) -> T,
) -> Result<T, String> {
    let (start, count) = layout.region(top, rows);
    let failed = |e: hdf5_pure::Error| {
        format!("NetCDF: rows {}…{}: {}", top, top + count[layout.axes.0], e)
    };
    Ok(match layout.dtype {
        DType::U8 => with(&Samples::U8(&dataset.read_u8_region(&start, &count).map_err(failed)?)),
        DType::U16 => with(&Samples::U16(&dataset.read_u16_region(&start, &count).map_err(failed)?)),
        DType::I16 => with(&Samples::I16(&dataset.read_i16_region(&start, &count).map_err(failed)?)),
        _ => with(&Samples::F32(&dataset.read_f32_region(&start, &count).map_err(failed)?)),
    })
}

/// Headers, variable selection and its layout — without the plane.
///
/// The file length enters no ceiling here, and that is not an omission: it is
/// read on demand, and gigabytes never reached cost neither memory nor wire.
/// What costs is the sample description of the chosen variable — and one for
/// each rejected before it (see [`sampled`]).
///
/// `wanted` — the variable named by the caller (a path from the candidate
/// list); empty — selection by the [`preferred`] order. The named one is
/// checked with the same sample as the chosen one: empty over this place is a
/// refusal in words, not a transparent frame; a flat one is shown.
pub fn describe(resource_id: u64, len: u64, wanted: &str) -> Result<Info, String> {
    let file = opened(resource_id, len, Arc::default())?;

    let surveyed = survey(&file)?;
    let order = preferred(&surveyed);
    if order.is_empty() {
        return Err(explain(&surveyed));
    }
    if !wanted.is_empty() {
        let Some(chosen) = order.iter().copied().find(|item| item.path == wanted) else {
            return Err(format!("NetCDF: variable '{}' is not among the {} fit ones", wanted, order.len()));
        };
        let (height, width) = chosen.plane.ok_or_else(|| explain(&surveyed))?;
        let (layout, values) = probed(&file, chosen, width, height)?;
        return match spread(&values, chosen.fill) {
            Spread::Empty => Err(format!("NetCDF: '{}' is empty in the sample — it was not measured over this place", wanted)),
            Spread::Flat => {
                // A flat field lands in the middle of the scale — and without
                // a word about it the question would be "why is it grey?".
                veldsdk::log::info!(target: "decode", "NetCDF: the named '{}' is flat in the sample — shown as a flat field", wanted);
                told(&file, &surveyed, chosen, layout, &order, &[])
            }
            Spread::Varying => told(&file, &surveyed, chosen, layout, &order, &[]),
        };
    }

    // An empty variable is not an answer, and only the samples reveal it. A
    // Sentinel-3 granule carries variables measured not over every surface
    // ("over the ocean only"), and over land such a one lies as solid
    // `_FillValue`: shown, it gives a transparent frame without a single word
    // about why there is nothing on the globe. A flat one is the last-resort
    // answer: it can be shown (a flat field lands in the middle of the scale),
    // but any neighbour with variation says more. It is remembered together
    // with its layout: its sample has already been read, and nobody goes for it
    // a second time.
    //
    // The reason for a skip is named, not swallowed: otherwise the answer would
    // become "the file has no two-dimensional variables", although it is
    // established above that it has.
    let mut skipped: Vec<String> = Vec::new();
    let mut flat: Option<(&Item, Layout)> = None;
    for chosen in &order {
        let Some((height, width)) = chosen.plane else { continue };
        let (layout, values) = match probed(&file, chosen, width, height) {
            Ok(read) => read,
            Err(why) => {
                skipped.push(why);
                continue;
            }
        };
        match spread(&values, chosen.fill) {
            Spread::Varying => return told(&file, &surveyed, chosen, layout, &order, &skipped),
            Spread::Empty => skipped.push(format!("'{}' is empty in the sample", chosen.path)),
            Spread::Flat => {
                skipped.push(format!("'{}' is flat in the sample", chosen.path));
                flat.get_or_insert((chosen, layout));
            }
        }
    }
    match flat {
        Some((chosen, layout)) => told(&file, &surveyed, chosen, layout, &order, &skipped),
        None => Err(match skipped.is_empty() {
            true => explain(&surveyed),
            // The skipped ones have different reasons — empty, flat, not read
            // — and the common phrase must cover them all: naming one of them,
            // it would lie about the rest.
            false => format!(
                "NetCDF: none of the {} fit variables suited: {}",
                skipped.len(),
                listed(&skipped)
            ),
        }),
    }
}

/// The variable's layout and a sample of its samples — what decides whether it
/// is the one, and what it goes to production with.
fn probed(file: &File, item: &Item, width: u32, height: u32) -> Result<(Layout, Vec<f32>), String> {
    let failed = |e: hdf5_pure::Error| format!("NetCDF: {}: {}", item.path, e);
    let dataset = file.dataset(&item.path).map_err(failed)?;
    let dtype = dataset.dtype().map_err(failed)?;
    let mut layout = Layout::of(width, height, rows_of(height, item.chunk_rows), dtype);
    layout.shape = item.shape.clone();
    layout.axes = item.axes;
    // The window is admitted before reading, by the same measure as a fresh
    // driver chunk (`Grid::chunk_bytes`): for a variable whose window grows
    // with the plane the sample is the plane, and exceeding the limit would end
    // in a trap, not a refusal.
    Peak::new()
        .with("fresh chunk", layout.grid.chunk_bytes(layout.grid.chunk))
        .admit()
        .map_err(|why| format!("NetCDF: '{}' with a window of {} rows: {}", item.path, layout.rows(), why))?;
    let values = sampled(&dataset, &layout, width, height)?;
    layout.path = item.path.clone();
    layout.fill = item.fill;
    layout.said = item.said.clone();
    layout.mapping = mapping(&item.path, &values, item.fill);
    Ok((layout, values))
}

/// A sample of the variable's samples — from windows scattered over its height.
///
/// Up to [`SAMPLE_WINDOWS`] windows, the first and last at the edges: the edge
/// of the imaging swath is not measured at all, and a variable's fitness is
/// worst seen at the edges. Inside a window the samples are taken with a step
/// coprime with the width ([`sampling_step`]), and the step is shared across
/// all windows at once so the sample does not outgrow [`STRETCH_SAMPLES`].
/// "No data" marks stay in the sample: emptiness and flatness are recognized
/// by them (see [`spread`]).
///
/// It costs windows, not the plane. A variable with no more windows than the
/// sample takes — a window-plane (see [`rows_of`]) or three chunks of five
/// thousand rows at OLCI — is read whole by it, one window at a time: the
/// sample saves window memory, not bytes, and the first tiles will read the
/// same again. A sample is a sample: a variable with data only outside its
/// windows passes as empty, and the next candidate is no less honest than a
/// false display; that is the price of not reading the plane.
fn sampled(dataset: &Dataset, layout: &Layout, width: u32, height: u32) -> Result<Vec<f32>, String> {
    let rows = layout.rows();
    let windows = height.div_ceil(rows).max(1);
    // Windows are spread evenly from the first to the last; for a variable with
    // fewer windows the repeats collapse.
    let mut picks: Vec<u32> = (0..SAMPLE_WINDOWS)
        .map(|at| (u64::from(at) * u64::from(windows - 1) / u64::from(SAMPLE_WINDOWS - 1)) as u32)
        .collect();
    picks.dedup();
    let per_window = (width as usize) * (rows as usize);
    let stride = sampling_step(per_window * picks.len(), width as usize);
    let mut values = Vec::with_capacity(per_window * picks.len() / stride + picks.len());
    for &window in &picks {
        let start = u64::from(window) * u64::from(rows);
        read_rows(dataset, layout, start, rows, |samples| {
            values.extend((0..samples.len()).step_by(stride).map(|at| samples.get(at)));
        })?;
    }
    Ok(values)
}

/// A binding from a separate coordinates file — where the raster does not carry
/// one.
///
/// That is how Sentinel-3 packs: the measurement lies in one `.nc`, and the
/// latitude and longitude in a neighbour, and which file it is the provider
/// says (only it knows the product layout). Here it remains to read what is
/// written: a pair of `latitude` and `longitude` planes of one shape — by
/// units, by the same rule as inside the raster (see [`northing`]).
///
/// The coordinate lattice can be sparse: at OLCI the tie grid is sixteen times
/// narrower than the image across the columns, at SLSTR the `tx` lattice is
/// additionally wider than it on both sides. How the lattice is tied to the
/// raster is asked of the file — see [`seating`]; if it does not say, no
/// binding comes out.
///
/// The refusal is "no binding came out", not "the raster is bad": the snapshot
/// lands as the catalogue described it.
pub fn geolocation(
    resource_id: u64,
    len: u64,
    raster: Option<Frame>,
    width: u32,
    height: u32,
) -> Result<Vec<Tie>, String> {
    if width < 2 || height < 2 {
        return Err(format!("raster {}×{} is smaller than a tie node", width, height));
    }
    let file = opened(resource_id, len, Arc::default())?;
    let items = survey(&file)?;

    // The coordinate plane in the file is not alone: next to the latitude lies
    // the height, and at SLSTR — also the coordinates of neighbouring lattices.
    // The most detailed pair of one shape is taken: the lattice has a couple of
    // dozen nodes, and the denser the grid, the closer they are to the place
    // asked about.
    // Uniqueness is mandatory — the same rule as inside the raster (see
    // [`swath`]): two latitudes of one shape is already a question of "which",
    // and the file has no answer to it.
    let pair = |pick: fn(&Item) -> bool, plane: (u32, u32)| -> Option<&Item> {
        let mut found = items.iter().filter(|item| item.plane == Some(plane) && pick(item));
        let one = found.next()?;
        found.next().is_none().then_some(one)
    };
    let mut planes: Vec<(u32, u32)> = items
        .iter()
        .filter(|item| northing(item))
        .filter_map(|item| item.plane)
        .collect();
    planes.sort_unstable_by_key(|plane| std::cmp::Reverse(u64::from(plane.0) * u64::from(plane.1)));
    planes.dedup();

    let found = planes
        .into_iter()
        .find_map(|plane| Some((plane, pair(northing, plane)?, pair(easting, plane)?)));
    let Some(((geo_h, geo_w), lat, lon)) = found else {
        return Err(format!(
            "the file has no latitude–longitude pair of one shape: {}",
            listed(&items.iter().map(|item| item.path.clone()).collect::<Vec<String>>())
        ));
    };
    if geo_w < 2 || geo_h < 2 {
        return Err(format!("coordinate grid {}×{} is not a grid", geo_w, geo_h));
    }
    // A grid denser than the raster means it is not its own: a per-sample file
    // has one exactly its size, a tie grid — sparser. Stretching over such a
    // one would lay the image away from itself, silently.
    if geo_w > width || geo_h > height {
        return Err(format!(
            "coordinate grid {}×{} is denser than raster {}×{} — these are not its coordinates",
            geo_w, geo_h, width, height
        ));
    }
    // Lattices are read as rows of nodes, not as a plane (see [`Rows`]): there
    // is no ceiling on their size here, only the cost of the rows the lattice
    // asked for.
    let (north, east) = (Rows::new(&file, lat), Rows::new(&file, lon));

    let attrs = globals(&file);
    let seat = seating(raster, frame(&attrs), subsampling(&attrs), (width, height), (geo_w, geo_h))?;
    let at = |row: u32, column: u32| -> Option<(f64, f64)> {
        Some((north.value(row, column)?, east.value(row, column)?))
    };
    let ties = lattice((0, geo_h - 1), (0, geo_w - 1), seat, at);
    if ties.is_empty() {
        return Err(match north.failed() || east.failed() {
            true => unread(lat, lon),
            false => nodes_unfit(geo_w, geo_h),
        });
    }
    veldsdk::log::debug!(target: "decode",
        "NetCDF binding from a neighbour file: grid {}×{} ('{}' and '{}'), step {:.2}×{:.2} pixels from {:+.1}×{:+.1}, {} nodes, {} rows read",
        geo_w, geo_h, lat.path, lon.path,
        seat.step.0, seat.step.1, seat.origin.0, seat.origin.1, ties.len(), north.read_rows() + east.read_rows());
    Ok(ties)
}

/// Whether there is anything to look at in the variable's samples.
#[derive(PartialEq, Debug)]
enum Spread {
    /// Not a single fit sample: the variable was not measured over this place.
    Empty,
    /// Fit ones exist, but all are equal: it can be shown, nothing to tell.
    Flat,
    /// There is a change — that is an image.
    Varying,
}

fn spread(values: &[f32], fill: Option<f32>) -> Spread {
    let mut first = None;
    for value in values.iter().copied().filter(|value| radiometry::is_data(*value, fill)) {
        match first {
            None => first = Some(value),
            Some(seen) if seen != value => return Spread::Varying,
            Some(_) => {}
        }
    }
    match first.is_some() {
        true => Spread::Flat,
        false => Spread::Empty,
    }
}

/// Assemble the description of the chosen variable and say in the log how the
/// choice ended.
fn told(
    file: &File,
    surveyed: &[Item],
    chosen: &Item,
    layout: Layout,
    order: &[&Item],
    skipped: &[String],
) -> Result<Info, String> {
    let (height, width) = chosen.plane.ok_or_else(|| explain(surveyed))?;
    veldsdk::log::debug!(target: "decode",
        "NetCDF: showing '{}' ({}, units '{}') — {}×{}, window of {} rows, {} of {} variables fit{}",
        chosen.path, chosen.said, chosen.units, width, height, layout.rows(), order.len(), surveyed.len(),
        match skipped.is_empty() {
            true => String::new(),
            false => format!("; skipped: {} ({})", skipped.len(), listed(skipped)),
        });

    let (ties, binding_trouble) = ties(file, surveyed, chosen);
    Ok(Info {
        width,
        height,
        kind: Kind::Netcdf(layout),
        ties,
        // NetCDF coordinates are written in degrees and as a lattice: there is
        // never a projection here.
        placement: None,
        frame: frame(&globals(file)),
        binding_trouble,
        // The choice is named to the viewer: which of the file's dozens of
        // variables landed on screen — otherwise it cannot be known, the log
        // line above is not visible to them.
        variable: Some(named(chosen)),
        // And all the candidates, in the same order: the viewer sees what else
        // lies in the file, and any of them may be named (`describe` with
        // `wanted`).
        variables: order.iter().map(|item| named(item)).collect(),
    })
}

/// The variable in the file's words — what is reported about it to the viewer.
fn named(item: &Item) -> super::Variable {
    super::Variable { path: item.path.clone(), said: item.said.clone(), units: item.units.clone() }
}

/// An enumeration for one log line: the first three and "and N more".
/// The full list can be fifty names, and a layer caption has no room for it.
fn listed(names: &[String]) -> String {
    let head = names.iter().take(3).cloned().collect::<Vec<String>>().join(", ");
    match names.len() > 3 {
        true => format!("{} and {} more", head, names.len() - 3),
        false => head,
    }
}

/// The variable's chunks behind the driver's trait: a row window from the file,
/// expanded by the probe's stretch.
struct Chunks<'a> {
    dataset: Dataset,
    layout: &'a Layout,
    width: u32,
    height: u32,
    /// How far the file reading got — and the progress counter this is poured
    /// into after every chunk: the crate reader will not peek into `Rc` (see
    /// [`Resource`]).
    reached: Arc<AtomicU64>,
    bytes: &'a Rc<Cell<u64>>,
}

impl<'a> Chunks<'a> {
    fn open(
        resource_id: u64,
        len: u64,
        bytes: &'a Rc<Cell<u64>>,
        info: &Info,
        layout: &'a Layout,
    ) -> Result<Self, String> {
        let reached = Arc::new(AtomicU64::new(bytes.get()));
        let file = opened(resource_id, len, reached.clone())?;
        let dataset =
            file.dataset(&layout.path).map_err(|e| format!("NetCDF: {}: {}", layout.path, e))?;
        Ok(Self { dataset, layout, width: info.width, height: info.height, reached, bytes })
    }
}

impl Chunked for Chunks<'_> {
    fn chunk(&mut self, _image: usize, index: u32) -> Result<(Vec<u8>, u32, u32), String> {
        let rows = self.layout.rows();
        let top = index.saturating_mul(rows);
        if top >= self.height {
            return Err(format!("NetCDF: window {} does not exist: {} rows", index, self.height));
        }
        // The bottom window is shorter: the variable has as many rows as it has.
        let dh = rows.min(self.height - top);
        let pixels = (self.width as usize) * (dh as usize);
        let mapping = self.layout.mapping;
        let rgba = read_rows(&self.dataset, self.layout, u64::from(top), dh, |samples| {
            mapping.rgba(samples, Pixel::named(1), pixels)
        })?;
        if rgba.len() != pixels * 4 {
            return Err(format!(
                "NetCDF: '{}' in window {} has {} samples instead of {}×{}",
                self.layout.path, index, rgba.len() / 4, self.width, dh
            ));
        }
        self.bytes.set(self.bytes.get().max(self.reached.load(Ordering::Relaxed)));
        Ok((rgba, self.width, dh))
    }
}

/// Point reading of level tiles — by the driver over row windows.
pub fn produce_direct(
    resource_id: u64,
    len: u64,
    bytes: &Rc<Cell<u64>>,
    info: &Info,
    layout: &Layout,
    level: u32,
    wants: &[(u32, u32)],
    emit: Emit,
) -> Result<(), String> {
    let mut chunks = Chunks::open(resource_id, len, bytes, info, layout)?;
    veldsdk::log::debug!(target: "decode",
        "NetCDF by window: '{}' ({}), level {}, window of {} rows", layout.path, layout.said, level, layout.rows());
    grid::direct(&mut chunks, &layout.grid, (info.width, info.height), level, wants, emit)
}

/// A pass over the variable top to bottom — by the driver over row windows.
pub fn produce_pass(
    resource_id: u64,
    len: u64,
    bytes: &Rc<Cell<u64>>,
    info: &Info,
    layout: &Layout,
    emit: Emit,
) -> Result<(), String> {
    let mut chunks = Chunks::open(resource_id, len, bytes, info, layout)?;
    veldsdk::log::debug!(target: "decode",
        "NetCDF pass: '{}' ({}), {}×{}, window of {} rows",
        layout.path, layout.said, info.width, info.height, layout.rows());
    grid::pass(&mut chunks, &layout.grid, (info.width, info.height), emit)
}

/// Unpacked coordinates: `value · scale_factor + add_offset`.
///
/// The displayed variable does not need these coefficients (see [`mapping`]),
/// while the coordinates must have them: the Sentinel-3 latitude is written as
/// integers with a step of a millionth of a degree, and without unpacking these
/// are not degrees but tens of millions — that is, not a binding but its
/// absence.
fn unpacked(item: &Item, values: Vec<f32>) -> Vec<f32> {
    let (scale, offset) = item.packing;
    if scale == 1.0 && offset == 0.0 {
        return values;
    }
    values.into_iter().map(|value| (f64::from(value) * scale + offset) as f32).collect()
}

/// The stretch sampling step over the samples laid out in a row.
///
/// Coprime with the width: the samples lie row by row, and a step sharing a
/// divisor with the width lands every row in the same columns. The imaging
/// swath is dangerous in exactly this — its edge is not measured at all, and a
/// sample of edge columns alone would stretch over "no data", that is, a frame
/// stretched past its own values. The step grows, never shrinks: the sample
/// only gets sparser from it, and sparser is cheaper.
fn sampling_step(len: usize, width: usize) -> usize {
    let mut stride = (len / STRETCH_SAMPLES).max(1);
    // A width of zero or one leaves nothing to separate from: just one column.
    if width < 2 {
        return stride;
    }
    let gcd = |mut a: usize, mut b: usize| {
        while b != 0 {
            (a, b) = (b, a % b);
        }
        a
    };
    // The step is no longer than the plane itself: otherwise the sample would
    // come out empty.
    while gcd(stride, width) != 1 && stride < len {
        stride += 1;
    }
    stride
}

/// The display stretch over the variable sample: the same percentiles as the
/// wide TIFF samples. "No data" does not enter the sample — otherwise the
/// −9999 mark would drag the lower edge and the whole snapshot would come out
/// white.
///
/// The `scale_factor` and `add_offset` coefficients are deliberately not
/// applied: the transform is linear and increasing, and the stretch is computed
/// over percentiles of the same values — it brings nothing to brightness, while
/// "no data" is compared against the raw value, exactly as recorded. The
/// coordinates, on the contrary, need them — see [`unpacked`].
fn mapping(path: &str, values: &[f32], fill: Option<f32>) -> Mapping {
    let mut sample: Vec<f32> =
        values.iter().copied().filter(|value| radiometry::is_data(*value, fill)).collect();
    let stretch = percentile_stretch(&mut sample);

    // The numbers by which the frame came out this way and not another. Without
    // them a "white rectangle" on the globe can only be explained by guesses:
    // neither how many samples turned out "no data" nor what the rest stretched
    // into is visible.
    veldsdk::log::debug!(target: "decode",
        "NetCDF stretch: '{}' — {} fit of {} in the sample, 'no data' {:?}, stretch {:?}",
        path, sample.len(), values.len(), fill, stretch);

    match stretch {
        Some((lo, hi)) => Mapping::stretched(lo, hi, fill),
        // Not a single fit value — there is nothing to stretch, and a limit
        // cannot be invented: any assigned one whitens everything above it. The
        // values are taken as bytes — the same rule as TIFF. The chosen
        // variable cannot end up here: an empty one is filtered out by
        // [`spread`] on the same sample.
        None => Mapping::identity(fill),
    }
}

// ── What to show ─────────────────────────────────────────────

/// The file's variable together with everything that needs to be known about it
/// to decide whether it is the one. Collected once by a traversal — headers are
/// more expensive to re-read than to keep.
struct Item {
    path: String,
    /// The group the variable lies in: its coordinates are looked for there too.
    group: String,
    /// How many groups to the root. Details of the ordering lie deeper than the
    /// variable itself — see [`preferred`].
    depth: usize,
    /// The shape without unit axes: `Some((rows, columns))` only for a plane.
    plane: Option<(u32, u32)>,
    /// The length, if this is a one-dimensional series — by it a grid axis is
    /// recognized.
    line: Option<u32>,
    /// The variable is floating-point.
    real: bool,
    /// Measured in angular degrees — see [`angular`].
    angular: bool,
    /// Fit to be shown — see [`preferred`].
    candidate: bool,
    fill: Option<f32>,
    /// The variable's packing: `scale_factor` and `add_offset` as the file
    /// wrote them. One and zero — the variable is recorded as is.
    ///
    /// The display does not need them (see [`mapping`]), while the coordinates
    /// must have them: latitude is packed as integers with a step of a
    /// millionth of a degree, and without unpacking these are not degrees but
    /// tens of millions.
    packing: (f64, f64),
    /// `long_name` or `standard_name`, if the file named them.
    said: String,
    /// What is written in `units`, as written: the units are shown to the
    /// viewer in the same letters ("K"), while CF comparisons are done in lower
    /// case.
    units: String,
    /// Names from `coordinates`, resolved to file paths.
    coordinates: Vec<String>,
    /// Names from `ancillary_variables`, resolved to file paths.
    ancillary: Vec<String>,
    /// The variable's shape in the file and its plane axes in it (rows,
    /// columns) — by them a row window becomes a region (see
    /// [`Layout::region`]).
    shape: Vec<u64>,
    axes: (usize, usize),
    /// The height of the file chunk along the plane's rows axis; `None` — the
    /// layout is not chunked.
    chunk_rows: Option<u64>,
}

/// Traversal of the file: all variables with their headers.
fn survey(file: &File) -> Result<Vec<Item>, String> {
    let mut items = Vec::new();
    walk(file, "", 0, &mut items)?;
    if items.is_empty() {
        return Err("NetCDF: the file holds no variables at all".to_string());
    }

    // What is excluded is visible only as a whole: a variable falls under the
    // exclusion not by its own header, but by another one referring to it.
    // Coordinates are "where", not "what"; auxiliary ones
    // (`ancillary_variables`) — accuracy, quality, observation time: they
    // describe the measurement, they are not it.
    let mut named: HashSet<&str> = HashSet::new();
    for item in &items {
        named.extend(item.coordinates.iter().map(String::as_str));
        named.extend(item.ancillary.iter().map(String::as_str));
    }
    let named: HashSet<String> = named.into_iter().map(str::to_string).collect();
    for item in &mut items {
        // "Where" and "when" are not "what", even when nothing refers to
        // them: the Sentinel-5P swath holds latitude and longitude as planes
        // next to the measurements, and the Sentinel-3 granule also holds the
        // imaging time per sample. Without this rule a ladder of degrees or a
        // ladder of microseconds would be shown, both across the swath.
        let bearings = northing(item) || easting(item) || timing(item);
        item.candidate = item.plane.is_some() && !bearings && !named.contains(&item.path);
    }
    Ok(items)
}

fn walk(file: &File, path: &str, depth: usize, items: &mut Vec<Item>) -> Result<(), String> {
    if depth > MAX_DEPTH || items.len() >= MAX_DATASETS {
        return Ok(());
    }
    let group = match path.is_empty() {
        true => file.root(),
        false => file.group(path).map_err(|e| format!("NetCDF: {}: {}", path, e))?,
    };
    for name in group.datasets().map_err(|e| format!("NetCDF: {}", e))? {
        if items.len() >= MAX_DATASETS {
            break;
        }
        let full = format!("{}/{}", path, name);
        if let Some(item) = describe_item(file, &full, path, depth) {
            items.push(item);
        }
    }
    for name in group.groups().map_err(|e| format!("NetCDF: {}", e))? {
        walk(file, &format!("{}/{}", path, name), depth + 1, items)?;
    }
    Ok(())
}

fn describe_item(file: &File, full: &str, group: &str, depth: usize) -> Option<Item> {
    let dataset = file.dataset(full).ok()?;
    let shape = dataset.shape().ok()?;
    let dtype = dataset.dtype().ok()?;
    let real = matches!(dtype, DType::F32 | DType::F64);
    let numeric = width_of(&dtype).is_some();
    let attrs = dataset.attrs().unwrap_or_default();

    // A coordinate grid axis is a series, not a plane; a variable is a plane.
    // Unit axes are dropped: the time in a Sentinel-5P granule is written as an
    // axis of length one, and without this every its variable would be
    // three-dimensional.
    let wide: Vec<usize> =
        shape.iter().enumerate().filter(|(_, side)| **side > 1).map(|(at, _)| at).collect();
    let sides: Vec<u64> = wide.iter().map(|&at| shape[at]).collect();
    // The plane axes in the file's shape are the two non-unit ones. A region is
    // read only for a plane (`describe` takes a variable with `plane`), and a
    // plane is exactly two such axes; a coordinate axis and a variable of
    // another rank need no pair and get any.
    let axes = match wide.as_slice() {
        [rows, columns] => (*rows, *columns),
        _ => (0, shape.len().saturating_sub(1)),
    };
    let plane = match sides.as_slice() {
        [rows, columns] if numeric => Some((fit(*rows)?, fit(*columns)?)),
        _ => None,
    };
    let line = match sides.as_slice() {
        [length] if numeric => Some(fit(*length)?),
        _ => None,
    };

    // A variable with `flag_values` is a state code, not a measurement: a solid
    // ladder of "sea", "cloud", "not processed". It can be stretched into
    // brightness, and the result would be a deception.
    let coded = attrs.contains_key("flag_values") || attrs.contains_key("flag_masks");

    let said = match text(&attrs, "long_name") {
        said if !said.is_empty() => said,
        _ => text(&attrs, "standard_name"),
    };
    let coordinates =
        words(text(&attrs, "coordinates")).map(|name| resolve(group, &name)).collect();
    let ancillary = words(text(&attrs, "ancillary_variables"))
        .map(|name| resolve(group, &name))
        .collect();

    // The file chunk is the smallest thing that is read, and it is measured
    // along the plane's rows axis ([`rows_of`]).
    let chunk_rows =
        dataset.chunk_shape().ok().flatten().and_then(|chunk| chunk.get(axes.0).copied());

    Some(Item {
        path: full.to_string(),
        group: group.to_string(),
        depth,
        plane: plane.filter(|_| !coded),
        line,
        real,
        angular: angular(&text(&attrs, "units").to_ascii_lowercase()),
        candidate: false,
        fill: attrs.get("_FillValue").and_then(number),
        packing: (
            attrs.get("scale_factor").and_then(number).map_or(1.0, f64::from),
            attrs.get("add_offset").and_then(number).map_or(0.0, f64::from),
        ),
        said,
        units: text(&attrs, "units"),
        coordinates,
        ancillary,
        shape,
        axes,
        chunk_rows,
    })
}

/// Which of the fit variables to show.
///
/// The order of questions runs from "what kind of file is this" to "what is the
/// main thing in it". Closer to the root: the details of the calculation lie in
/// subgroups (`SUPPORT_DATA`, `DETAILED_RESULTS`), while the product itself is
/// outside. Floating-point before integer: in CF an integer is a counter, a
/// code or an index, and a measurement is almost always fractional. Then by
/// the alphabet, and that is not a choice but determinacy: the file says
/// nothing more about seniority, and showing something different every time is
/// worse than always showing one thing.
///
/// A list, not a single answer: a variable's fitness is visible only by its
/// samples, and those are read (see [`describe`]). A variable empty in this
/// granule is not a measurement of this granule, and the next candidate in
/// order is more honest than it.
fn preferred(items: &[Item]) -> Vec<&Item> {
    // Placeability is asked once per variable, not at every comparison: the
    // answer costs a traversal of all the file's headers, and a sort would ask
    // it as many times as it compares.
    let mut order: Vec<(bool, &Item)> = items
        .iter()
        .filter(|item| item.candidate)
        .map(|item| (placeable(items, item), item))
        .collect();
    order.sort_by(|(left_placed, left), (right_placed, right)| {
        // Place outranks everything else, and that is not a convenience: a
        // variable with nowhere to lie is not an image of this granule,
        // whatever it measured. Asked last, it would win over any of the other
        // arguments and take the display to something not about the Earth at
        // all: in a Sentinel-5P level 1B granule the instrument wavelength
        // table won this way — not angular, fractional, at the same depth as
        // the viewing angles.
        //
        // By selection, not rejection: a file that said nothing about place
        // still has to be shown — it lies by the catalogue contour, as it did.
        right_placed
            .cmp(left_placed)
            .then(left.depth.cmp(&right.depth))
            .then(right.real.cmp(&left.real))
            .then(left.angular.cmp(&right.angular))
            .then(left.path.cmp(&right.path))
    });
    order.into_iter().map(|(_, item)| item).collect()
}

/// Why there is nothing to show — in the same words the decision was made in.
fn explain(items: &[Item]) -> String {
    let planes = items.iter().filter(|item| item.plane.is_some()).count();
    match planes {
        0 => format!(
            "NetCDF: among the {} variables of the file there is not a single \
             two-dimensional one — these are measurements, not an image",
            items.len()
        ),
        _ => format!(
            "NetCDF: all {} two-dimensional variables of the file are \
             coordinates, state codes or accuracies: there is nothing to \
             measure by them",
            planes
        ),
    }
}

// ── Where this lies on Earth ─────────────────────────────────────

/// Tie-point lattice of the displayed variable.
///
/// Two ways, and both are CF. The imaging swath carries latitude and longitude
/// per sample, and the variable names them itself (`coordinates`). A regular
/// grid instead carries two axes — rows of latitudes and longitudes — and they
/// are recognized by units and length: the rows axis is exactly as long as the
/// raster has rows.
///
/// Empty — no binding was found in the file; the snapshot lies by the
/// catalogue contour.
///
/// Second goes the caveat ([`Info::binding_trouble`]) — and it goes exactly
/// where the coordinates were found but no lattice came out of them. Emptiness
/// without it means "the file has none at all", and these are different
/// answers: by the first the layer lies as our guess, by the second — the
/// file's.
fn ties(file: &File, items: &[Item], chosen: &Item) -> (Vec<Tie>, Option<String>) {
    let (height, width) = match chosen.plane {
        Some(plane) => plane,
        None => return (Vec::new(), None),
    };
    if width < 2 || height < 2 {
        return (Vec::new(), None);
    }
    let (rows, columns) = ((0, height - 1), (0, width - 1));

    // The refusal here is spoken aloud, because a silent one is
    // indistinguishable from "the file has no coordinates": for a granule with
    // a complex contour both end with the layer taken down, and their reasons
    // are different.
    // The reason goes both to the log and as a field. One does not replace the
    // other: the consumer carries the field and decides for its own layer
    // whether to show, while the parser always needs it.
    let said = |why: String| {
        veldsdk::log::warn!(target: "decode", "NetCDF: {}", why);
        Some(why)
    };
    let told = |ties: Vec<Tie>| match ties.is_empty() {
        true => (Vec::new(), said(nodes_unfit(width, height))),
        false => (ties, None),
    };
    if let Some((lat, lon)) = swath_pair(items, chosen) {
        // Per-sample coordinates are read as rows of nodes (see [`Rows`]): the
        // imaging swath costs its rows of nodes, not the plane.
        let (north, east) = (Rows::new(file, lat), Rows::new(file, lon));
        let at = |row: u32, column: u32| -> Option<(f64, f64)> {
            Some((north.value(row, column)?, east.value(row, column)?))
        };
        let ties = lattice(rows, columns, Seating::SAME, at);
        if ties.is_empty() && (north.failed() || east.failed()) {
            return (Vec::new(), said(unread(lat, lon)));
        }
        return told(ties);
    }
    if let Some((lat, lon)) = grid(items, chosen, file) {
        // The axes are one-dimensional, and their badness is separable: a bad
        // latitude takes out a whole row of nodes, a bad longitude — a whole
        // column. The footing here fixes only the ends of the axes, and a CF
        // axis with a gap in the middle is a broken file.
        let at = |row: u32, column: u32| -> Option<(f64, f64)> {
            Some((*lat.get(row as usize)?, *lon.get(column as usize)?))
        };
        return told(lattice(rows, columns, Seating::SAME, at));
    }
    // No coordinates were found at all — and that is an answer too, not
    // silence: for a variable not measured over the Earth (an instrument
    // calibration table, a spectral axis) there are none, and "no binding"
    // without the variable name cannot be dissected.
    (
        Vec::new(),
        said(format!(
            "no coordinates for '{}': neither per-sample in its group nor axes of length {} and {}",
            chosen.path, height, width
        )),
    )
}

/// The nodes were found, but no lattice came out of them. One text for both
/// places where this happens — at one's own grid and at the neighbour file:
/// the viewer has one question, and two wordings would read as two different
/// troubles.
fn nodes_unfit(width: u32, height: u32) -> String {
    format!("grid nodes {}×{} do not fit a binding", width, height)
}

/// Instrument frame the file's lattice is recorded in: which of its nodes the
/// file starts from and how many metres are in a cell.
///
/// Sentinel-3 declares this in global attributes: `track_offset` and
/// `start_offset` are the node numbers of the common imaging lattice that fell
/// into the first column and the first row of the file, `resolution` is metres
/// across the track and along it. The raster and its per-sample coordinates
/// stand in one frame, while the tie grid stands in its own, and without these
/// numbers the two do not match (see [`seating`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    /// `track_offset` — the file column holding the nadir, that is the start of
    /// the sample across the track. The across-track coordinate of column `j`
    /// is `(j − track_offset) · across`.
    pub across_at: f64,
    /// `start_offset` — the number of the file's first row in the continuous
    /// orbit count. It is counted **from the start of the orbit, not from the
    /// start of the file**, and therefore enters the coordinate with the
    /// opposite sign: the along-track coordinate of row `i` is
    /// `(start_offset + i) · along`. Looking alike, the two offsets combined by
    /// one rule drift a whole cell apart — on the half-kilometre SLSTR grid that
    /// is a kilometre along the track.
    pub along_at: f64,
    /// Metres in a cell across the track.
    pub across: f64,
    /// Metres in a cell along the track.
    pub along: f64,
}

/// Seating of the coordinate lattice on the raster: how many pixels per node
/// and at which pixel the zero node stands.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Seating {
    step: (f64, f64),
    origin: (f64, f64),
}

impl Seating {
    /// Sample for sample: the node is the pixel.
    const SAME: Seating = Seating { step: (1.0, 1.0), origin: (0.0, 0.0) };
}

/// The file's global attributes — the ones recorded at the root group. With
/// them Sentinel-3 speaks about the lattice everything in it lies on.
fn globals(file: &File) -> HashMap<String, AttrValue> {
    file.root().attrs().unwrap_or_default()
}

/// Instrument frame from global attributes. `None` — the packing says nothing
/// about it, and there is nothing to derive it from.
fn frame(attrs: &HashMap<String, AttrValue>) -> Option<Frame> {
    let (across, along) = resolution(&text(attrs, "resolution"))?;
    Some(Frame {
        across_at: f64::from(number(attrs.get("track_offset")?)?),
        along_at: f64::from(number(attrs.get("start_offset")?)?),
        across,
        along,
    })
}

/// Tie grid subsampling — the second vocabulary the product names the same
/// relation with: how many raster pixels per node across the track and along.
fn subsampling(attrs: &HashMap<String, AttrValue>) -> Option<(f64, f64)> {
    let read = |name: &str| Some(f64::from(number(attrs.get(name)?)?));
    Some((read("ac_subsampling_factor")?, read("al_subsampling_factor")?))
}

/// `resolution` is written as a string: `[ 16000 1000 ]` — across the track and
/// along it.
///
/// The number is taken whole and must parse whole: fractional, written with an
/// exponent, signed — all of these are legitimate number records, while
/// `1.2.3` is not a number at all. The string must not be cut by digits: `1.5`
/// would fall apart into one and five, and both would pass on as plausible
/// resolutions.
fn resolution(said: &str) -> Option<(f64, f64)> {
    let mut numbers = said
        .split(|sign: char| !matches!(sign, '0'..='9' | '.' | '-' | '+' | 'e' | 'E'))
        .filter(|word| !word.is_empty())
        .map(str::parse::<f64>);
    let (across, along) = (numbers.next()?.ok()?, numbers.next()?.ok()?);
    let sane = |side: f64| side.is_finite() && side > 0.0;
    (sane(across) && sane(along)).then_some((across, along))
}

/// Whether the seated lattice covers the raster — to within a cell.
///
/// What is read must also be checked against what is visible: the attribute can
/// belong to not this pair of files. At OLCI, for example, `resolution` is the
/// same in the per-sample and the tie file — it is a product property — and the
/// ratio of resolutions would give a sixteen-times sparser lattice a step of
/// one. Such a lattice would cover a sixteenth of the image, and here that is
/// visible.
///
/// The tolerance is a cell: the lattice edge need not land exactly on the
/// raster edge, but it cannot fall short of it by more than a step.
fn covers(seat: Seating, (width, height): (u32, u32), (geo_w, geo_h): (u32, u32)) -> bool {
    let axis = |step: f64, origin: f64, nodes: u32, side: u32| {
        let last = origin + step * f64::from(nodes - 1);
        step > 0.0 && origin <= step && last >= f64::from(side - 1) - step
    };
    axis(seat.step.0, seat.origin.0, geo_w, width) && axis(seat.step.1, seat.origin.1, geo_h, height)
}

/// How the lattice `geo_w`×`geo_h` sits on the raster `width`×`height`.
///
/// The order of answers runs from read to derived, and it is binding.
///
/// **The instrument frame named by both files** answers first: the step is the
/// ratio of resolutions, the origin is the raster offset minus the lattice
/// offset, taken at the same step. That is how SLSTR speaks of itself, and only
/// so does its tie lattice `tx` sit where it should: it is wider than the image
/// on both sides, and the origin drifts 26 columns to the left (the `in`
/// lattice), then 52 (the half-kilometre `an`), then 574 (the oblique view
/// `fo`).
///
/// **A lattice the size of the raster** sits sample for sample. That is a
/// derivation, not something read, and therefore the second answer: the
/// instrument frame, if it is named, knows better.
///
/// **Subsampling** — last, and it must match the sizes.
/// `ac_subsampling_factor` describes the tie lattice of the *product*, not the
/// file it is recorded in: at OLCI the per-sample `geo_coordinates.nc` carries
/// it too. Asked before the sizes, it would stretch the per-sample lattice
/// sixteen times.
///
/// **Nothing said — refusal.** A seating derived from sizes alone is silent
/// exactly where it errs: the outermost node of the `tx` lattice lands on
/// column 2038 of a raster 1500 wide, and such a derivation would stretch the
/// image across the track by 1.38 times. By the catalogue contour it lies at
/// least roughly right.
fn seating(
    raster: Option<Frame>,
    grid: Option<Frame>,
    subsampled: Option<(f64, f64)>,
    (width, height): (u32, u32),
    (geo_w, geo_h): (u32, u32),
) -> Result<Seating, String> {
    if let (Some(raster), Some(grid)) = (raster, grid) {
        let step = (grid.across / raster.across, grid.along / raster.along);
        let seat = Seating {
            step,
            // The signs differ, and that is not a typo: see [`Frame::across_at`]
            // and [`Frame::along_at`].
            origin: (
                raster.across_at - step.0 * grid.across_at,
                step.1 * grid.along_at - raster.along_at,
            ),
        };
        return covers(seat, (width, height), (geo_w, geo_h)).then_some(seat).ok_or_else(|| {
            format!(
                "lattice {}×{}, seated at step {:.2}×{:.2} from {:+.1}×{:+.1}, does not cover raster {}×{}",
                geo_w, geo_h, seat.step.0, seat.step.1, seat.origin.0, seat.origin.1, width, height
            )
        });
    }
    if (geo_w, geo_h) == (width, height) {
        return Ok(Seating::SAME);
    }
    if let Some((across, along)) = subsampled {
        // It must match the sizes to within a pixel: a mismatch here means the
        // subsampling is not about this pair, and the seating would come out as
        // silent as one derived from sizes.
        let spans = |step: f64, nodes: u32, side: u32| {
            step > 0.0 && (f64::from(nodes - 1) * step - f64::from(side - 1)).abs() < 1.0
        };
        if spans(across, geo_w, width) && spans(along, geo_h, height) {
            return Ok(Seating { step: (across, along), origin: (0.0, 0.0) });
        }
        return Err(format!(
            "subsampling {}×{} does not match lattice {}×{} over raster {}×{}",
            across, along, geo_w, geo_h, width, height
        ));
    }
    Err(format!(
        "lattice {}×{} is sparser than raster {}×{}, and the file says nothing about how it relates to it",
        geo_w, geo_h, width, height
    ))
}

/// The stretch of samples the lattice stands on: the first and the last, inclusive.
type Span = (u32, u32);

/// Nodes per side: one for every [`NODE_STEP`] samples, but not fewer than
/// [`TIE_GRID`], not more than [`TIE_CAP`] and not more than the samples
/// themselves.
///
/// Per side, not one number for both: the granule is stretched along the orbit,
/// and a single constant would mean for it dozens of samples between nodes
/// across the track and hundreds along. The floor is needed by short sides —
/// for a grid of two hundred samples the step would give four nodes, that is
/// corners, not a lattice. The clamp by the samples themselves is for sides
/// shorter than the floor: a repeated node would break the lattice at the
/// consumer, where the axes are assembled from different fractions
/// (`Grid::new`).
fn count(side: u32) -> u32 {
    ((side - 1) / NODE_STEP + 1).clamp(TIE_GRID, TIE_CAP).min(side)
}

/// Nodes along one axis: the sample indices the tie points stand on.
///
/// The stretch is given by its ends inclusive — the lattice does not
/// necessarily stand on the whole side (see [`footing`]). A stretch of one
/// sample gives no nodes: there would be nothing to divide by the number of
/// gaps.
fn nodes(from: u32, to: u32) -> Vec<u32> {
    if to <= from {
        return Vec::new();
    }
    let count = count(to - from + 1);
    (0..count)
        .map(|at| {
            let span = f64::from(to - from);
            from + (f64::from(at) * span / f64::from(count - 1)).round() as u32
        })
        .collect()
}

/// What the lattice stands on: a retreat from the edge that moves its nodes off
/// unfilled samples.
///
/// Holes in the coordinate lattice lie along the edge — outside the imaging
/// swath no coordinates are written at all, and the edge can be ragged. A
/// lattice over the whole side catches such an edge with a node, and a node
/// cannot be knocked out: the consumer needs a full rectangle (`Grid::new`),
/// and a lattice missing one node does not assemble at all. So this is fixed by
/// a retreat — the lattice stays rectangular, just stands on a shorter stretch
/// and is denser for it. It keeps as many lines while the stretch is longer
/// than the node count; on a stretch shorter than that a node lands on every
/// sample, and a line is lost to the retreat.
///
/// The move is chosen in two stages, and the order between them matters.
///
/// **A helping step is taken wherever it comes from** — including from a clean
/// side: the nodes are re-laid over the new stretch as a whole, and shifting
/// the boundary by one sample moves the whole axis off the defect. A solid bad
/// column landing exactly on a node column is fixed only this way — one node on
/// the first and last row of the lattice is bad, and a dirt-driven retreat
/// would abandon rows where it helps nothing.
///
/// **No helping step — the dirtiest side goes.** That is a march through a
/// solid band of badness: while the lattice stands wholly inside it, no step
/// reduces anything, and it can only be crossed one sample at a time. Choosing
/// here by "fewer left" is not allowed — on level ground the choice would fall
/// on the clean side, and the budget would leak into steps that approach
/// nothing. The clean side goes only when there are no dirty ones at all: the
/// badness lies inside the lattice, and only a re-laying can move nodes off it.
///
/// The budget is the **smallest** lattice step, and it also caps the step
/// count: each side is given no more than it, four steps in total no more. The
/// smallest of the two, not one per axis: the granule is stretched along the
/// orbit, the lattice step along its rows is hundreds of samples, and a retreat
/// on that measure would abandon hundreds of kilometres of track. Beyond the
/// lattice edge the coordinates continue in a straight line (`Grid::cell`), so
/// only what the lattice cannot tell apart may be abandoned.
///
/// `None` — there is nowhere to go: the budget is spent, or there is no
/// badness on any side and nothing to reduce it with. A hole in the middle of
/// the lattice is not fixed by a retreat.
///
/// **A retreat is not always found, and that is a choice, not an oversight.**
/// The rule is greedy and looks one step ahead, while the march goes through
/// dirt without looking at the remainder — that is, a march step can run at a
/// loss: more holes after it than before. On the edge hole the rule is written
/// for this is a win: on a real SYNERGY AOD mask with one added bad column, a
/// "fewer left" choice abandons 31 rows and 13 columns of fit data for 44
/// extra steps, while the dirt-driven choice abandons nothing. On a sparse
/// scatter it happens the other way — there are arrangements of nine points
/// where a lossy step costs the whole binding. One measure cannot suit both
/// classes, and the more common one is taken: imaging products suffer from edge
/// holes, scatter — none of those seen.
///
/// A refusal here costs exactly what it would cost with no retreat at all, and
/// searching further means enumerating rectangles: there are `(budget + 1)⁴` of
/// them, and for the OLCI grid that is fourteen million.
fn footing(rows: Span, columns: Span, sound: impl Fn(u32, u32) -> bool) -> Option<(Span, Span)> {
    // Each axis has its own lattice step (see [`count`]), and the budget is the
    // smallest of the two. It is computed from the nodes themselves, not from
    // their count anew: two derivations of one rule would drift at the first
    // edit, and the budget would stop matching the real step.
    let step = |span: Span| match nodes(span.0, span.1).len() {
        more_than_one if more_than_one > 1 => (span.1 - span.0) / (more_than_one as u32 - 1),
        _ => 0,
    };
    let budget = step(rows).min(step(columns));
    // The sides in order: the first row, the last row, the first column, the
    // last column.
    let step = |side: usize, rows: Span, columns: Span| match side {
        0 => ((rows.0 + 1, rows.1), columns),
        1 => ((rows.0, rows.1 - 1), columns),
        2 => (rows, (columns.0 + 1, columns.1)),
        _ => (rows, (columns.0, columns.1 - 1)),
    };
    let holes = |rows: Span, columns: Span| -> usize {
        let (r, c) = (nodes(rows.0, rows.1), nodes(columns.0, columns.1));
        r.iter().map(|row| c.iter().filter(|column| !sound(*row, **column)).count()).sum()
    };

    let (mut rows, mut columns) = (rows, columns);
    let mut spent = [0u32; 4];
    loop {
        let (r, c) = (nodes(rows.0, rows.1), nodes(columns.0, columns.1));
        if r.is_empty() || c.is_empty() {
            return None;
        }
        let here = holes(rows, columns);
        if here == 0 {
            return Some((rows, columns));
        }
        let now = [
            c.iter().filter(|column| !sound(r[0], **column)).count(),
            c.iter().filter(|column| !sound(r[r.len() - 1], **column)).count(),
            r.iter().filter(|row| !sound(**row, c[0])).count(),
            r.iter().filter(|row| !sound(**row, c[c.len() - 1])).count(),
        ];
        let tried: Vec<(usize, usize)> = (0..4)
            .filter(|side| spent[*side] < budget)
            .map(|side| {
                let (rows, columns) = step(side, rows, columns);
                (holes(rows, columns), side)
            })
            .collect();
        let side = tried
            .iter()
            .filter(|(left, _)| *left < here)
            .min_by_key(|(left, side)| (*left, std::cmp::Reverse(now[*side])))
            .or_else(|| {
                tried.iter().min_by_key(|(left, side)| (std::cmp::Reverse(now[*side]), *left))
            })
            .map(|(_, side)| *side)?;
        spent[side] += 1;
        (rows, columns) = step(side, rows, columns);
    }
}

/// A lattice of nodes standing on the stretches [`footing`] found. Empty — no
/// binding came out: a lattice with a hole would lay the snapshot anywhere, and
/// for a granule with a complex contour the layer ends on this with an error
/// (see `globe::module::on_described`).
///
/// A node stands in the middle of the sample (+0.5): latitude is written for
/// the centre of the pixel, not its corner.
///
/// `seat` — how the lattice nodes sit on the raster pixels (see [`seating`]).
fn lattice(
    rows: Span,
    columns: Span,
    seat: Seating,
    at: impl Fn(u32, u32) -> Option<(f64, f64)>,
) -> Vec<Tie> {
    // The same closure is asked twice — by the footing and by the assembly —
    // and it must answer the same argument the same way: the footing checked
    // the nodes' fitness, and it is not asked a second time.
    let sound = |row: u32, column: u32| at(row, column).is_some_and(|(lat, lon)| placed(lat, lon));
    let Some((stand_rows, stand_columns)) = footing(rows, columns, sound) else {
        return Vec::new();
    };
    if (stand_rows, stand_columns) != (rows, columns) {
        veldsdk::log::debug!(target: "decode",
            "NetCDF: the lattice retreated from the edge — rows {}…{} of {}…{}, columns {}…{} of {}…{}",
            stand_rows.0, stand_rows.1, rows.0, rows.1,
            stand_columns.0, stand_columns.1, columns.0, columns.1);
    }
    let (r, c) = (nodes(stand_rows.0, stand_rows.1), nodes(stand_columns.0, stand_columns.1));
    let mut ties = Vec::with_capacity(r.len() * c.len());
    for &row in &r {
        for &column in &c {
            // The footing ensured the nodes' fitness — here only the edge of
            // the read remains: if it did not reach the sample, there is no
            // lattice.
            let Some((lat, lon)) = at(row, column) else { return Vec::new() };
            ties.push(Tie {
                px: f64::from(column) * seat.step.0 + seat.origin.0 + 0.5,
                py: f64::from(row) * seat.step.1 + seat.origin.1 + 0.5,
                lat,
                lon,
            });
        }
    }
    ties
}

/// The pair of per-sample coordinates of a variable — from the headers, without
/// a single sample read.
///
/// What `coordinates` names is the first answer: the file itself said where its
/// samples lie, and there is nothing to argue with. If it did not — the units
/// are asked: a plane in `degrees_north` of the same shape and the same group
/// is the latitude of this measurement. That is how the `.nc` lies inside a
/// `.SAFE`: the packing there is its own, the CF attribute `coordinates` is
/// absent from the measurements altogether, and the latitude and longitude lie
/// nearby and are named by their units.
///
/// Uniqueness is mandatory: two latitudes of one shape is already a question of
/// "which", and the file has no answer to it. An SLSTR granule holds several
/// (`latitude_in`, `latitude_tx`), but of different shapes, and they do not
/// fall under this condition.
fn swath_pair<'a>(items: &'a [Item], chosen: &Item) -> Option<(&'a Item, &'a Item)> {
    let named: Vec<&Item> = chosen
        .coordinates
        .iter()
        .filter_map(|path| items.iter().find(|item| &item.path == path))
        .filter(|item| item.plane == chosen.plane)
        .collect();
    let alone = |pick: fn(&Item) -> bool| -> Option<&'a Item> {
        let mut found = items
            .iter()
            .filter(|item| item.plane == chosen.plane && item.group == chosen.group && pick(item));
        let one = found.next()?;
        found.next().is_none().then_some(one)
    };
    let lat = match named.iter().copied().find(|item| northing(item)) {
        Some(lat) => lat,
        None => alone(northing)?,
    };
    let lon = match named.iter().copied().find(|item| easting(item)) {
        Some(lon) => lon,
        None => alone(easting)?,
    };
    Some((lat, lon))
}

/// Axes of a regular grid: rows of latitudes and longitudes of the same length
/// as the raster sides. Looked for in the variable's group and in the root —
/// that is where CF keeps them.
fn grid_axes<'a>(items: &'a [Item], chosen: &Item) -> Option<(&'a Item, &'a Item)> {
    let (height, width) = chosen.plane?;
    let nearby = |item: &Item| item.group == chosen.group || item.group.is_empty();
    let lat = items.iter().find(|item| nearby(item) && item.line == Some(height) && northing(item))?;
    let lon = items.iter().find(|item| nearby(item) && item.line == Some(width) && easting(item))?;
    Some((lat, lon))
}

/// Whether the variable has earth axes — that is, whether its place is
/// expressed in the file at all.
///
/// The same code the coordinates are later taken with: a second measure of the
/// same thing would drift from the first silently — the variable would be
/// chosen as bindable while having no binding. It answers from the headers,
/// reading not a single sample: the shape and the units are written next to the
/// name.
fn placeable(items: &[Item], chosen: &Item) -> bool {
    swath_pair(items, chosen).is_some() || grid_axes(items, chosen).is_some()
}

/// Axes of a regular grid: rows of latitudes and longitudes of the same length
/// as the raster sides. Looked for in the variable's group and in the root —
/// that is where CF keeps them.
fn grid(items: &[Item], chosen: &Item, file: &File) -> Option<(Vec<f64>, Vec<f64>)> {
    let (lat, lon) = grid_axes(items, chosen)?;
    let read = |item: &Item| {
        let values = file.dataset(&item.path).ok()?.read_f64().ok()?;
        let (scale, offset) = item.packing;
        Some(values.into_iter().map(|value| value * scale + offset).collect::<Vec<f64>>())
    };
    Some((read(lat)?, read(lon)?))
}

/// Imaging time — "when", not "what", and the rule is the same as for latitude
/// and longitude: the time plane lies next to the measurements and stretches
/// into a smooth ladder across the swath.
///
/// Recognized by the first word of the units. CF writes time as "<unit> since
/// <moment>", the Sentinel-3 packings get by with a single unit
/// ("microseconds", and "since when" is written in words in `long_name`) —
/// both are caught. By the first word, not by occurrence: "m s-1" is a speed.
fn timing(item: &Item) -> bool {
    matches!(
        item.units.split_whitespace().next().unwrap_or_default().to_ascii_lowercase().as_str(),
        "s" | "sec" | "secs" | "second" | "seconds"
            | "ms" | "millisecond" | "milliseconds"
            | "us" | "microsecond" | "microseconds"
            | "ns" | "nanosecond" | "nanoseconds"
            | "min" | "minute" | "minutes"
            | "h" | "hr" | "hour" | "hours"
            | "d" | "day" | "days"
            | "year" | "years"
    )
}

/// An angular quantity: wind direction, solar zenith, viewing azimuth.
///
/// Such a thing is not shown as brightness, and it is not about beauty. An
/// angle is closed: 359° and 1° are neighbours, while a percentile stretch
/// pulls them to the ends of the scale, and a flat direction field comes out
/// white with a black wedge at the wrap-around. A viewing angle is not a
/// measurement at all, but imaging geometry: a smooth ladder across the swath.
///
/// Therefore such a variable goes last among the fit ones (see [`preferred`]),
/// but is not discarded: if the file has nothing else, showing it is more
/// honest than staying silent. Latitude and longitude do not get here — they
/// are filtered out earlier and by their own units (`degrees_north`,
/// `degrees_east`).
fn angular(units: &str) -> bool {
    matches!(units.trim(), "degrees" | "degree" | "deg" | "degrees_t" | "radians" | "rad")
}

/// Whether this is latitude. By the units of measurement — that is how CF
/// orders them written; the variable name can be anything (`lat`, `latitude`,
/// `LATITUDE`).
fn northing(item: &Item) -> bool {
    let units = item.units.to_ascii_lowercase();
    units.starts_with("degrees_north") || units.starts_with("degree_north")
}

fn easting(item: &Item) -> bool {
    let units = item.units.to_ascii_lowercase();
    units.starts_with("degrees_east") || units.starts_with("degree_east")
}

// ── Odds and ends ──────────────────────────────────────────────

/// Bytes per sample; `None` — the type is not numeric, there is nothing to
/// show it with.
fn width_of(dtype: &DType) -> Option<u32> {
    Some(match dtype {
        DType::I8 | DType::U8 => 1,
        DType::I16 | DType::U16 => 2,
        DType::I32 | DType::U32 | DType::F32 => 4,
        DType::I64 | DType::U64 | DType::F64 => 8,
        _ => return None,
    })
}

/// Raster side as a number. `None` — it is larger than a raster ever gets;
/// such a variable cannot be shown anyway.
fn fit(side: u64) -> Option<u32> {
    u32::try_from(side).ok()
}

fn text(attrs: &HashMap<String, AttrValue>, name: &str) -> String {
    attrs.get(name).and_then(|value| value.as_str()).unwrap_or_default().to_string()
}

/// A number from an attribute. Both ladders are asked on purpose: `_FillValue`
/// is written with the type of the variable itself, and for an integer variable
/// it is integer (`-9999` for surface temperature), for a fractional one
/// fractional — one is not enough.
fn number(value: &AttrValue) -> Option<f32> {
    let single = |numbers: Vec<f64>| numbers.first().copied();
    value
        .as_f64()
        .or_else(|| value.to_f64s().and_then(single))
        .or_else(|| value.as_i64().map(|number| number as f64))
        .or_else(|| value.to_i64s().and_then(|numbers| numbers.first().map(|n| *n as f64)))
        .map(|number| number as f32)
}

/// Names in a CF attribute go separated by a space, and sometimes by a comma.
fn words(text: String) -> impl Iterator<Item = String> {
    text.split([' ', ',', '\t', '\n'])
        .filter(|word| !word.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>()
        .into_iter()
}

/// A name from an attribute becomes a file path. An absolute one is taken as
/// is, the rest are looked for in the group of the variable itself: that is how
/// CF reads them.
fn resolve(group: &str, name: &str) -> String {
    match name.starts_with('/') {
        true => name.to_string(),
        false => format!("{}/{}", group, name),
    }
}
