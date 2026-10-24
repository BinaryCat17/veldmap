//! Scenes and their parts: how scenes are assembled from catalogue products.
//!
//! The catalogue holds one scene as several products. Sometimes this is one
//! acquisition at different processing: receiver raw, the level-1 scene as a
//! striped TIFF, the same as tiled COG. Sometimes these are different measured
//! quantities of one pass: eight bands of a radiometer, seven gases over one
//! and the same strip of ground. To a human both are one scene, and the list
//! must show it as one row. Which part to show is decided here as well — by the
//! same knowledge and from the same facts.
//!
//! Everything here is pure functions over catalogue facts: response parsing
//! lives in catalogue.rs, the rules live here, and they are checked without a
//! network.

use crate::proto::data_provider::{DataProduct, Part};

/// What must be known about a product to assemble scenes. Separate from the
/// product itself because it lives in catalogue attributes and never leaves:
/// the caller needs the scene, not the ingredients it was assembled from.
#[derive(Clone)]
pub struct Facts {
    /// Satellite in catalogue words: "SENTINEL-1".
    pub platform: String,
    /// Instrument in catalogue words: "TROPOMI". Needed because one spacecraft
    /// may carry several of them — see [`acquisition`].
    pub instrument: String,
    /// Datatake — a continuous stretch of imaging. Empty for missions that do
    /// not report one (Sentinel-2 makes do with tiles).
    pub datatake: String,
    /// Grid tile — "43XDB" for Sentinel-2, as the catalogue names it. Empty
    /// for missions without a grid and for those the catalogue did not report
    /// it to.
    pub tile: String,
    /// Cell read from the product name (see [`tile_in_name`]). Separate from
    /// `tile` because the catalogue does not know it: asking for neighbours by
    /// it means asking about an attribute the product does not have and
    /// getting an empty answer. It names the scene on equal terms with `tile`,
    /// but never goes into catalogue queries.
    pub named_tile: String,
    /// Slice number within the datatake. A datatake is a continuous stretch of
    /// imaging, and it is cut into slices; the slice number is the only thing
    /// that names one and the same scene identically across all its parts.
    /// Empty for missions without slicing and for parts the catalogue did not
    /// report it to.
    pub slice: String,
    /// Orbit number. Empty if the catalogue did not report one.
    pub orbit: String,
    /// Second when imaging started.
    pub second: i64,
    /// Processing level — see [`level`]. `None` — the catalogue named none.
    pub level: Option<u32>,
    /// Product type in catalogue words: "IW_GRDH_1S-COG".
    pub kind: String,
    pub size: u64,
    /// The product has a footprint. This is what separates a scene from
    /// auxiliary data — see [`group`].
    pub framed: bool,
}

/// Scene key. `None` — nothing can say this product shares a scene with
/// anyone.
///
/// Products are linked only by what the catalogue itself said about them. A
/// shared second is not enough — Sentinel-3 images with two instruments at
/// once, and two of its products with the same time are two different scenes.
/// Leaving a scene ungrouped is better than merging foreign ones into one.
pub fn acquisition(facts: &Facts) -> Option<String> {
    // The slice number is the only thing the parts of one scene are named by
    // identically. A second in its place will not do: raw has its own slicing,
    // and its slice starts four seconds apart from the processed one, and a
    // second and a half from the complex one. By second these parts split into
    // different scenes, and one Sentinel-1 scene shows as three rows out of
    // five possible.
    if !facts.datatake.is_empty() && !facts.slice.is_empty() {
        return Some(format!(
            "by slice|{}|{}|{}",
            facts.platform, facts.datatake, facts.slice
        ));
    }
    // A datatake without a slice number, and a grid tile. The second is needed
    // in both cases: for a datatake it separates slices from each other, for a
    // tile — different passes over one and the same cell.
    let tile = match facts.tile.is_empty() {
        true => &facts.named_tile,
        false => &facts.tile,
    };
    if !facts.datatake.is_empty() || !tile.is_empty() {
        return Some(format!(
            "by scene|{}|{}|{}|{}",
            facts.platform, facts.datatake, tile, facts.second
        ));
    }
    // Neither — then the scene is named by the orbit together with the
    // instrument and the second. An orbit lasts an hour and a half, and on its
    // own it would glue the whole pass into one; the instrument is needed
    // because one spacecraft may carry several. This is how Sentinel-5P lies:
    // ten gases over one and the same five-minute granule. Radiometer bands of
    // the same orbit do not land here — they start a minute earlier, and that
    // is a separate scene, not parts of this one.
    if !facts.orbit.is_empty() {
        return Some(format!(
            "by orbit|{}|{}|{}|{}",
            facts.platform, facts.instrument, facts.orbit, facts.second
        ));
    }
    None
}

/// Datatake from a Sentinel-1 product name — as a decimal number, the way the
/// catalogue names it.
///
/// The catalogue does not name `datatakeID` for the whole archive: products
/// before autumn 2023 lack it entirely (the live catalogue shows 2019 and 2022
/// empty, the field is filled since October 2023). Without it the "by slice"
/// rule does not fire, the key falls back to "by orbit with second", and the
/// seconds of raw, complex and processed diverge by two to four — so one scene
/// shows as three rows out of five possible, exactly what [`acquisition`]
/// prevents.
///
/// In the name the datatake is always present — a hexadecimal field right
/// after the pair of imaging times and the orbit number:
/// `S1A_IW_GRDH_1SDV_<start>_<end>_<orbit>_<datatake>_<CRC>`.
/// It is found relative to that pair, not by counting fields: in `WV_SLC__1SSV`
/// the type takes two fields with an empty one between them, and the tiled
/// packaging adds `_COG` at the end, so counting from either edge breaks.
///
/// Read only from Sentinel-1 names, and this is not caution: the pair of times
/// stands in foreign names too, and the field after it means something else
/// there. In COP-DEM (`DEM1_SAR_DTE_30_<t1>_<t2>_ADS_000000_rLrO`) zeros stand
/// in the datatake's place, and all cells of one date would merge into a single
/// list row — 200 products out of 200 in the live catalogue.
pub fn datatake_in_name(name: &str) -> Option<String> {
    let name = name.split('.').next().unwrap_or(name);
    let fields: Vec<&str> = name.split('_').collect();
    // The satellite is the first field: `S1A`…`S1D`. That is what identifies
    // this name layout.
    let platform = fields.first()?;
    let sentinel1 = platform.len() == 3
        && platform.starts_with("S1")
        && platform.as_bytes()[2].is_ascii_uppercase();
    if !sentinel1 {
        return None;
    }
    let a_time = |field: &&str| {
        field.len() == 15
            && field.as_bytes()[8] == b'T'
            && field.chars().enumerate().all(|(at, sign)| at == 8 || sign.is_ascii_digit())
    };
    let pair = fields.windows(2).position(|pair| a_time(&pair[0]) && a_time(&pair[1]))?;
    // After the pair of times come the orbit and the datatake — both six
    // digits, but the orbit is decimal and the datatake hexadecimal.
    let datatake = fields.get(pair + 3)?;
    let hex = |field: &&str| field.len() == 6 && field.chars().all(|sign| sign.is_ascii_hexdigit());
    hex(datatake).then(|| u64::from_str_radix(datatake, 16).ok())?.map(|value| value.to_string())
}

/// Degree-grid cell from a product name — "N03W004", the way Sentinel-1 RTC
/// names its tiles.
///
/// The catalogue is silent about it: `tileId` is empty for these products, as
/// are the datatake and the slice number — and without a tile the scene key
/// falls back to orbit with second. And neighbouring cells share one orbit and
/// one second: the pass is cut into cells after imaging, so all their time
/// comes from it. A dozen cells at different corners of the Earth would become
/// a single list row.
///
/// Read strictly: two latitude digits, three longitude digits, hemispheres as
/// letters, and an underscore right after the cell. Nothing matches this by
/// accident — other missions' names start with the platform ("S1A_", "S2B_").
pub fn tile_in_name(name: &str) -> Option<String> {
    let tile = name.split('_').next()?;
    let sign = tile.as_bytes();
    let shaped = tile.len() == 7
        && matches!(sign[0], b'N' | b'S')
        && matches!(sign[3], b'E' | b'W')
        && sign[1..3].iter().all(u8::is_ascii_digit)
        && sign[4..7].iter().all(u8::is_ascii_digit);
    shaped.then(|| tile.to_string())
}

/// Suffixes that record the packaging, not the scene, in a product name. CDSE
/// climate data comes in two of these: `…_V3.0.1_cog` and `…_V3.0.1_nc` — the
/// same quantities in COG and in NetCDF.
const PACKAGING_SUFFIXES: [&str; 2] = ["_cog", "_nc"];

/// Scene key by name — a fallback for products the catalogue linked by
/// nothing: no datatake, no tile, no orbit.
///
/// The names of such a scene's parts match completely, except for the tail that
/// names the packaging. The rule is deliberately narrow: a full name match
/// merges nothing (the catalogue never has two products with one name), and
/// exactly what differs by a known suffix gets merged.
///
/// Case does not matter, and the suffix need not stand at the very end: for
/// climate data it is `…_V3.0.1_cog` with no extension at all, while
/// third-party providers use `…_5415_COG.DIMA`, where an extension follows the
/// packaging. Searching for it only in the tail and only lowercase would
/// disagree with [`tiled`], which looks at the same thing and lets neither of
/// these cases slip.
fn by_name(facts: &Facts, name: &str) -> String {
    format!("by name|{}|{}|{}", facts.platform, stem(name), facts.second)
}

/// Name without whatever names the packaging in it: without the container
/// extension and without the suffix tail. Both are stripped because packaging
/// is written sometimes one way, sometimes the other, and sometimes both at
/// once: `…_5415_COG` versus `…_5415.DIMA` and `…_675b.DIMA` versus
/// `…_675b_COG.DIMA` — both pairs are about one scene.
fn stem(name: &str) -> &str {
    let stem = without_extension(name);
    PACKAGING_SUFFIXES
        .iter()
        .find_map(|suffix| strip_ignoring_case(stem, suffix))
        .unwrap_or(stem)
}

/// Name without the container extension.
///
/// An extension is recognized by shape, not by a lone dot: in climate names the
/// dot stands inside the version number (`…_V3.0.1_nc`), and cutting at the
/// last dot would leave a stub of a name. A real extension is a short tail of
/// letters and digits with at least one letter in it.
fn without_extension(name: &str) -> &str {
    let Some((head, tail)) = name.rsplit_once('.') else { return name };
    let looks_like = (2..=5).contains(&tail.len())
        && tail.bytes().all(|byte| byte.is_ascii_alphanumeric())
        && tail.bytes().any(|byte| byte.is_ascii_alphabetic());
    match looks_like {
        true => head,
        false => name,
    }
}

/// Name without the named tail, if it is there; case does not matter.
fn strip_ignoring_case<'a>(name: &'a str, suffix: &str) -> Option<&'a str> {
    let cut = name.len().checked_sub(suffix.len())?;
    // By slicing, not symbol by symbol: the suffixes here are ASCII, and at an
    // invalid character boundary the slice simply fails to assemble.
    let tail = name.get(cut..)?;
    tail.eq_ignore_ascii_case(suffix).then(|| &name[..cut])
}

/// Preference order of parts: smaller goes first, and the first one is shown.
///
/// Level-0 raw is always last: it holds no image at all, only the receiver's
/// echo. Before it goes whatever level the catalogue did not name: service
/// annotations to a scene lie there (`IW_ETA__AX` arrives a week after the
/// scene itself), and they are small, so under the other rules they would land
/// in front of the gigabyte scene and be shown in its place. Then level in
/// ascending order — level 1 is the scene itself, level 2 already derived
/// quantities on top of it. Tiled packaging goes before striped: it is read by
/// a window, while the striped top pyramid level would cost reading the whole
/// file. Otherwise — the smaller one: within one scene it is either the same
/// picture packed denser, or a quantity that is cheaper to show.
pub fn rank(facts: &Facts, name: &str) -> (u32, bool, u64) {
    let order = match facts.level {
        Some(0) => u32::MAX,
        None => u32::MAX - 1,
        Some(level) => level,
    };
    (order, !tiled(&facts.kind, name), facts.size)
}

/// The answer about a single product: the product itself together with its
/// scene mates — or a mate, if there is nothing to show of the product itself.
/// `None` — the asked-for product is not among these.
///
/// Substitution is justified exactly twice. Level-0 raw holds no image at all:
/// showing the processed scene of the same acquisition in its place is an
/// answer, not a fraud. And the same quantity lying nearby in tiled packaging
/// beats the striped one, which would have to be read whole for a single tile.
///
/// In all other cases substitution is forbidden, and this is not a quibble:
/// parts of one scene can be different measured quantities, and answering a
/// question about carbon monoxide with ozone is a lie. The order of parts stays
/// shared (see [`rank`]) — only the "shown" mark moves.
pub fn about(products: Vec<(Facts, DataProduct)>, asked: &str) -> Option<DataProduct> {
    let (kind, level, mine) = products
        .iter()
        .find(|(_, product)| product.identifier == asked)
        .map(|(facts, product)| (facts.kind.clone(), facts.level, product.clone()))?;

    let scene = group(products).into_iter().find(|scene| {
        scene.identifier == asked || scene.parts.iter().any(|part| part.identifier == asked)
    })?;

    // The type is taken from the shown part. The scene itself carries the same
    // one — the scene is named after that part — but it still has to be found:
    // below, that same one decides whether the set is expanded.
    let shown = scene.parts.iter().find(|part| part.shown).map(|part| part.product_type.clone());
    let same_quantity = shown.is_some_and(|shown| quantity(&kind) == quantity(&shown));
    if scene.identifier == asked || level == Some(0) || same_quantity {
        return Some(scene);
    }
    let parts = scene
        .parts
        .into_iter()
        .map(|part| Part { shown: part.identifier == asked, ..part })
        .collect();
    Some(DataProduct { parts, ..mine })
}

/// Quantity without the packaging tail: `IW_GRDH_1S-COG` and `IW_GRDH_1S` are
/// the same acquisition and processing, packaged differently.
fn quantity(kind: &str) -> &str {
    for suffix in ["-COG", "_COG"] {
        let cut = kind.len().saturating_sub(suffix.len());
        if kind.len() > suffix.len() && kind[cut..].eq_ignore_ascii_case(suffix) {
            return &kind[..cut];
        }
    }
    kind
}

/// The part is packaged as tiles — it serves an arbitrary tile without reading
/// itself whole. This is stated either by the product type
/// (`IW_GRDH_1S-COG` for Sentinel-1) or by a name tail (`…_V3.0.1_cog` for
/// climate data): the same thing is written differently, which means it is one
/// and the same thing.
fn tiled(kind: &str, name: &str) -> bool {
    let cog = |text: &str| {
        let lower = text.to_ascii_lowercase();
        lower.ends_with("-cog") || lower.ends_with("_cog")
    };
    cog(kind) || cog(name)
}

/// Processing level from a catalogue word. `None` — the catalogue kept silent
/// about it.
///
/// Its form is not single: "LEVEL1" for Sentinel-1, "S2MSI1C" for Sentinel-2.
/// What is common is that the level is written as the **last** digit — the
/// first one for Sentinel-2 belongs to the instrument name.
///
/// A silent catalogue is not "like the majority" but a separate answer, and the
/// difference is not theoretical: service annotations to a scene carry no level
/// word, and counted as level 1 they land in the parts list above the scene
/// itself (see [`rank`]) — they are small, after all.
pub fn level(said: &str) -> Option<u32> {
    said.bytes().rev().find(u8::is_ascii_digit).map(|digit| u32::from(digit - b'0'))
}

/// How far from the start of a slice a part the catalogue left without a slice
/// number may begin.
///
/// Ten seconds: parts of one slice diverge in start by four seconds (raw),
/// while the slices themselves stand twenty-five seconds and more apart. So the
/// window generously covers its own and generously falls short of the
/// neighbour.
///
/// The same measure asks the catalogue for neighbours of every product with a
/// datatake (`catalogue::siblings`): the "by slice" rule does not limit time at
/// all, the query has no boundary of its own, and it is taken from here. Asking
/// narrower than one groups by means not getting in the answer the very part
/// one meant to attach.
pub(crate) const BESIDE_A_SLICE_S: i64 = 10;

/// A datatake slice: what names it and when it started.
struct Slice {
    datatake: String,
    key: String,
    second: i64,
}

/// Scene slices: named by catalogue number, and where it named none — marked
/// by time.
///
/// The second is needed because the catalogue reports slice numbers not for
/// the whole archive, and there is a datatake where no part has one (a live
/// example is scene 191698 of 5 March 2019). There is nothing to attach such
/// parts to, and the key falls back to "datatake with second", while the
/// seconds of raw, complex and processed diverge by two to four — one scene
/// scattering over rows by exactly what [`acquisition`] prevents.
///
/// The marking window is the same as for attaching: parts of one slice diverge
/// in start by four seconds, while the slices themselves stand twenty-five
/// seconds and more apart.
///
/// Dividing this way is allowed only because a non-empty datatake belongs to a
/// single mission — Sentinel-1: the catalogue never names one for the rest
/// (Sentinel-2 has `tileId` in its place). Were it shared across many tiles,
/// they would merge into one row.
fn slices(products: &[(Facts, DataProduct)]) -> Vec<Slice> {
    let mut named: Vec<Slice> = products
        .iter()
        .filter(|(facts, _)| !facts.slice.is_empty() && !facts.datatake.is_empty())
        .filter_map(|(facts, _)| {
            Some(Slice {
                datatake: facts.datatake.clone(),
                key: acquisition(facts)?,
                second: facts.second,
            })
        })
        .collect();

    // Parts of datatakes where nobody has a slice number. By ascending time,
    // so that marking does not depend on the catalogue's output order.
    let mut nameless: Vec<&Facts> = products
        .iter()
        .map(|(facts, _)| facts)
        .filter(|facts| !facts.datatake.is_empty() && facts.slice.is_empty())
        .filter(|facts| !named.iter().any(|slice| slice.datatake == facts.datatake))
        .collect();
    nameless.sort_by(|a, b| (&a.datatake, a.second).cmp(&(&b.datatake, b.second)));

    for facts in nameless {
        let beside = named.iter().any(|slice| {
            slice.datatake == facts.datatake && (slice.second - facts.second).abs() <= BESIDE_A_SLICE_S
        });
        if !beside {
            named.push(Slice {
                datatake: facts.datatake.clone(),
                key: format!("beside a slice|{}|{}|{}", facts.platform, facts.datatake, facts.second),
                second: facts.second,
            });
        }
    }
    named
}

/// Key of the slice this part was taken beside. `None` — the part has its own
/// slice number, or there is no such slice nearby.
///
/// Needed because the catalogue reports slice numbers not for every part:
/// Sentinel-1's `OCN` lacks one, and without this move the derived quantity
/// would ride off into a scene of its own — even though it starts at the very
/// same microsecond as the scene it was computed from.
fn beside_a_slice(slices: &[Slice], facts: &Facts) -> Option<String> {
    if !facts.slice.is_empty() || facts.datatake.is_empty() {
        return None;
    }
    slices
        .iter()
        .filter(|slice| slice.datatake == facts.datatake)
        .min_by_key(|slice| (slice.second - facts.second).abs())
        .filter(|slice| (slice.second - facts.second).abs() <= BESIDE_A_SLICE_S)
        .map(|slice| slice.key.clone())
}

/// Catalogue products → scenes, in catalogue order (fresh on top).
///
/// The scene key of a single product — the one thing the whole module answers
/// "is this one scene" with.
///
/// Slices come as an argument rather than being computed here: they are
/// recognized over the whole set at once, while the key is asked per single
/// product.
fn key_of(slices: &[Slice], facts: &Facts, name: &str) -> String {
    beside_a_slice(slices, facts)
        .or_else(|| acquisition(facts))
        .unwrap_or_else(|| by_name(facts, name))
}

/// Those from the set taken by the same scene as the named product.
///
/// The answer here is exactly the one [`group`] groups scenes with, and that is
/// not politeness. A second answer would diverge: the reverse lookup by key
/// picks neighbours with the bare [`acquisition`], and for parts without a
/// slice number the keys disagree — a part the search shows inside a scene
/// stays a loner by key. The same list row then unfolds differently depending
/// on where it was asked about.
pub fn same_scene(
    facts: &Facts,
    name: &str,
    others: Vec<(Facts, DataProduct)>,
) -> Vec<(Facts, DataProduct)> {
    let framed: Vec<(Facts, DataProduct)> =
        others.into_iter().filter(|(facts, _)| facts.framed).collect();
    // Slices — over the whole set together with the asked-for one itself: its
    // slice also names the scene that parts without a number attach to.
    let mut all = framed.clone();
    all.push((facts.clone(), DataProduct { name: name.to_string(), ..Default::default() }));
    let slices = slices(&all);

    let key = key_of(&slices, facts, name);
    framed
        .into_iter()
        .filter(|(other, product)| key_of(&slices, other, &product.name) == key)
        .collect()
}

/// Not a scene is what has no footprint. Auxiliary data (calibration tables,
/// ephemerides) the catalogue serves as products on par with a scene, and in
/// freshness they outpace it: a table's validity is written as a future date.
/// There is nothing to outline on the Earth with them, nothing to show, and
/// they have no place in the scene list.
///
/// The parts list is filled only where there is more than one part: a single
/// part is the scene itself, and repeating it with its own content is
/// pointless.
pub fn group(products: Vec<(Facts, DataProduct)>) -> Vec<DataProduct> {
    let mut order: Vec<String> = Vec::new();
    let mut scenes: std::collections::HashMap<String, Vec<(Facts, DataProduct)>> =
        std::collections::HashMap::new();

    let framed: Vec<(Facts, DataProduct)> =
        products.into_iter().filter(|(facts, _)| facts.framed).collect();
    let slices = slices(&framed);

    for (facts, product) in framed {
        let key = key_of(&slices, &facts, &product.name);
        if !scenes.contains_key(&key) {
            order.push(key.clone());
        }
        scenes.entry(key).or_default().push((facts, product));
    }

    order
        .into_iter()
        .filter_map(|key| {
            let mut packed = scenes.remove(&key)?;
            packed.sort_by_key(|(facts, product)| rank(facts, &product.name));
            let parts = match packed.len() {
                0 => return None,
                1 => Vec::new(),
                _ => packed
                    .iter()
                    .enumerate()
                    .map(|(at, (facts, product))| Part {
                        identifier: product.identifier.clone(),
                        name: product.name.clone(),
                        product_type: facts.kind.clone(),
                        size: product.size,
                        folder: product.folder,
                        shown: at == 0,
                        unviewable: product.unviewable.clone(),
                    })
                    .collect(),
            };
            let (_, mut shown) = packed.into_iter().next()?;
            shown.parts = parts;
            Some(shown)
        })
        .collect()
}
