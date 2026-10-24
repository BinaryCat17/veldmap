#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Role {
    Preview,
    Detailed,
}

pub fn scan(product: &str, keys: &[String], measured: &[String], downloaded: bool) -> Scan {
    let mut rasters = Vec::new();

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

    if rasters.is_empty() {
        let readable: Vec<&String> = keys.iter().filter(|key| is_raster(key)).collect();
        if let Some(quicklook) = readable.iter().copied().find(|key| a_quicklook(key)) {
            rasters.push((quicklook.clone(), Role::Preview));
        }
        let named: Vec<&String> =
            readable.iter().copied().filter(|key| measured_by(key, measured)).collect();
        let among = match named.is_empty() {
            true => readable,
            false => named,
        };
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
        return Scan { rasters, alternates, guessed: true };
    }

    Scan { rasters, alternates: Vec::new(), guessed: false }
}

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

pub struct Scan {
    pub rasters: Vec<(String, Role)>,
    pub alternates: Vec<String>,
    pub guessed: bool,
}

fn file_name(key: &str) -> &str {
    key.trim_end_matches('/').rsplit('/').next().unwrap_or("")
}

fn a_quicklook(key: &str) -> bool {
    let name = file_name(key).to_ascii_lowercase();
    let by_piece = ["quick-look", "quicklook", "thumb", "browse", "preview", "_pvi"]
        .iter()
        .any(|hint| name.contains(hint));
    by_piece || words(&name).any(|word| word == "bp")
}

fn an_aside(key: &str) -> bool {
    const ASIDE: [&str; 4] = ["ancillary", "coordinates", "geodetic", "geolocation"];
    let name = file_name(key).to_ascii_lowercase();
    words(&name).any(|word| ASIDE.contains(&word))
}

const MEASURED_SUFFIXES: [&str; 6] = ["tif", "tiff", "jp2", "j2k", "nc", "h5"];

fn a_picture_format(key: &str) -> bool {
    let name = file_name(key).to_ascii_lowercase();
    match name.rsplit_once('.') {
        Some((_, suffix)) => !MEASURED_SUFFIXES.contains(&suffix),
        None => true,
    }
}

fn words(name: &str) -> impl Iterator<Item = &str> {
    name.split(['_', '-', '.', ' ']).filter(|word| !word.is_empty())
}

fn measured_by(key: &str, measured: &[String]) -> bool {
    measured.iter().any(|href| key.ends_with(&format!("/{}", href)))
}

fn a_decoration(key: &str) -> bool {
    key.contains("/preview/")
}

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

fn names_the_measurand(key: &str, product: &str) -> bool {
    let product = file_name(product);
    let times = |name: &str, word: &str| {
        words(name).filter(|other| other.eq_ignore_ascii_case(word)).count()
    };
    words(product)
        .filter(|word| !word.bytes().all(|sign| sign.is_ascii_digit()))
        .any(|word| times(file_name(key), word) > times(product, word))
}

pub fn geolocation(keys: &[String], raster: &str) -> Option<String> {
    let folder = raster.rsplit_once('/')?.0;
    let sibling = |name: &str| {
        let wanted = format!("{}/{}", folder, name);
        keys.iter().find(|key| key.as_str() == wanted).cloned()
    };
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

const RASTER_SUFFIXES: [&str; 12] = [
    "tif", "tiff", "jp2", "j2k", "png", "jpg", "jpeg", "nc", "h5", "gif", "bmp", "webp",
];

pub fn is_raster(identifier: &str) -> bool {
    let name = identifier.trim_end_matches('/');
    let Some((_, suffix)) = name.rsplit_once('.') else { return false };
    let suffix = suffix.to_ascii_lowercase();
    RASTER_SUFFIXES.contains(&suffix.as_str())
}

pub fn single(identifier: &str) -> Option<(String, Role)> {
    let name = identifier.trim_end_matches('/');
    is_raster(name).then(|| (name.to_string(), Role::Detailed))
}

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

fn hopeless_type(kind: &str) -> Option<&'static str> {
    let kind = kind.to_ascii_uppercase();
    if kind.starts_with("AUX") {
        return Some("this is instrument auxiliary data, not an acquisition");
    }
    let cube = ["L1B_RA_BD", "L1B_IR_", "L1B_ENG", "L1B_CA"];
    if cube.iter().any(|head| kind.starts_with(head)) {
        return Some(
            "this is a radiometer spectral cube: it holds no ready image — \
             the measurement goes over hundreds of channels, and only viewing angles are two-dimensional",
        );
    }
    None
}

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

pub fn nothing_here(identifier: &str, keys: &[String]) -> String {
    if keys.is_empty() {
        return "the storage holds no files at this path".to_string();
    }
    let name = file_name(identifier);
    if name.contains("_RAW__0S") {
        return format!(
            "this is level 0 raw data: the product holds no image yet, only echo signals ({} files)",
            keys.len()
        );
    }
    format!(
        "among the {} product files there is no raster — what lies there: {}",
        keys.len(),
        suffixes(keys)
    )
}

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
