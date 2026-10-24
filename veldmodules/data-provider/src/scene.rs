use crate::proto::data_provider::{DataProduct, Part};

#[derive(Clone)]
pub struct Facts {
    pub platform: String,
    pub instrument: String,
    pub datatake: String,
    pub tile: String,
    pub named_tile: String,
    pub slice: String,
    pub orbit: String,
    pub second: i64,
    pub level: Option<u32>,
    pub kind: String,
    pub size: u64,
    pub framed: bool,
}

pub fn acquisition(facts: &Facts) -> Option<String> {
    if !facts.datatake.is_empty() && !facts.slice.is_empty() {
        return Some(format!(
            "by slice|{}|{}|{}",
            facts.platform, facts.datatake, facts.slice
        ));
    }
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
    if !facts.orbit.is_empty() {
        return Some(format!(
            "by orbit|{}|{}|{}|{}",
            facts.platform, facts.instrument, facts.orbit, facts.second
        ));
    }
    None
}

pub fn datatake_in_name(name: &str) -> Option<String> {
    let name = name.split('.').next().unwrap_or(name);
    let fields: Vec<&str> = name.split('_').collect();
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
    let datatake = fields.get(pair + 3)?;
    let hex = |field: &&str| field.len() == 6 && field.chars().all(|sign| sign.is_ascii_hexdigit());
    hex(datatake).then(|| u64::from_str_radix(datatake, 16).ok())?.map(|value| value.to_string())
}

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

const PACKAGING_SUFFIXES: [&str; 2] = ["_cog", "_nc"];

fn by_name(facts: &Facts, name: &str) -> String {
    format!("by name|{}|{}|{}", facts.platform, stem(name), facts.second)
}

fn stem(name: &str) -> &str {
    let stem = without_extension(name);
    PACKAGING_SUFFIXES
        .iter()
        .find_map(|suffix| strip_ignoring_case(stem, suffix))
        .unwrap_or(stem)
}

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

fn strip_ignoring_case<'a>(name: &'a str, suffix: &str) -> Option<&'a str> {
    let cut = name.len().checked_sub(suffix.len())?;
    let tail = name.get(cut..)?;
    tail.eq_ignore_ascii_case(suffix).then(|| &name[..cut])
}

pub fn rank(facts: &Facts, name: &str) -> (u32, bool, u64) {
    let order = match facts.level {
        Some(0) => u32::MAX,
        None => u32::MAX - 1,
        Some(level) => level,
    };
    (order, !tiled(&facts.kind, name), facts.size)
}

pub fn about(products: Vec<(Facts, DataProduct)>, asked: &str) -> Option<DataProduct> {
    let (kind, level, mine) = products
        .iter()
        .find(|(_, product)| product.identifier == asked)
        .map(|(facts, product)| (facts.kind.clone(), facts.level, product.clone()))?;

    let scene = group(products).into_iter().find(|scene| {
        scene.identifier == asked || scene.parts.iter().any(|part| part.identifier == asked)
    })?;

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

fn quantity(kind: &str) -> &str {
    for suffix in ["-COG", "_COG"] {
        let cut = kind.len().saturating_sub(suffix.len());
        if kind.len() > suffix.len() && kind[cut..].eq_ignore_ascii_case(suffix) {
            return &kind[..cut];
        }
    }
    kind
}

fn tiled(kind: &str, name: &str) -> bool {
    let cog = |text: &str| {
        let lower = text.to_ascii_lowercase();
        lower.ends_with("-cog") || lower.ends_with("_cog")
    };
    cog(kind) || cog(name)
}

pub fn level(said: &str) -> Option<u32> {
    said.bytes().rev().find(u8::is_ascii_digit).map(|digit| u32::from(digit - b'0'))
}

pub(crate) const BESIDE_A_SLICE_S: i64 = 10;

struct Slice {
    datatake: String,
    key: String,
    second: i64,
}

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

fn key_of(slices: &[Slice], facts: &Facts, name: &str) -> String {
    beside_a_slice(slices, facts)
        .or_else(|| acquisition(facts))
        .unwrap_or_else(|| by_name(facts, name))
}

pub fn same_scene(
    facts: &Facts,
    name: &str,
    others: Vec<(Facts, DataProduct)>,
) -> Vec<(Facts, DataProduct)> {
    let framed: Vec<(Facts, DataProduct)> =
        others.into_iter().filter(|(facts, _)| facts.framed).collect();
    let mut all = framed.clone();
    all.push((facts.clone(), DataProduct { name: name.to_string(), ..Default::default() }));
    let slices = slices(&all);

    let key = key_of(&slices, facts, name);
    framed
        .into_iter()
        .filter(|(other, product)| key_of(&slices, other, &product.name) == key)
        .collect()
}

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
