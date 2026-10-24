//! Product rasters for the overlay: which files of which roles lie inside.
//!
//! Only this module knows the .SAFE layout — as it knows the bucket layout. There
//! are two roles, by purpose: the preview is a small file that gives the overlay a
//! picture at once; the detailed one is what zooming goes to.
//!
//! Resolution goes from exact to approximate. First the name templates of known
//! missions: there the layout is named and there is nothing to guess. If none is
//! recognized, the file names themselves decide, and that is already a guess, but an
//! honest one: unknown layouts cover Landsat and Sentinel-3 granules, and showing one
//! band is far more useful than staying silent about a product that holds two dozen.

/// A raster role. Its own, not from types.proto: a pure function parses the .SAFE
/// layout and has no need to know the protocol — a match at the boundary translates
/// the role into the contract (see `cdse::imagery_response`).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Role {
    Preview,
    Detailed,
}

/// Product subtree keys → rasters with roles. Keys are full identifiers (with the
/// bucket prefix), as the listing returns them.
///
/// `product` — the identifier of the product itself, the same one the subtree was
/// listed with. It is asked for one reason: the product names the quantity it
/// measures, and by it the measurement is recognized among the bands (see
/// [`names_the_measurand`]).
///
/// `measured` — files that the product manifest named as the measurement (paths from
/// the product root; see `manifest::measurements`). Empty — there was no manifest or
/// it said nothing, and the detailed raster is chosen by file names instead.
pub fn scan(product: &str, keys: &[String], measured: &[String], downloaded: bool) -> Scan {
    let mut rasters = Vec::new();

    // Sentinel-2: the granule quicklook lies in QI_DATA, true colour — in
    // IMG_DATA (L2A — by resolutions, R10m the most detailed; L1C — flat).
    if let Some(pvi) = keys.iter().find(|key| key.ends_with("_PVI.jp2")) {
        rasters.push((pvi.clone(), Role::Preview));
    }
    let tci = keys
        .iter()
        .find(|key| key.contains("/IMG_DATA/R10m/") && key.ends_with("_TCI_10m.jp2"))
        .or_else(|| {
            keys.iter().find(|key| key.contains("/IMG_DATA/") && key.ends_with("_TCI.jp2"))
        });
    if let Some(tci) = tci {
        rasters.push((tci.clone(), Role::Detailed));
    }

    // Sentinel-1: quicklook in preview/, detailed — measurement. Co-polarisation
    // (vv/hh) is preferred to cross-: its amplitude reads both land and sea.
    //
    // Sentinel-1: quicklook in preview/, detailed — measurement. Co-polarisation
    // (vv/hh) is preferred to cross-: its amplitude reads both land and sea.
    //
    // The tiled GeoTIFF with copies (-COG) is always offered: it has its own copy
    // for every level, and any of them is worth its tiles.
    //
    // The striped giant of old GRD — only when the file is at hand, and its coarse
    // edge decides this, not the detailed one. A detailed tile is cheap for it: 512
    // rows across the full width, tens of megabytes, and the tiler reads such a level
    // by a window (a pointwise row of the level table). But a coarse level needs EVERY
    // row of the file —
    // a strip cannot be skipped, the needed row is in each one — and it costs a whole
    // pass. What is asked is precisely the coarse one: the view canvas opens the
    // detailed raster and fits it into the window. Over the network this is a quarter
    // of an hour, from disk — seconds.
    //
    // Only the requester knows this: what lies on disk is kept by the library, while
    // the provider sees a single storage.
    if rasters.is_empty() {
        if let Some(quicklook) = keys.iter().find(|key| key.ends_with("/preview/quick-look.png"))
        {
            rasters.push((quicklook.clone(), Role::Preview));
        }
        let raster = |key: &str| {
            key.contains("/measurement/") && (key.ends_with(".tiff") || key.ends_with(".tif"))
        };
        let affordable = |key: &str| raster(key) && (downloaded || key.contains("_COG.SAFE/"));
        let measurement = keys
            .iter()
            .find(|key| affordable(key) && (key.contains("-vv-") || key.contains("-hh-")))
            .or_else(|| keys.iter().find(|key| affordable(key)));
        if let Some(measurement) = measurement {
            rasters.push((measurement.clone(), Role::Detailed));
        }
    }

    // No layout was recognized. Then the only thing known about the files before
    // opening them decides — their names.
    //
    // Refusing would be simpler but wrong: this rule covers not exotics but Landsat
    // with all its optics up to Sentinel and Sentinel-3 granules, where bands count
    // two dozen. Showing one band is an honest answer ("here is what the product
    // has"); staying silent is not.
    if rasters.is_empty() {
        let readable: Vec<&String> = keys.iter().filter(|key| is_raster(key)).collect();
        if let Some(quicklook) = readable.iter().copied().find(|key| a_quicklook(key)) {
            rasters.push((quicklook.clone(), Role::Preview));
        }
        // What the manifest names narrows the choice to the measurement — where a
        // manifest exists and speaks about a readable file. It does not narrow to
        // nothing: the measurement may be something that cannot be opened as a raster
        // (level 0 raw is written as `.dat`), and then the question stands unchanged.
        let named: Vec<&String> =
            readable.iter().copied().filter(|key| measured_by(key, measured)).collect();
        let among = match named.is_empty() {
            true => readable,
            false => named,
        };
        // Choice order: first what looks like a whole colour picture; then a
        // measurement format over a display one; then what the product's own
        // quantity names; then everything except those that declared themselves
        // auxiliary; then the densest recording grid, if its name states it
        // ([`grid_rank`]); and if none of the five told them apart — alphabetically:
        // that is no longer a choice but certainty, the same answer for one product
        // from run to run.
        let mut ranked: Vec<&String> =
            among.into_iter().filter(|key| !a_quicklook(key) && !a_decoration(key)).collect();
        ranked.sort_by_key(|key| {
            (
                !a_whole_picture(key),
                a_picture_format(key),
                !names_the_measurand(key, product),
                an_aside(key),
                grid_rank(key),
                file_name(key),
            )
        });
        let mut alternates = Vec::new();
        if let Some((detailed, rest)) = ranked.split_first() {
            rasters.push(((*detailed).clone(), Role::Detailed));
            alternates = spares(detailed, rest);
        }
        // Only unrecognized layouts get here, and the detailed raster here is chosen
        // by file names — that is, by a guess. This is what sets apart the case that
        // is worth going to the manifest for.
        return Scan { rasters, alternates, guessed: true };
    }

    Scan { rasters, alternates: Vec::new(), guessed: false }
}

/// Spare detailed rasters behind the chosen one: the best file of each grid coarser
/// than it, in rank order ([`grid_rank`]), without the reference grid and without
/// files whose name is silent about the grid — an instrument calibration table does
/// not qualify as a spare raster.
///
/// The choice by name does not see what is inside a file: an SLSTR granule shot at
/// night has its visible channels written as solid "no data", and the half-kilometre
/// `S1_radiance_an.nc` is empty. The tiler rejects such as empty, and the layer would
/// be left without a detailed raster and without its georeference, though a kilometre
/// thermal channel with data lies next to it. Therefore along with the chosen one
/// comes what to try next: one per grid, because the neighbours on a grid are empty
/// for the same reason. Without grid tails (OLCI, Landsat) there are no spares — the
/// names give no second answer.
fn spares<'a>(detailed: &String, rest: &[&'a String]) -> Vec<String> {
    let chosen = grid_rank(detailed).0;
    let mut seen = Vec::new();
    rest.iter()
        .filter(|key| {
            let grid = grid_rank(key).0;
            let fresh = grid_tag(key).is_some() && grid > chosen && grid < 2 && !seen.contains(&grid);
            if fresh {
                seen.push(grid);
            }
            fresh
        })
        .map(|key| (*key).clone())
        .collect()
}

/// The file named by the viewer becomes the detailed raster, and the layout's choice
/// becomes the spare behind it: the viewer knows better which channel it needs, but a
/// file that will not open would leave the layer without a detailed raster at all. A
/// named file that is not from the product, not a raster by name (manifest), or a
/// quicklook — a refusal with words (the second answer) and the layout's rasters: such
/// a choice cannot be shown by anything, and silently replacing it would mean lying in
/// the layer's status line. Whether the file is really a raster is not visible from the
/// name (`geodetic_*.nc` is also `.nc`): the tiler says so when describing it. The
/// preview stays as it is.
pub fn preferring(scan: Scan, wanted: &str, keys: &[String]) -> (Scan, Option<String>) {
    if wanted.is_empty() {
        return (scan, None);
    }
    let refused = if !keys.iter().any(|key| key == wanted) {
        Some("it is not in the product")
    } else if !is_raster(wanted) {
        Some("by its name it is not a raster")
    } else if scan.rasters.iter().any(|(key, role)| key == wanted && *role == Role::Preview) {
        Some("this is a quicklook, it is already the preview")
    } else {
        None
    };
    if let Some(why) = refused {
        let said = format!("file '{}' did not take the detailed role: {} — the layout choice stays", file_name(wanted), why);
        log::warn!(target: "handlers", "{}", said);
        return (scan, Some(said));
    }
    let mut rasters: Vec<(String, Role)> =
        scan.rasters.iter().filter(|(_, role)| *role == Role::Preview).cloned().collect();
    rasters.push((wanted.to_string(), Role::Detailed));
    let alternates = scan
        .rasters
        .iter()
        .filter(|(key, role)| *role == Role::Detailed && key != wanted)
        .map(|(key, _)| key.clone())
        .chain(scan.alternates.into_iter().filter(|key| key != wanted))
        .collect();
    (Scan { rasters, alternates, guessed: scan.guessed }, None)
}

/// What came out of [`scan`]: rasters with roles, spares behind the detailed one, and
/// what the detailed one was chosen by.
pub struct Scan {
    pub rasters: Vec<(String, Role)>,
    /// Detailed rasters for the case the chosen one will not open or is empty, in
    /// preference order (see [`spares`]).
    pub alternates: Vec<String>,
    /// The detailed raster is chosen by a guess from file names, not by a known
    /// mission's layout. Only such a choice needs the manifest: for Sentinel-1 and
    /// Sentinel-2 the layout is named, and an extra signed request for hundreds of
    /// kilobytes adds nothing.
    pub guessed: bool,
}

/// Last segment of the key.
fn file_name(key: &str) -> &str {
    key.trim_end_matches('/').rsplit('/').next().unwrap_or("")
}

/// A name that usually hides a small overview picture, not a measurement.
///
/// By a piece of the name, because it is written differently: hyphenated, joined, and
/// with a mission prefix. Exception — `BP`: two letters as a piece catch anything,
/// therefore Browse Product is recognized as a separate word.
///
/// The ESA archive calls its picture so: for Landsat-5 next to the `.TIFF` directory
/// holding the bands stands `LS05_…_52FE.BP.PNG`. Not named as overview, it becomes
/// the detailed raster — alphabetically `LS05…` comes before `LT51…_B1.TIF` — and on
/// the globe a display picture lands instead of the acquisition.
fn a_quicklook(key: &str) -> bool {
    let name = file_name(key).to_ascii_lowercase();
    let by_piece = ["quick-look", "quicklook", "thumb", "browse", "preview", "_pvi"]
        .iter()
        .any(|hint| name.contains(hint));
    by_piece || words(&name).any(|word| word == "bp")
}

/// A name that says by itself the file stands by a measurement rather than is one.
///
/// By a word, not a piece: "coordinates" inside someone else's name means nothing.
///
/// The words here are the same as those naming coordinate files for [`geolocation`],
/// plus "ancillary". The difference is the question: there the exact file that will
/// seat under the raster is sought, here — every such one is recognized, so that it
/// does not seat on the globe instead of a raster.
///
/// This holds on what lies in the tree. For an SLSTR granule `LST_ancillary_ds.nc`
/// lies next to `LST_in.nc`, and alphabetically the former stands first — uppercase
/// letters come before lowercase, so the dispute is decided on `a` against `i`. For an
/// OLCI granule all names are lowercase, and first alphabetically goes
/// `geo_coordinates.nc` — a file of pure latitudes with longitudes; the measurement
/// (`gifapar.nc`) stands right after it.
///
/// Usually the manifest parses such a granule, it names the measurement directly; but
/// the manifest may also be unavailable (`cdse.rs`, the `Asked::Manifest` branch), and
/// then the whole answer lies in the file names.
fn an_aside(key: &str) -> bool {
    const ASIDE: [&str; 4] = ["ancillary", "coordinates", "geodetic", "geolocation"];
    let name = file_name(key).to_ascii_lowercase();
    words(&name).any(|word| ASIDE.contains(&word))
}

/// Extensions behind which the measurement arrives, not a display picture: they can
/// hold both samples wider than a byte and a "no data" mark.
///
/// One list, not two: it is asked of what already passed [`is_raster`], and there
/// "not a measurement" means exactly "display".
const MEASURED_SUFFIXES: [&str; 6] = ["tif", "tiff", "jp2", "j2k", "nc", "h5"];

/// Whether the key carries a display picture rather than a measurement.
///
/// By extension, not by name: for Landsat in the ESA archive the overview picture is
/// named after the product and differs from the bands only in that —
/// `LS05_…_52FE.BP.PNG` against `LT51…_B1.TIF` — and no list can enumerate every such
/// picture name. Measurements are not carried in PNG: it can hold neither samples wider
/// than a byte nor "no data".
fn a_picture_format(key: &str) -> bool {
    let name = file_name(key).to_ascii_lowercase();
    match name.rsplit_once('.') {
        Some((_, suffix)) => !MEASURED_SUFFIXES.contains(&suffix),
        None => true,
    }
}

/// The name parsed into words: every name parser shares the same separators, and they
/// cannot disagree — otherwise `BP` is a word in one place and a piece in another.
fn words(name: &str) -> impl Iterator<Item = &str> {
    name.split(['_', '-', '.', ' ']).filter(|word| !word.is_empty())
}

/// Whether the manifest named the key among the measurements. Manifest paths go from
/// the product root, keys — from the bucket root, and they meet by the tail.
fn measured_by(key: &str, measured: &[String]) -> bool {
    measured.iter().any(|href| key.ends_with(&format!("/{}", href)))
}

/// Whether the key lies in a display subdirectory rather than a measurement one.
///
/// The `preview/` directory is part of the layout, and it says the same as the word:
/// inside are an overview picture, the instrument colour scale, the mission logo.
/// Nothing there is a detailed raster, and by file name this cannot be parsed: for
/// Sentinel-1 OCN `preview/icons/logo.png` would otherwise claim the detailed raster —
/// "logo" stands earlier than "measurement" in the alphabet.
fn a_decoration(key: &str) -> bool {
    key.contains("/preview/")
}

/// A name that usually hides a whole colour picture rather than one of its bands.
///
/// By a whole word, not a piece: "TCI" inside a word means nothing, and as a substring
/// it catches `otci.nc` — the OLCI chlorophyll index, which shares only three letters
/// with a colour picture.
fn a_whole_picture(key: &str) -> bool {
    let name = file_name(key).to_ascii_uppercase();
    let mut parts = words(&name).peekable();
    while let Some(word) = parts.next() {
        if ["TCI", "TRUECOLOR", "RGB"].contains(&word) {
            return true;
        }
        if word == "TRUE" && parts.peek() == Some(&"COLOR") {
            return true;
        }
    }
    false
}

/// Whether the band is named after the same quantity as the product itself.
///
/// CLMS builds the file name from the product name and the band prefix: the product
/// `c_gls_LST_202608271600_GLOBE_GEO_V3.0.1_cog` holds `c_gls_LST-LST_…`,
/// `c_gls_LST-ERRORBAR_…`, `c_gls_LST-QFLAG_…` and `c_gls_LST-TDELTA_…`, all four
/// readable rasters of one size. Alphabetically the first of them is the error bar,
/// and it would land on the globe — the spread in kelvins instead of the temperature
/// itself, and one cannot be told from the other by looks: the "no data" mask is shared
/// by the bands.
///
/// By repetition of a word, not a list of service prefixes: a list would have to be
/// kept for all CLMS products, while the product names its own quantity. The word must
/// occur in the file name **more often** than in the product name — a bare occurrence
/// is not enough: the whole product name lies in each of the four bands, and all of
/// them would answer at once.
///
/// Numbers do not count as words here: the date and version pieces repeat in band
/// names by themselves and say nothing about the quantity.
///
/// Counted over the last path segment: neighbouring segments bring their own words
/// into the count, and a repetition that happened higher up the storage tree would
/// cancel a correct match — the quantity would already be named twice before the band.
///
/// The repetition is literal, word for word. For a product named together with its
/// resolution (`c_gls_LAI300_…` against a `-LAI` band) the words differ, and the rule
/// stays silent — the alphabet decides, as everywhere there is nothing to repeat.
///
/// "No" here is the usual answer, not a miss: Landsat bands are named `_B1`…`_B7`,
/// there is nothing to repeat in them, and the alphabet decides. A tier above stands
/// the whole colour picture ([`a_whole_picture`]): it is a ready image, while the
/// repetition of the quantity only tells measurement bands apart from each other.
fn names_the_measurand(key: &str, product: &str) -> bool {
    let product = file_name(product);
    let times = |name: &str, word: &str| {
        words(name).filter(|other| other.eq_ignore_ascii_case(word)).count()
    };
    words(product)
        .filter(|word| !word.bytes().all(|sign| sign.is_ascii_digit()))
        .any(|word| times(file_name(key), word) > times(product, word))
}

/// The file with pixel coordinates of the raster — where they lie separately from it.
///
/// This is how Sentinel-3 packs: the measurement `.nc` holds one band of numbers, while
/// latitude with longitude lie in a neighbouring file of the same product. Without it
/// the swath band has nowhere to sit: the catalogue footprint says which piece of Earth
/// was shot, but not which pixel goes where.
///
/// It is sought among the directory neighbours by name — only this module knows the
/// product layout. The answer order goes from exact to cheap:
///  * `geodetic_tx.nc` — the reference grid of the swath, shared by all SLSTR grids:
///    an order of magnitude cheaper than the per-sample one, and it stands in its own
///    instrument sample and seats on the raster only through offsets declared by both
///    files (see `image-tiler::netcdf::seating`). The first answer for kilometre grids
///    (`i`, `f`) and for the reference itself;
///  * `geodetic_<grid>.nc` — per-sample coordinates of the grid the raster is recorded
///    on (SLSTR keeps one file per grid: `_in`, `_an`, `_fn`). The first answer for
///    half-kilometre grids (`a`, `b`, `c`): reference nodes deviate from per-sample by
///    453 m at median and 1667 m at worst — less than a kilometre pixel and one to
///    three half-kilometre ones — and a georeference coarser than the raster would eat
///    its detail. For the rest — when the product has no reference grid;
///  * `tie_geo_coordinates.nc` — the OLCI reference grid: for full resolution this is
///    1.2 MB against 50 MB of the per-sample file, and it holds nodes to spare (the
///    georeference is taken from the lattice anyway);
///  * `geo_coordinates.nc` — OLCI per-sample coordinates, when the product has no
///    reference grid;
///  * `geolocation.nc` — how SYNERGY (`SY_2_SYN`) names its file, which has none of
///    the names above.
///
/// Empty — the coordinates are either inside the raster itself or absent; there is
/// nothing to ask.
pub fn geolocation(keys: &[String], raster: &str) -> Option<String> {
    // Only next to the raster: a product may have several subdirectories, and
    // another grid's coordinates are worse than none.
    let folder = raster.rsplit_once('/')?.0;
    let sibling = |name: &str| {
        let wanted = format!("{}/{}", folder, name);
        keys.iter().find(|key| key.as_str() == wanted).cloned()
    };
    // The reference grid of the swath — the first answer for the SLSTR kilometre
    // grid, and also the cheapest: for an `SL_2_LST` granule it is 394 KB against
    // 2.2 MB of the per-sample file. Accuracy pays for it, and the price is measured:
    // `tx` nodes stand on the nominal instrument lattice and deviate from per-sample
    // coordinates by 453 m at median, 825 m at the ninety-fifth and 1667 m at worst.
    // Less than a pixel of the kilometre raster, and that is half the georeference:
    // there is no point densifying the lattice from such a file, it cannot go below
    // its own offset. For the half-kilometre grid the same offset is one to three
    // pixels, and its per-sample file answers it first.
    //
    // A raster recorded on the reference grid itself (`met_tx.nc`) gets the same
    // file — it has no other grid of its own.
    if let Some(grid) = grid_tag(raster) {
        let own = format!("geodetic_{}.nc", grid);
        let order = match grid_rank(raster).0 == 0 {
            true => [own.as_str(), "geodetic_tx.nc"],
            false => ["geodetic_tx.nc", own.as_str()],
        };
        if let Some(found) = order.iter().find_map(|name| sibling(name)) {
            return Some(found);
        }
    }
    sibling("tie_geo_coordinates.nc")
        .or_else(|| sibling("geo_coordinates.nc"))
        .or_else(|| sibling("geolocation.nc"))
}

/// The name tail by which Sentinel-3 names the recording grid: `LST_in.nc` → `in`,
/// `F1_BT_fn.nc` → `fn`. Two letters, both from closed value lists: the first is the
/// grid itself (`a`, `b`, `c` — radiometric channels, `i` — infrared, `f` — fire,
/// `t` — the reference one), the second is the view (`n` — nadir, `o` — oblique,
/// `x` — shared reference).
///
/// By lists, not "two lowercase letters": such a rule would cover `LST_ancillary_ds.nc`
/// and OLCI's `chl_nn.nc`, which have no grid in their tails. A coordinate file will
/// not be found for them anyway, but a rule answering "yes" about the wrong thing will
/// eventually stumble on something.
///
/// `None` — the name is not built that way, and nothing is said about a grid.
fn grid_tag(key: &str) -> Option<&str> {
    const GRIDS: [char; 6] = ['a', 'b', 'c', 'i', 'f', 't'];
    const VIEWS: [char; 3] = ['n', 'o', 'x'];
    let name = file_name(key);
    let stem = name.strip_suffix(".nc")?;
    let (_, tag) = stem.rsplit_once('_')?;
    let mut sign = tag.chars();
    let (grid, view) = (sign.next()?, sign.next()?);
    let shaped = sign.next().is_none() && GRIDS.contains(&grid) && VIEWS.contains(&view);
    shaped.then_some(tag)
}

/// Grid rank of the recording by its tail ([`grid_tag`]): the smaller, the denser —
/// (grid, view). Grids `a`, `b`, `c` are half a kilometre, `i` and `f` a kilometre,
/// `t` the reference one, rarest of all; nadir view (`n`) before oblique (`o`): the
/// oblique one is narrower and shot at an angle, and the shared reference (`x`) comes
/// last. A name without a grid tail says nothing about a grid and counts as kilometre
/// nadir: no better and no worse than an ordinary measurement, so that for a product
/// without such tails (OLCI, Landsat) the rank decides nothing.
///
/// This measure is about grid density, not file size: one grid spans three octaves of
/// bytes in SLSTR, and the kilometre thermal channel weighs more than the half-kilometre
/// visible one; size cannot measure density.
fn grid_rank(key: &str) -> (u8, u8) {
    let Some(tag) = grid_tag(key) else { return (1, 0) };
    let mut sign = tag.chars();
    let grid = match sign.next() {
        Some('a' | 'b' | 'c') => 0,
        Some('i' | 'f') => 1,
        _ => 2,
    };
    let view = match sign.next() {
        Some('n') => 0,
        Some('o') => 1,
        _ => 2,
    };
    (grid, view)
}

/// Extensions behind which a raster stands. They are listed here and nowhere else:
/// two parties ask about this — both the overlay and the storage layout
/// (`s3::is_single_object`) — and two lists of the same thing will once diverge
/// silently, and a raster file will show up as a folder without a single file inside.
///
/// The same set the tiler opens (`adapters::describe`): there the format is determined
/// by content, so the extension here is only a guess about whether it is worth opening
/// at all.
const RASTER_SUFFIXES: [&str; 12] = [
    "tif", "tiff", "jp2", "j2k", "png", "jpg", "jpeg", "nc", "h5", "gif", "bmp", "webp",
];

/// Whether the key looks like a raster — by its extension alone.
pub fn is_raster(identifier: &str) -> bool {
    let name = identifier.trim_end_matches('/');
    let Some((_, suffix)) = name.rsplit_once('.') else { return false };
    let suffix = suffix.to_ascii_lowercase();
    RASTER_SUFFIXES.contains(&suffix.as_str())
}

/// The product lies in storage as a single object: itself a raster — if it is one.
///
/// There is no layout to parse here, and the extension decides — the only case where
/// it is not a guess but everything known about the product at all. Being wrong with
/// it is safe: the tiler determines the format by content anyway and refuses what does
/// not look like one.
pub fn single(identifier: &str) -> Option<(String, Role)> {
    let name = identifier.trim_end_matches('/');
    is_raster(name).then(|| (name.to_string(), Role::Detailed))
}

/// Why this product cannot be put on the globe. Empty — it can, more precisely "it
/// seems so".
///
/// As a reason, not a flag: an icon that vanished without explanation leaves the
/// viewer guessing, and there is something to explain — both refusals here are named
/// in words and differ from each other. Level 0 raw is a hard no: such a product holds
/// no image at all, only receiver echoes. A single-object product is seen through and
/// through: whether it is a raster is said by the extension. And about a folder without
/// a listing there is nothing to say, and "it seems so" is more honest than a refusal
/// here — there are many layouts, a listing costs a request, and a raster not found in
/// it will be explained by [`nothing_here`].
///
/// `level` — the processing level, if known (in the storage listing there is no one to
/// report it: the level lives in the catalogue attributes).
/// `kind` — the product type from the catalogue (`productType`); empty where no one
/// said it (the storage listing knows only paths).
pub fn unviewable(identifier: &str, folder: bool, level: Option<u32>, kind: &str) -> String {
    if level == Some(0) {
        return "this is level 0 raw data: the product holds no image at all, only receiver echoes"
            .to_string();
    }
    if let Some(why) = hopeless_type(kind) {
        return why.to_string();
    }
    match folder || single(identifier).is_some() {
        true => String::new(),
        false => just_a_file(identifier),
    }
}

/// Product types that will never yield an image no matter how they are read.
///
/// This is asked before downloading — that is where its value lies. The product
/// declares its type, and the catalogue has already parsed it; without this the refusal
/// comes from the decoder on the first chunk, that is after gigabytes have already
/// arrived over the wire, while the whole path before that looked sound.
///
/// The list is deliberately short, and it is kept short not by laziness but by the
/// price of a mistake. A needless download a person notices and never repeats; a dimmed
/// icon over something that would open they will never notice — they will simply
/// conclude there is no data.
///
/// Therefore the whole PRODUCT is dimmed, not the raster, and the measure here is its
/// own: only what makes every part unshowable qualifies. Radar before aperture
/// compression (SLC) does not go here, though its measurement raster really is complex:
/// a quicklook lies next to it and is shown — dimming the product would take away the
/// only way to see it. The complex sample is caught where it lives — on the raster
/// description (`tiff::sampled`).
fn hopeless_type(kind: &str) -> Option<&'static str> {
    let kind = kind.to_ascii_uppercase();
    if kind.starts_with("AUX") {
        return Some("this is instrument auxiliary data, not an acquisition");
    }
    // Sentinel-5P level 1B. The level cannot catch it: `scene::level` reads the
    // last digit, and "1B" is a one for it, that is an ordinary first level.
    //
    // It speaks about the product layout, not about our inability. The measurement
    // here is three-dimensional — row × pixel × channel — and no ready picture lies
    // anywhere. There is something two-dimensional in the file, but it is not a scene:
    // viewing angles and instrument tables, that is "from where they looked", not
    // "what was shot". The tiler takes the first of them by its own order and puts an
    // angle map on the globe — a picture that looks like data and is not.
    //
    // A slice of the cube by a single channel would become an image, and coordinates
    // for it lie nearby. But which of hundreds of channels is the picture, no one
    // says, and across the swath the first band has 77 samples, that is tens of
    // kilometres per pixel. The ready and useful part of this cube is already computed
    // by the ground processor and lies at level 2 — that is what we show.
    let cube = ["L1B_RA_BD", "L1B_IR_", "L1B_ENG", "L1B_CA"];
    if cube.iter().any(|head| kind.starts_with(head)) {
        return Some(
            "this is a radiometer spectral cube: it holds no ready image — \
             the measurement goes over hundreds of channels, and only viewing angles are two-dimensional",
        );
    }
    None
}

/// The formats the overlay opens — in words, for the refusal.
///
/// Assembled from the same [`RASTER_SUFFIXES`] that select the raster: written as a
/// separate list, the two would diverge — `h5` could be selected but never named — and
/// they are shown to one person from both sides.
fn readable() -> String {
    fn said(suffix: &str) -> &str {
        match suffix {
            "tif" | "tiff" => "TIFF",
            "jp2" | "j2k" => "JPEG 2000",
            "png" => "PNG",
            "jpg" | "jpeg" => "JPEG",
            "nc" => "NetCDF",
            "h5" => "HDF5",
            "gif" => "GIF",
            "bmp" => "BMP",
            "webp" => "WebP",
            other => other,
        }
    }
    let mut names: Vec<&str> = Vec::new();
    for suffix in RASTER_SUFFIXES {
        let name = said(suffix);
        if !names.contains(&name) {
            names.push(name);
        }
    }
    names.join(", ")
}

/// The product is a single file, and it is not a raster. What kind of file is visible
/// from the file itself, and that is the whole answer: the overlay cannot unpack
/// archives.
pub fn just_a_file(identifier: &str) -> String {
    let name = file_name(identifier);
    match name.rsplit_once('.') {
        Some((_, suffix)) => format!(
            "the product is a single .{} file, but an overlay can take a raster — {}",
            suffix.to_ascii_lowercase(),
            readable()
        ),
        None => format!("the product is a single file, but an overlay can take a raster — {}", readable()),
    }
}

/// Why no rasters were found in the product subtree — by the same keys they were
/// sought with.
///
/// An answer, not an excuse: "no rasters" sounds the same over level 0 raw, where
/// there is no image yet, and over a picture with an unfamiliar layout, where there is
/// one and we did not recognize it. They are handled differently, and from the outside
/// only words can tell them apart.
pub fn nothing_here(identifier: &str, keys: &[String]) -> String {
    if keys.is_empty() {
        return "the storage holds no files at this path".to_string();
    }
    let name = file_name(identifier);
    // Level 0 raw is receiver echo, not an image: only a ground processor can build a
    // picture from it, and in such a product a raster has nowhere to come from.
    if name.contains("_RAW__0S") {
        return format!(
            "this is level 0 raw data: the product holds no image yet, only echo signals ({} files)",
            keys.len()
        );
    }
    // The layout is not at fault here: it is no longer required to be recognizable —
    // for an unrecognized one the first readable file is shown. So not a single
    // readable file was found, and that must be said through what the product holds.
    format!(
        "among the {} product files there is no raster — what lies there: {}",
        keys.len(),
        suffixes(keys)
    )
}

/// The extensions of the keys as a list, once each. They name the product content in
/// the explanation: ".dat, .xml" at once says there was never a picture there.
fn suffixes(keys: &[String]) -> String {
    let mut seen: Vec<String> = Vec::new();
    for key in keys {
        let name = key.rsplit('/').next().unwrap_or("");
        let Some((_, suffix)) = name.rsplit_once('.') else { continue };
        let suffix = format!(".{}", suffix.to_ascii_lowercase());
        if !seen.contains(&suffix) {
            seen.push(suffix);
        }
    }
    match seen.is_empty() {
        true => "files without extensions".to_string(),
        false => seen.join(", "),
    }
}
