//! Метрика цели «читать только нужное» — тестами на фальшивом хосте.
//!
//! Фикстуры — настоящие TIFF, записанные энкодером крейта `tiff`: полосный —
//! его же `ImageEncoder`, тайловый с копиями — руками через
//! `DirectoryEncoder`, потому что тайлов энкодер не пишет. Копии строятся тем
//! же `resample::halve`, которым каскад строит уровни, — на этом стоит
//! равенство прямого чтения и прохода. Файлы в несколько окон читателя
//! (`WINDOW`), и утверждения здесь — про окна, которые попросили у хоста
//! (`veldsdk::fake::reads`), а не про байты.

use std::cell::Cell;
use std::io::Cursor;
use std::rc::Rc;

use ::tiff::encoder::{colortype, TiffEncoder};
use ::tiff::tags::Tag;
use veldsdk::fake;

use super::super::pyramid::{self, TILE};
use super::super::resample::halve;
use super::codec::fixture::{gray_j2k, tiled_j2k};
use super::grid::Overview;
use super::tiff::{self, Layout};
use super::table::Serve;

/// Байт на пиксель фикстур: все они RGB8.
const RGB8: u32 = 3;
use super::{describe, produce, Info, Kind, Metered};
use crate::proto::image_tiler::Reach;

/// Окно читателя SDK — то, чем меряются чтения.
const WINDOW: u64 = 256 * 1024;

/// Узор без периода в тайл: копии тогда не сходятся случайно.
fn rgb_pattern(width: u32, height: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity((width * height * 3) as usize);
    for y in 0..height {
        for x in 0..width {
            out.push((x * 7 + y * 3) as u8);
            out.push((x ^ y) as u8);
            out.push((x * y / 97) as u8);
        }
    }
    out
}

fn rgb_to_rgba(rgb: &[u8]) -> Vec<u8> {
    rgb.chunks(3).flat_map(|px| [px[0], px[1], px[2], 255]).collect()
}

fn rgba_to_rgb(rgba: &[u8]) -> Vec<u8> {
    rgba.chunks(4).flat_map(|px| [px[0], px[1], px[2]]).collect()
}

/// Байты одного тайла TIFF: полный тайл, края добиты нулями.
fn tile_bytes(rgb: &[u8], width: u32, height: u32, tx: u32, ty: u32) -> Vec<u8> {
    let mut out = vec![0u8; (TILE * TILE * 3) as usize];
    for row in 0..TILE {
        let y = ty * TILE + row;
        if y >= height {
            break;
        }
        let x0 = tx * TILE;
        let x1 = (x0 + TILE).min(width);
        let from = ((y * width + x0) * 3) as usize;
        let len = ((x1 - x0) * 3) as usize;
        let to = (row * TILE * 3) as usize;
        out[to..to + len].copy_from_slice(&rgb[from..from + len]);
    }
    out
}

/// Тайловый TIFF без сжатия с `levels` копиями-половинами: каждая копия —
/// следующий IFD с битом reduced, как их пишет GDAL. Раскладка в файле при
/// этом не GDAL-овская: энкодер крейта кладёт каталог и массивы смещений
/// после данных уровня, а COG держит все IFD в голове файла. Возвращает файл,
/// диапазоны тайлов базового IFD и смещения самих IFD.
pub fn tiled_cog(width: u32, height: u32, levels: u32) -> (Vec<u8>, Vec<(u64, u64)>, Vec<u64>) {
    let mut file = Cursor::new(Vec::new());
    let mut encoder = TiffEncoder::new(&mut file).unwrap();
    let mut base_tiles = Vec::new();
    let mut ifds = Vec::new();
    let mut rgb = rgb_pattern(width, height);
    let (mut w, mut h) = (width, height);
    for level in 0..=levels {
        let mut dir = encoder.image_directory().unwrap();
        if level > 0 {
            dir.write_tag(Tag::NewSubfileType, 1u32).unwrap();
        }
        dir.write_tag(Tag::ImageWidth, w).unwrap();
        dir.write_tag(Tag::ImageLength, h).unwrap();
        dir.write_tag(Tag::BitsPerSample, &[8u16, 8, 8][..]).unwrap();
        dir.write_tag(Tag::Compression, 1u16).unwrap();
        dir.write_tag(Tag::PhotometricInterpretation, 2u16).unwrap();
        dir.write_tag(Tag::SamplesPerPixel, 3u16).unwrap();
        dir.write_tag(Tag::PlanarConfiguration, 1u16).unwrap();
        dir.write_tag(Tag::TileWidth, TILE).unwrap();
        dir.write_tag(Tag::TileLength, TILE).unwrap();
        let (across, down) = (w.div_ceil(TILE), h.div_ceil(TILE));
        let mut offsets = Vec::new();
        let mut counts = Vec::new();
        for ty in 0..down {
            for tx in 0..across {
                let tile = tile_bytes(&rgb, w, h, tx, ty);
                let at = dir.write_data(&tile[..]).unwrap();
                offsets.push(at as u32);
                counts.push(tile.len() as u32);
                if level == 0 {
                    base_tiles.push((at, tile.len() as u64));
                }
            }
        }
        dir.write_tag(Tag::TileOffsets, &offsets[..]).unwrap();
        dir.write_tag(Tag::TileByteCounts, &counts[..]).unwrap();
        ifds.push(u64::from(dir.finish_with_offsets().unwrap().offset));

        rgb = rgba_to_rgb(&halve(&rgb_to_rgba(&rgb), w, h));
        w = pyramid::level_size(w, 1);
        h = pyramid::level_size(h, 1);
    }
    drop(encoder);
    (file.into_inner(), base_tiles, ifds)
}

/// Полосный TIFF без копий — `ImageEncoder` крейта, полосы по `rows` строк.
pub fn stripped(width: u32, height: u32, rows: u32) -> Vec<u8> {
    let mut file = Cursor::new(Vec::new());
    let mut encoder = TiffEncoder::new(&mut file).unwrap();
    let mut image = encoder.new_image::<colortype::RGB8>(width, height).unwrap();
    image.rows_per_strip(rows).unwrap();
    image.write_data(&rgb_pattern(width, height)).unwrap();
    drop(encoder);
    file.into_inner()
}

/// Окна, задетые чтением: (смещение, размер), как их просили у хоста.
fn windows() -> Vec<(u64, u64)> {
    fake::reads().iter().map(|r| (r.offset, r.size)).collect()
}

fn overlaps(window: (u64, u64), range: (u64, u64)) -> bool {
    window.0 < range.0 + range.1 && range.0 < window.0 + window.1
}

/// Сколько байт окна лежит в диапазоне.
fn overlap_bytes(window: (u64, u64), range: (u64, u64)) -> u64 {
    let from = window.0.max(range.0);
    let to = (window.0 + window.1).min(range.0 + range.1);
    to.saturating_sub(from)
}

/// Окно задевает шапку файла или окрестность каталога: то, что читает разбор
/// заголовков, а не пиксели. Окрестность — окно в обе стороны: читатель
/// начинает окно с места чтения, а декодер прыгает по каталогу и назад.
fn near_head_or_ifd(window: (u64, u64), ifds: &[u64]) -> bool {
    overlaps(window, (0, WINDOW))
        || ifds.iter().any(|&at| overlaps(window, (at.saturating_sub(WINDOW), 2 * WINDOW)))
}

/// Описать смонтированный файл.
fn described(handle: &veldsdk::ResourceHandle) -> Info {
    describe(handle.id, handle.size, &Rc::new(Cell::new(0)), true).expect("описывается")
}

/// Произвести тайлы уровня: адрес → RGBA.
fn produced(
    handle: &veldsdk::ResourceHandle,
    info: &Info,
    level: u32,
    wants: &[(u32, u32)],
) -> Vec<((u32, u32, u32), Vec<u8>)> {
    let mut tiles = Vec::new();
    let mut emit = |lvl: u32, tx: u32, ty: u32, _w: u32, _h: u32, rgba: &[u8]| {
        tiles.push(((lvl, tx, ty), rgba.to_vec()));
        Ok(())
    };
    produce(handle.id, handle.size, info, level, wants, &Rc::new(Cell::new(0)), &mut emit).expect("производится");
    tiles
}

/// Описание не читает пикселей базового уровня, сколько бы мегабайт их ни
/// было: только шапку файла и окрестности каталогов. Что в эти окна всё же
/// попадает — начало первого тайла (он лежит сразу за шапкой), хвост
/// последнего (за ним каталог) и пиксели копий, лежащие вплотную к своим
/// каталогам, — свойство отличить не может и меряет долей: базовых байт
/// прочитано много меньше, чем их есть. Окон на каталог по нескольку:
/// декодер прыгает по нему назад, а окно читателя начинается с места чтения
/// и назад не смотрит; у настоящего COG каталог в голове файла, и цена там
/// другая.
#[test]
fn describing_reads_the_header_and_the_ifds_only() {
    fake::install();
    let levels = 2;
    let (file, base_tiles, ifds) = tiled_cog(2 * TILE, 2 * TILE, levels);
    assert!(file.len() as u64 > 8 * WINDOW, "фикстура обязана быть в несколько окон");
    let handle = fake::mount(file);

    let info = described(&handle);
    assert!(matches!(&info.kind, Kind::Tiff(layout) if layout.grid.overviews.len() == levels as usize));
    let asked = windows();
    let strangers: Vec<(u64, u64)> = asked.iter().copied().filter(|w| !near_head_or_ifd(*w, &ifds)).collect();
    assert!(strangers.is_empty(), "описание читало вдали от каталогов: {:?} (каталоги в {:?})", strangers, ifds);
    let over_base: u64 = asked
        .iter()
        .map(|w| base_tiles.iter().map(|t| overlap_bytes(*w, *t)).sum::<u64>())
        .sum();
    let base_total: u64 = base_tiles.iter().map(|t| t.1).sum();
    assert!(over_base * 4 < base_total, "описание прочло {} байт базовых тайлов из {}", over_base, base_total);
    assert!(
        asked.len() <= 1 + 3 * ifds.len(),
        "описание попросило {} окон при {} каталогах: {:?}", asked.len(), ifds.len(), asked
    );
}

/// Один тайл нулевого уровня стои́т чтения своего чанка и каталога: окна
/// прямого чтения лежат в диапазоне этого тайла в файле либо у каталога, а
/// по соседним тайлам не размазаны.
#[test]
fn one_tile_reads_only_its_own_chunk() {
    fake::install();
    let (file, base_tiles, ifds) = tiled_cog(2 * TILE, 2 * TILE, 1);
    let handle = fake::mount(file);
    let info = described(&handle);
    let head = fake::reads().len();

    let tiles = produced(&handle, &info, 0, &[(1, 1)]);
    assert_eq!(tiles.len(), 1);
    assert_eq!(tiles[0].0, (0, 1, 1));

    let own = base_tiles[3];
    let asked = &windows()[head..];
    assert!(asked.iter().any(|w| overlaps(*w, own)), "свой тайл не читался: {:?}", asked);
    let strangers: Vec<(u64, u64)> = asked
        .iter()
        .copied()
        .filter(|w| !overlaps(*w, own) && !near_head_or_ifd(*w, &ifds))
        .collect();
    assert!(
        strangers.is_empty(),
        "ради тайла (1,1) прочитано чужого: {:?} (свой диапазон {:?})", strangers, own
    );
}

/// Проход одним чтением: сам проход зовётся напрямую — файл в несколько
/// окон слишком мал, чтобы `produce` выбрал его сам, окно у него есть на всех
/// уровнях. Тайлы уезжают все, а окна пикселей не просятся дважды; повторно
/// читается только каталог, к которому декодер возвращается.
#[test]
fn a_pass_reads_the_file_once() {
    fake::install();
    let file = stripped(2 * TILE, 2 * TILE, 64);
    let handle = fake::mount(file);
    let info = described(&handle);
    let Kind::Tiff(layout) = &info.kind else { panic!("не TIFF") };
    let head = fake::reads().len();

    let mut tiles = Vec::new();
    let mut emit = |lvl: u32, tx: u32, ty: u32, _w: u32, _h: u32, _rgba: &[u8]| {
        tiles.push((lvl, tx, ty));
        Ok(())
    };
    let reader = Metered::new(handle.id, handle.size, Rc::new(Cell::new(0)));
    tiff::produce_pass(reader, &info, layout, &mut emit).expect("проход идёт");
    assert_eq!(tiles.len(), 4 + 1, "оба уровня пирамиды: {:?}", tiles);

    let asked = &windows()[head..];
    let mut seen = std::collections::HashSet::new();
    let repeated: Vec<(u64, u64)> = asked.iter().copied().filter(|w| !seen.insert(*w)).collect();
    assert!(repeated.iter().all(|w| near_head_or_ifd(*w, &[])) || repeated.is_empty(),
        "проход перечитал пиксели: {:?}", repeated);
    let bytes: u64 = asked.iter().map(|w| w.1).sum();
    assert!(bytes <= handle.size + 4 * WINDOW, "проход прочёл {} байт из {}", bytes, handle.size);
}

/// Прямое чтение копии и проход с ужатием сходятся на копиях ровно вдвое:
/// геометрия драйвера одна, откуда бы уровень ни взялся. Это тест геометрии,
/// не инвариант формата — копии GDAL так не сойдутся.
#[test]
fn direct_equals_pass_on_exact_halves() {
    fake::install();
    let (file, _, _) = tiled_cog(2 * TILE, 2 * TILE, 1);
    let handle = fake::mount(file);
    let info = described(&handle);
    assert_eq!(info.reach(), Reach::Exact);
    let direct = produced(&handle, &info, 1, &[(0, 0)]);
    assert_eq!(direct.len(), 1);
    assert_eq!(direct[0].0, (1, 0, 0));

    // Тот же файл проходом от нулевого уровня: копию он не смотрит.
    let Kind::Tiff(layout) = &info.kind else { panic!("не TIFF") };
    let mut pass = Vec::new();
    let mut emit = |lvl: u32, tx: u32, ty: u32, _w: u32, _h: u32, rgba: &[u8]| {
        if (lvl, tx, ty) == (1, 0, 0) {
            pass.push(rgba.to_vec());
        }
        Ok(())
    };
    let reader = Metered::new(handle.id, handle.size, Rc::new(Cell::new(0)));
    tiff::produce_pass(reader, &info, layout, &mut emit).expect("проход идёт");
    assert_eq!(pass.len(), 1);
    assert_eq!(direct[0].1, pass[0], "копия из файла и ужатие каскада разошлись");
}

/// `Info::reach()` и рукав `produce` спрашивают одно и то же — и обязаны
/// отвечать согласно на всякой раскладке и всяком уровне. Обе стороны читают
/// таблицу уровней, а та — `Grid::pointwise`, так что держит это правило
/// вывода `reach()`, и оно проверяется здесь против самого окна: `Exact` при
/// окне на всех уровнях, иначе `Windowed` с окном ровно на нижних. Оборванная
/// цепочка — 32·TILE с одной копией: верхний уровень читался бы из неё
/// областью больше `REGION_CAP`, и окно кончается раньше уровней.
#[test]
fn reach_and_the_produce_branch_agree() {
    let tiles = (TILE, TILE);
    let overviews = |count: usize, (mut w, mut h): (u32, u32)| -> Vec<Overview> {
        (1..=count).map(|image| {
            w = pyramid::level_size(w, 1);
            h = pyramid::level_size(h, 1);
            Overview { image, width: w, height: h, chunk: tiles }
        }).collect()
    };
    let layouts = [
        ("тайловый с полной цепочкой копий", (4 * TILE, 4 * TILE), true, 2usize, tiles),
        ("тайловый без копий", (4 * TILE, 4 * TILE), true, 0, tiles),
        ("полосный", (4 * TILE, 4 * TILE), false, 0, (4 * TILE, 64)),
        ("тайловый с оборванной цепочкой", (32 * TILE, 32 * TILE), true, 1, tiles),
    ];
    let mut partial_seen = false;
    for (name, (w, h), tiled, count, chunk) in layouts {
        let layout = Layout::of(tiled, chunk, overviews(count, (w, h)), RGB8);
        let info = Info::plain(w, h, Kind::Tiff(layout));
        let levels = pyramid::level_count(w, h);
        let Kind::Tiff(layout) = &info.kind else { unreachable!() };
        let branch: Vec<bool> = (0..levels).map(|level| layout.grid.pointwise((w, h), level)).collect();
        let served: Vec<bool> = (0..levels)
            .map(|level| info.level(level).expect("уровень есть").serve == Serve::Pointwise)
            .collect();
        assert_eq!(served, branch, "{name}: таблица уровней разошлась с правилом окна");
        match info.reach() {
            Reach::Exact => assert!(branch.iter().all(|&b| b), "{name}: Exact, но рукав прохода на {branch:?}"),
            Reach::Windowed => {
                let pointwise = info.windowed() as usize;
                assert!(branch[..pointwise].iter().all(|&b| b), "{name}: окно обещано, а рукав — проход: {branch:?}");
                assert!(pointwise == branch.len() || !branch[pointwise], "{name}: за концом окна рукав всё ещё окно: {branch:?}");
                partial_seen |= pointwise > 0 && pointwise < branch.len();
            }
            other => panic!("{name}: у TIFF не бывает {other:?}"),
        }
    }
    assert!(partial_seen, "ни одна раскладка не дала окно на части уровней — таблица проверяет не всё");
}

// ── JPEG 2000 ──────────────────────────────────────────────────

/// Шум, который не сжимается: файл выходит ростом с растр, и окна чтения одного
/// тайла отличимы от чтения всего файла.
fn noise(width: u32, height: u32) -> Vec<u8> {
    let mut state = 0x9E37_79B9u32;
    (0..width * height * 3)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            (state >> 24) as u8
        })
        .collect()
}

/// Тайл-парты сырого кодстрима по маркерам SOT: (смещение, длина Psot).
fn tile_parts(j2k: &[u8]) -> Vec<(u64, u64)> {
    let mut parts = Vec::new();
    let mut at = 2usize;
    while at + 4 <= j2k.len() && j2k[at] == 0xFF {
        match j2k[at + 1] {
            0x90 => {
                let psot = u32::from_be_bytes(j2k[at + 6..at + 10].try_into().unwrap()) as usize;
                let len = if psot == 0 { j2k.len() - at } else { psot };
                parts.push((at as u64, len as u64));
                at += len;
            }
            0xD9 => break,
            _ => at += 2 + usize::from(u16::from_be_bytes([j2k[at + 2], j2k[at + 3]])),
        }
    }
    parts
}

/// Описание JPEG 2000 читает только голову файла: сетку тайлов и разрешения
/// даёт главный заголовок, кодек не запускается.
#[test]
fn describing_a_jp2_reads_the_head_only() {
    fake::install();
    let (w, h) = (2 * TILE, 2 * TILE);
    let file = tiled_j2k(w, h, TILE, 3, &noise(w, h));
    assert!(file.len() as u64 > 8 * WINDOW, "фикстура обязана быть в несколько окон");
    let handle = fake::mount(file);

    let info = described(&handle);
    let Kind::Jp2(layout) = &info.kind else { panic!("не JP2") };
    assert_eq!(layout.grid.chunk, (TILE, TILE));
    assert_eq!(layout.grid.overviews.len(), 2, "две копии при трёх разрешениях");
    let asked = windows();
    assert!(asked.iter().all(|win| overlaps(*win, (0, WINDOW))), "описание ушло за голову: {:?}", asked);
}

/// Тайл нулевого уровня стои́т своего тайл-парта и заголовков вокруг него,
/// не чужих данных. Цена названа точно: до своего тайла кодек читает
/// заголовки SOT предыдущих (по окну читателя на каждый), а на первом тайле
/// кодека — ещё и всех последующих до конца кодстрима: так OpenJPEG сверяет
/// число тайл-партов (issue 254), и делает это один раз на кодек. Второй тайл
/// того же заказа идёт от последнего прочитанного SOT и ничего лишнего не
/// читает. Сам тайл приезжает без потерь.
#[test]
fn a_jp2_tile_reads_its_tile_part_and_only_headers_beyond() {
    fake::install();
    let (w, h) = (4 * TILE, 4 * TILE);
    let rgb = noise(w, h);
    let file = tiled_j2k(w, h, TILE, 3, &rgb);
    let parts = tile_parts(&file);
    assert_eq!(parts.len(), 16, "по тайл-парту на тайл");
    let len = file.len() as u64;
    let handle = fake::mount(file);
    let info = described(&handle);
    let head = fake::reads().len();

    let tiles = produced(&handle, &info, 0, &[(1, 1), (2, 1)]);
    assert_eq!(tiles.len(), 2);
    assert_eq!(tiles[0].0, (0, 1, 1));
    assert_eq!(tiles[0].1, rgb_to_rgba(&tile_bytes(&rgb, w, h, 1, 1)), "обратимый тайл разошёлся с исходником");
    assert_eq!(tiles[1].1, rgb_to_rgba(&tile_bytes(&rgb, w, h, 2, 1)));

    // Данные читались только у своих двух тайл-партов (5 и 6); окна, лежащие
    // за ними, начинаются ровно на SOT чужого тайл-парта либо на EOC — это
    // заголовки, и их не больше, чем тайл-партов за своими.
    let asked = &windows()[head..];
    let own = (parts[5].0, parts[6].0 + parts[6].1 - parts[5].0);
    assert!(asked.iter().any(|win| overlaps(*win, own)), "свои тайл-парты не читались: {:?}", asked);
    let beyond: Vec<(u64, u64)> = asked.iter().copied().filter(|win| win.0 >= own.0 + own.1).collect();
    let headers_only = beyond
        .iter()
        .all(|win| parts.iter().any(|part| part.0 == win.0) || win.0 == len - 2);
    assert!(headers_only, "за своими тайл-партами читались данные: {:?}", beyond);
    assert!(beyond.len() <= parts.len() - 7 + 1, "чужих заголовков прочитано больше, чем есть: {:?}", beyond);
}

/// Вырожденная сетка (тайл 8×8, как у квиклука PVI) идёт проходом, и проход
/// отдаёт растр без потерь.
#[test]
fn a_degenerate_jp2_grid_goes_by_a_lossless_pass() {
    fake::install();
    let (w, h) = (96u32, 64u32);
    let rgb = noise(w, h);
    let handle = fake::mount(tiled_j2k(w, h, 8, 2, &rgb));
    let info = described(&handle);
    assert!(info.levels().iter().all(|row| row.serve == Serve::Pass { from: 0 }), "сетка 8×8 вырождена");

    let tiles = produced(&handle, &info, 0, &[(0, 0)]);
    assert_eq!(tiles.len(), 1, "один уровень — один тайл");
    assert_eq!(tiles[0].1, rgb_to_rgba(&rgb));
}

/// Полоса шире байта (12 бит, как у Sentinel-2 кроме TCI) с копиями читается
/// через растяг файла: выборка берётся по сетке тайлов кодстрима, а не по
/// уменьшенному чанку, и уровень из копии приезжает с яркостью, а не пустым.
#[test]
fn a_twelve_bit_band_with_overviews_is_stretched() {
    fake::install();
    // Ширина в четыре тайла: на уровне 1 их два, и градиент виден между ними.
    let (w, h, tile) = (4 * TILE, 2 * TILE, 256u32);
    let samples: Vec<u16> = (0..w * h).map(|at| ((at % w) * 4095 / (w - 1)) as u16).collect();
    let handle = fake::mount(gray_j2k(w, h, tile, 3, 12, &samples));
    let info = described(&handle);
    let Kind::Jp2(layout) = &info.kind else { panic!("не JP2") };
    assert_eq!(layout.grid.overviews.len(), 2);

    // Уровень 1 — из копии на факторе 1: и растяг, и тайлы на уменьшенной сетке.
    let tiles = produced(&handle, &info, 1, &[(0, 0), (1, 0)]);
    assert_eq!(tiles.len(), 2);
    let (left, right) = (&tiles[0].1, &tiles[1].1);
    assert!(left.iter().step_by(4).any(|v| *v > 0), "левый тайл пуст");
    let mean = |rgba: &Vec<u8>| rgba.iter().step_by(4).map(|v| u64::from(*v)).sum::<u64>() / (rgba.len() as u64 / 4);
    assert!(mean(right) > mean(left), "градиент растёт слева направо: {} против {}", mean(left), mean(right));
    assert!(left.iter().skip(3).step_by(4).all(|a| *a == 255), "без ключа nodata прозрачных нет");
}
