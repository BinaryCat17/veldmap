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

pub const MAGIC: &[u8] = b"\x89HDF\r\n\x1a\x0a";

pub const CLASSIC: [&[u8]; 2] = [b"CDF\x01", b"CDF\x02"];

const METADATA_CACHE: usize = 8 * 1024 * 1024;

const SAMPLE_WINDOWS: u32 = 4;

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

fn opened(resource_id: u64, len: u64, reached: Arc<AtomicU64>) -> Result<File, String> {
    let properties =
        FileAccessProperties::new().with_metadata_cache(MetadataCacheConfig::new(METADATA_CACHE));
    File::from_source_with_options(Resource { id: resource_id, len, reached }, properties)
        .map_err(|why| format!("NetCDF: {}", why))
}

struct Rows<'a> {
    item: &'a Item,
    dataset: Option<Dataset>,
    read: RefCell<HashMap<u32, Option<Rc<Vec<f32>>>>>,
    failed: Cell<bool>,
}

/// Chunk-cache ceiling for the coordinate lattice: a row of chunks across the
/// plane, what neighbouring node rows share. A larger chunk still reads
/// correctly but slowly — every node row unpacks it again.
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

    fn value(&self, row: u32, column: u32) -> Option<f64> {
        self.row(row)?.get(column as usize).map(|value| f64::from(*value))
    }

    fn failed(&self) -> bool {
        self.failed.get()
    }

    fn read_rows(&self) -> usize {
        self.read.borrow().values().filter(|row| row.is_some()).count()
    }
}

fn unread(lat: &Item, lon: &Item) -> String {
    format!("coordinates '{}' and '{}' failed to read", lat.path, lon.path)
}

const TIE_GRID: u32 = 21;

const NODE_STEP: u32 = 64;

const TIE_CAP: u32 = 256;

const MAX_DATASETS: usize = 512;
const MAX_DEPTH: usize = 8;

pub struct Layout {
    pub grid: Grid,
    shape: Vec<u64>,
    axes: (usize, usize),
    path: String,
    dtype: DType,
    fill: Option<f32>,
    said: String,
    mapping: Mapping,
}

impl Layout {
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

    fn rows(&self) -> u32 {
        self.grid.chunk.1
    }

    fn region(&self, top: u64, rows: u32) -> (Vec<u64>, Vec<u64>) {
        let (rows_axis, columns_axis) = self.axes;
        let mut start = vec![0u64; self.shape.len()];
        let mut count = vec![1u64; self.shape.len()];
        start[rows_axis] = top;
        count[rows_axis] = u64::from(rows).min(self.shape[rows_axis].saturating_sub(top));
        count[columns_axis] = self.shape[columns_axis];
        (start, count)
    }

    pub fn path(&self) -> &str {
        &self.path
    }
}

/// Rows in the read window of a variable `height` rows tall whose rows-axis
/// chunk is `chunk_rows` (`None` — a contiguous layout). A window is never
/// smaller than a chunk — the chunk is decoded whole, so half a chunk costs a
/// whole one — and never taller than a tile needs: the largest run of whole
/// chunks under `TILE`, or the chunk itself when it is taller than a tile.
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
                veldsdk::log::info!(target: "decode", "NetCDF: the named '{}' is flat in the sample — shown as a flat field", wanted);
                told(&file, &surveyed, chosen, layout, &order, &[])
            }
            Spread::Varying => told(&file, &surveyed, chosen, layout, &order, &[]),
        };
    }

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
            false => format!(
                "NetCDF: none of the {} fit variables suited: {}",
                skipped.len(),
                listed(&skipped)
            ),
        }),
    }
}

fn probed(file: &File, item: &Item, width: u32, height: u32) -> Result<(Layout, Vec<f32>), String> {
    let failed = |e: hdf5_pure::Error| format!("NetCDF: {}: {}", item.path, e);
    let dataset = file.dataset(&item.path).map_err(failed)?;
    let dtype = dataset.dtype().map_err(failed)?;
    let mut layout = Layout::of(width, height, rows_of(height, item.chunk_rows), dtype);
    layout.shape = item.shape.clone();
    layout.axes = item.axes;
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

fn sampled(dataset: &Dataset, layout: &Layout, width: u32, height: u32) -> Result<Vec<f32>, String> {
    let rows = layout.rows();
    let windows = height.div_ceil(rows).max(1);
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
    if geo_w > width || geo_h > height {
        return Err(format!(
            "coordinate grid {}×{} is denser than raster {}×{} — these are not its coordinates",
            geo_w, geo_h, width, height
        ));
    }
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

#[derive(PartialEq, Debug)]
enum Spread {
    Empty,
    Flat,
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
        placement: None,
        frame: frame(&globals(file)),
        binding_trouble,
        variable: Some(named(chosen)),
        variables: order.iter().map(|item| named(item)).collect(),
    })
}

fn named(item: &Item) -> super::Variable {
    super::Variable { path: item.path.clone(), said: item.said.clone(), units: item.units.clone() }
}

fn listed(names: &[String]) -> String {
    let head = names.iter().take(3).cloned().collect::<Vec<String>>().join(", ");
    match names.len() > 3 {
        true => format!("{} and {} more", head, names.len() - 3),
        false => head,
    }
}

struct Chunks<'a> {
    dataset: Dataset,
    layout: &'a Layout,
    width: u32,
    height: u32,
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

fn unpacked(item: &Item, values: Vec<f32>) -> Vec<f32> {
    let (scale, offset) = item.packing;
    if scale == 1.0 && offset == 0.0 {
        return values;
    }
    values.into_iter().map(|value| (f64::from(value) * scale + offset) as f32).collect()
}

fn sampling_step(len: usize, width: usize) -> usize {
    let mut stride = (len / STRETCH_SAMPLES).max(1);
    if width < 2 {
        return stride;
    }
    let gcd = |mut a: usize, mut b: usize| {
        while b != 0 {
            (a, b) = (b, a % b);
        }
        a
    };
    while gcd(stride, width) != 1 && stride < len {
        stride += 1;
    }
    stride
}

fn mapping(path: &str, values: &[f32], fill: Option<f32>) -> Mapping {
    let mut sample: Vec<f32> =
        values.iter().copied().filter(|value| radiometry::is_data(*value, fill)).collect();
    let stretch = percentile_stretch(&mut sample);

    veldsdk::log::debug!(target: "decode",
        "NetCDF stretch: '{}' — {} fit of {} in the sample, 'no data' {:?}, stretch {:?}",
        path, sample.len(), values.len(), fill, stretch);

    match stretch {
        Some((lo, hi)) => Mapping::stretched(lo, hi, fill),
        None => Mapping::identity(fill),
    }
}

struct Item {
    path: String,
    group: String,
    depth: usize,
    plane: Option<(u32, u32)>,
    line: Option<u32>,
    real: bool,
    angular: bool,
    candidate: bool,
    fill: Option<f32>,
    packing: (f64, f64),
    said: String,
    units: String,
    coordinates: Vec<String>,
    ancillary: Vec<String>,
    shape: Vec<u64>,
    axes: (usize, usize),
    chunk_rows: Option<u64>,
}

fn survey(file: &File) -> Result<Vec<Item>, String> {
    let mut items = Vec::new();
    walk(file, "", 0, &mut items)?;
    if items.is_empty() {
        return Err("NetCDF: the file holds no variables at all".to_string());
    }

    let mut named: HashSet<&str> = HashSet::new();
    for item in &items {
        named.extend(item.coordinates.iter().map(String::as_str));
        named.extend(item.ancillary.iter().map(String::as_str));
    }
    let named: HashSet<String> = named.into_iter().map(str::to_string).collect();
    for item in &mut items {
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

    let wide: Vec<usize> =
        shape.iter().enumerate().filter(|(_, side)| **side > 1).map(|(at, _)| at).collect();
    let sides: Vec<u64> = wide.iter().map(|&at| shape[at]).collect();
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

fn preferred(items: &[Item]) -> Vec<&Item> {
    let mut order: Vec<(bool, &Item)> = items
        .iter()
        .filter(|item| item.candidate)
        .map(|item| (placeable(items, item), item))
        .collect();
    order.sort_by(|(left_placed, left), (right_placed, right)| {
        right_placed
            .cmp(left_placed)
            .then(left.depth.cmp(&right.depth))
            .then(right.real.cmp(&left.real))
            .then(left.angular.cmp(&right.angular))
            .then(left.path.cmp(&right.path))
    });
    order.into_iter().map(|(_, item)| item).collect()
}

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

fn ties(file: &File, items: &[Item], chosen: &Item) -> (Vec<Tie>, Option<String>) {
    let (height, width) = match chosen.plane {
        Some(plane) => plane,
        None => return (Vec::new(), None),
    };
    if width < 2 || height < 2 {
        return (Vec::new(), None);
    }
    let (rows, columns) = ((0, height - 1), (0, width - 1));

    let said = |why: String| {
        veldsdk::log::warn!(target: "decode", "NetCDF: {}", why);
        Some(why)
    };
    let told = |ties: Vec<Tie>| match ties.is_empty() {
        true => (Vec::new(), said(nodes_unfit(width, height))),
        false => (ties, None),
    };
    if let Some((lat, lon)) = swath_pair(items, chosen) {
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
        let at = |row: u32, column: u32| -> Option<(f64, f64)> {
            Some((*lat.get(row as usize)?, *lon.get(column as usize)?))
        };
        return told(lattice(rows, columns, Seating::SAME, at));
    }
    (
        Vec::new(),
        said(format!(
            "no coordinates for '{}': neither per-sample in its group nor axes of length {} and {}",
            chosen.path, height, width
        )),
    )
}

fn nodes_unfit(width: u32, height: u32) -> String {
    format!("grid nodes {}×{} do not fit a binding", width, height)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub across_at: f64,
    pub along_at: f64,
    pub across: f64,
    pub along: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Seating {
    step: (f64, f64),
    origin: (f64, f64),
}

impl Seating {
    const SAME: Seating = Seating { step: (1.0, 1.0), origin: (0.0, 0.0) };
}

fn globals(file: &File) -> HashMap<String, AttrValue> {
    file.root().attrs().unwrap_or_default()
}

fn frame(attrs: &HashMap<String, AttrValue>) -> Option<Frame> {
    let (across, along) = resolution(&text(attrs, "resolution"))?;
    Some(Frame {
        across_at: f64::from(number(attrs.get("track_offset")?)?),
        along_at: f64::from(number(attrs.get("start_offset")?)?),
        across,
        along,
    })
}

fn subsampling(attrs: &HashMap<String, AttrValue>) -> Option<(f64, f64)> {
    let read = |name: &str| Some(f64::from(number(attrs.get(name)?)?));
    Some((read("ac_subsampling_factor")?, read("al_subsampling_factor")?))
}

fn resolution(said: &str) -> Option<(f64, f64)> {
    let mut numbers = said
        .split(|sign: char| !matches!(sign, '0'..='9' | '.' | '-' | '+' | 'e' | 'E'))
        .filter(|word| !word.is_empty())
        .map(str::parse::<f64>);
    let (across, along) = (numbers.next()?.ok()?, numbers.next()?.ok()?);
    let sane = |side: f64| side.is_finite() && side > 0.0;
    (sane(across) && sane(along)).then_some((across, along))
}

fn covers(seat: Seating, (width, height): (u32, u32), (geo_w, geo_h): (u32, u32)) -> bool {
    let axis = |step: f64, origin: f64, nodes: u32, side: u32| {
        let last = origin + step * f64::from(nodes - 1);
        step > 0.0 && origin <= step && last >= f64::from(side - 1) - step
    };
    axis(seat.step.0, seat.origin.0, geo_w, width) && axis(seat.step.1, seat.origin.1, geo_h, height)
}

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

type Span = (u32, u32);

fn count(side: u32) -> u32 {
    ((side - 1) / NODE_STEP + 1).clamp(TIE_GRID, TIE_CAP).min(side)
}

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

fn footing(rows: Span, columns: Span, sound: impl Fn(u32, u32) -> bool) -> Option<(Span, Span)> {
    let step = |span: Span| match nodes(span.0, span.1).len() {
        more_than_one if more_than_one > 1 => (span.1 - span.0) / (more_than_one as u32 - 1),
        _ => 0,
    };
    let budget = step(rows).min(step(columns));
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

fn lattice(
    rows: Span,
    columns: Span,
    seat: Seating,
    at: impl Fn(u32, u32) -> Option<(f64, f64)>,
) -> Vec<Tie> {
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

fn grid_axes<'a>(items: &'a [Item], chosen: &Item) -> Option<(&'a Item, &'a Item)> {
    let (height, width) = chosen.plane?;
    let nearby = |item: &Item| item.group == chosen.group || item.group.is_empty();
    let lat = items.iter().find(|item| nearby(item) && item.line == Some(height) && northing(item))?;
    let lon = items.iter().find(|item| nearby(item) && item.line == Some(width) && easting(item))?;
    Some((lat, lon))
}

fn placeable(items: &[Item], chosen: &Item) -> bool {
    swath_pair(items, chosen).is_some() || grid_axes(items, chosen).is_some()
}

fn grid(items: &[Item], chosen: &Item, file: &File) -> Option<(Vec<f64>, Vec<f64>)> {
    let (lat, lon) = grid_axes(items, chosen)?;
    let read = |item: &Item| {
        let values = file.dataset(&item.path).ok()?.read_f64().ok()?;
        let (scale, offset) = item.packing;
        Some(values.into_iter().map(|value| value * scale + offset).collect::<Vec<f64>>())
    };
    Some((read(lat)?, read(lon)?))
}

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

fn angular(units: &str) -> bool {
    matches!(units.trim(), "degrees" | "degree" | "deg" | "degrees_t" | "radians" | "rad")
}

fn northing(item: &Item) -> bool {
    let units = item.units.to_ascii_lowercase();
    units.starts_with("degrees_north") || units.starts_with("degree_north")
}

fn easting(item: &Item) -> bool {
    let units = item.units.to_ascii_lowercase();
    units.starts_with("degrees_east") || units.starts_with("degree_east")
}

fn width_of(dtype: &DType) -> Option<u32> {
    Some(match dtype {
        DType::I8 | DType::U8 => 1,
        DType::I16 | DType::U16 => 2,
        DType::I32 | DType::U32 | DType::F32 => 4,
        DType::I64 | DType::U64 | DType::F64 => 8,
        _ => return None,
    })
}

fn fit(side: u64) -> Option<u32> {
    u32::try_from(side).ok()
}

fn text(attrs: &HashMap<String, AttrValue>, name: &str) -> String {
    attrs.get(name).and_then(|value| value.as_str()).unwrap_or_default().to_string()
}

fn number(value: &AttrValue) -> Option<f32> {
    let single = |numbers: Vec<f64>| numbers.first().copied();
    value
        .as_f64()
        .or_else(|| value.to_f64s().and_then(single))
        .or_else(|| value.as_i64().map(|number| number as f64))
        .or_else(|| value.to_i64s().and_then(|numbers| numbers.first().map(|n| *n as f64)))
        .map(|number| number as f32)
}

fn words(text: String) -> impl Iterator<Item = String> {
    text.split([' ', ',', '\t', '\n'])
        .filter(|word| !word.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>()
        .into_iter()
}

fn resolve(group: &str, name: &str) -> String {
    match name.starts_with('/') {
        true => name.to_string(),
        false => format!("{}/{}", group, name),
    }
}
