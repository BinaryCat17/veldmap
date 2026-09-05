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
use super::codec::fixture::{addressed_j2k, gray_j2k, tiled_j2k, Addressing};
use super::excerpt::PROBE;
use super::grid::Overview;
use super::tiff::{self, Layout};
use super::table::Serve;

/// Байт на пиксель фикстур: все они RGB8.
const RGB8: u32 = 3;
use super::{describe, produce, Info, Kind, Metered};

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
    describe(handle.id, handle.size, &Rc::new(Cell::new(0))).expect("описывается")
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
    tiff::produce_pass(reader, handle.id, &info, layout, &mut emit).expect("проход идёт");
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
    assert!(info.levels().iter().all(|row| row.serve == Serve::Pointwise));
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
    tiff::produce_pass(reader, handle.id, &info, layout, &mut emit).expect("проход идёт");
    assert_eq!(pass.len(), 1);
    assert_eq!(direct[0].1, pass[0], "копия из файла и ужатие каскада разошлись");
}

/// Таблица уровней, которая уезжает на провод, и рукав `produce` спрашивают
/// одно и то же — и обязаны отвечать согласно на всякой раскладке и всяком
/// уровне. Обе стороны читают одну таблицу, а та — `Grid::pointwise`, и здесь
/// столбец обслуживания проверяется против самого окна: точечное начало ровно
/// там, где окно, и проход с нулевого за ним. Оборванная цепочка — 32·TILE с
/// одной копией: верхний уровень читался бы из неё областью больше
/// `REGION_CAP`, и окно кончается раньше уровней.
#[test]
fn the_level_table_and_the_produce_branch_agree() {
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
        let pointwise = info.windowed() as usize;
        assert!(branch[..pointwise].iter().all(|&b| b), "{name}: окно обещано, а рукав — проход: {branch:?}");
        assert!(pointwise == branch.len() || !branch[pointwise], "{name}: за концом окна рукав всё ещё окно: {branch:?}");
        assert!(
            info.levels()[pointwise..].iter().all(|row| row.serve == Serve::Pass { from: 0 }),
            "{name}: за окном у TIFF только проход с нулевого"
        );
        partial_seen |= pointwise > 0 && pointwise < branch.len();
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

/// Кодстрим с индексом: TLM в главном заголовке, PLT в каждом тайл-парте —
/// как пишет гранулы Kakadu.
const INDEXED: Addressing = Addressing { tlm: true, plt: true, precinct: None, container: false };

/// Байт, прочитанных у хоста с момента `from` в журнале.
fn read_since(from: usize) -> u64 {
    windows()[from..].iter().map(|win| win.1).sum()
}

/// С индексом тайл нулевого уровня стои́т ровно своего тайл-парта: пробы с
/// его начала и окон внутри него, ни одного окна вне своих — ни заголовков
/// предыдущих, ни обхода последующих. Пиксели те же, что без индекса.
#[test]
fn an_indexed_jp2_tile_reads_its_own_part_and_nothing_beyond() {
    fake::install();
    let (w, h) = (4 * TILE, 4 * TILE);
    let rgb = noise(w, h);
    let file = addressed_j2k(w, h, TILE, 3, INDEXED, &rgb);
    let parts = tile_parts(&file);
    assert_eq!(parts.len(), 16, "по тайл-парту на тайл");
    let handle = fake::mount(file);
    let info = described(&handle);
    let Kind::Jp2(layout) = &info.kind else { panic!("не JP2") };
    assert!(layout.indexed(), "TLM фикстуры прочитан в индекс");
    let head = fake::reads().len();

    let tiles = produced(&handle, &info, 0, &[(1, 1), (2, 1)]);
    assert_eq!(tiles[0].1, rgb_to_rgba(&tile_bytes(&rgb, w, h, 1, 1)), "обратимый тайл разошёлся с исходником");
    assert_eq!(tiles[1].1, rgb_to_rgba(&tile_bytes(&rgb, w, h, 2, 1)));

    let asked = &windows()[head..];
    let own = |win: &(u64, u64)| [parts[5], parts[6]].iter().any(|part| win.0 >= part.0 && win.0 + win.1 <= part.0 + part.1);
    assert!(asked.iter().all(own), "читалось вне своих тайл-партов: {:?}", asked);
    assert_eq!(asked[0], (parts[5].0, PROBE.min(parts[5].1)), "первое чтение — проба с начала тайл-парта");
    assert!(asked.iter().any(|win| win.0 >= parts[6].0), "второй тайл читался");
}

/// Самый грубый уровень с индексом стои́т по пробе на тайл: пакеты грубых
/// разрешений лежат в начале тайл-парта, и выдержка режет его там, где
/// кончается нужное разрешение. Уровень при этом тот же, что у чтения без
/// индекса, — обрезанный тайл-парт декодируется в те же пиксели.
#[test]
fn the_coarsest_level_of_an_indexed_jp2_costs_a_probe_per_tile() {
    fake::install();
    let (w, h) = (4 * TILE, 4 * TILE);
    let rgb = noise(w, h);
    let plain = fake::mount(tiled_j2k(w, h, TILE, 3, &rgb));
    let expected = produced(&plain, &described(&plain), 2, &[(0, 0)]);

    let file = addressed_j2k(w, h, TILE, 3, INDEXED, &rgb);
    let len = file.len() as u64;
    let handle = fake::mount(file);
    let info = described(&handle);
    assert_eq!(info.levels().len(), 3, "2048² — три уровня");
    let head = fake::reads().len();

    let tiles = produced(&handle, &info, 2, &[(0, 0)]);
    assert_eq!(tiles, expected, "выдержка декодируется в те же пиксели, что весь файл");
    let asked = &windows()[head..];
    assert_eq!(asked.len(), 16, "по одному чтению на тайл: {:?}", asked);
    assert!(asked.iter().all(|win| win.1 <= PROBE));
    assert!(read_since(head) * 8 < len, "прочитано {} из {} — не префикс", read_since(head), len);
}

/// Свои прецинкты, как у гранулы: пакетов на разрешение больше одного на
/// компонент, и счёт их обязан сойтись с PLT энкодера — иначе выдержка
/// молча взяла бы тайл-парт целиком, и это видно по прочитанному.
#[test]
fn precincts_are_counted_as_the_encoder_writes_them() {
    fake::install();
    let (w, h) = (2 * TILE, 2 * TILE);
    let rgb = noise(w, h);
    let plain = fake::mount(tiled_j2k(w, h, TILE, 3, &rgb));
    let info = described(&plain);
    let expected: Vec<_> = (0..info.levels().len() as u32).map(|level| produced(&plain, &info, level, &[(0, 0)])).collect();

    let file = addressed_j2k(w, h, TILE, 3, Addressing { precinct: Some(64), ..INDEXED }, &rgb);
    let len = file.len() as u64;
    let handle = fake::mount(file);
    let info = described(&handle);
    for level in 1..info.levels().len() as u32 {
        let head = fake::reads().len();
        assert_eq!(produced(&handle, &info, level, &[(0, 0)]), expected[level as usize], "уровень {}", level);
        assert!(read_since(head) * 2 < len, "уровень {}: прочитано {} из {}", level, read_since(head), len);
    }
    assert_eq!(produced(&handle, &info, 0, &[(0, 0)]), expected[0]);
}

/// Контейнер JP2 — коробки до `jp2c` идут в выдержку вместе с главным
/// заголовком, и кодек контейнера принимает её так же, как кодек кодстрима.
#[test]
fn a_jp2_container_is_excerpted_with_its_boxes() {
    fake::install();
    let (w, h) = (2 * TILE, 2 * TILE);
    let rgb = noise(w, h);
    let file = addressed_j2k(w, h, TILE, 3, Addressing { container: true, ..INDEXED }, &rgb);
    assert!(file.starts_with(super::jp2::JP2_MAGIC), "энкодер написал контейнер");
    let handle = fake::mount(file);
    let info = described(&handle);
    let Kind::Jp2(layout) = &info.kind else { panic!("не JP2") };
    assert!(layout.indexed(), "индекс читается и из контейнера");

    let tiles = produced(&handle, &info, 0, &[(1, 1)]);
    assert_eq!(tiles[0].1, rgb_to_rgba(&tile_bytes(&rgb, w, h, 1, 1)));
    let plain = fake::mount(tiled_j2k(w, h, TILE, 3, &rgb));
    assert_eq!(produced(&handle, &info, 1, &[(0, 0)]), produced(&plain, &described(&plain), 1, &[(0, 0)]));
}

/// TLM без PLT — второй исход: тайл находится по индексу и читается целым
/// тайл-партом на любом уровне, потому что резать его не по чему.
#[test]
fn tlm_without_plt_reads_whole_tile_parts() {
    fake::install();
    let (w, h) = (2 * TILE, 2 * TILE);
    let rgb = noise(w, h);
    let file = addressed_j2k(w, h, TILE, 3, Addressing { plt: false, ..INDEXED }, &rgb);
    let parts = tile_parts(&file);
    let handle = fake::mount(file);
    let info = described(&handle);
    let Kind::Jp2(layout) = &info.kind else { panic!("не JP2") };
    assert!(layout.indexed());
    let head = fake::reads().len();

    let plain = fake::mount(tiled_j2k(w, h, TILE, 3, &rgb));
    assert_eq!(produced(&handle, &info, 1, &[(0, 0)]), produced(&plain, &described(&plain), 1, &[(0, 0)]));
    let own: u64 = parts.iter().map(|part| part.1).sum();
    assert!(read_since(head) >= own, "тайл-парты целиком: прочитано {} при {} в них", read_since(head), own);
}


// ── NetCDF ─────────────────────────────────────────────────────

use hdf5_pure::{AttrValue, FileBuilder};

use super::netcdf;

/// Метка «нет данных» фикстур NetCDF.
const FILL: i16 = -9999;

/// Отсчёты величины `width`×`height`: перепад по обеим осям, левый столбец —
/// «нет данных», как у полосы съёмки с неизмеренным краем.
fn nc_pattern(width: u32, height: u32) -> Vec<i16> {
    (0..height)
        .flat_map(|y| {
            (0..width).map(move |x| match x {
                0 => FILL,
                _ => ((x * 7 + y * 3) % 2000) as i16,
            })
        })
        .collect()
}

/// Величина фикстуры: имя в группе `PRODUCT`, форма (строки и столбцы —
/// последние две неединичные оси), чанки с deflate либо непрерывная раскладка
/// и сами отсчёты.
struct Variable<'a> {
    name: &'a str,
    shape: &'a [u64],
    chunk: Option<&'a [u64]>,
    values: &'a [i16],
}

/// NetCDF-4 с величинами в группе `PRODUCT` — писателем того же крейта,
/// которым файл и читается.
fn netcdf_with(variables: &[Variable<'_>]) -> Vec<u8> {
    let mut file = FileBuilder::new();
    let mut product = file.create_group("PRODUCT");
    for variable in variables {
        let dataset = product.create_dataset(variable.name);
        dataset.with_i16_data(variable.values).with_shape(variable.shape);
        dataset.set_attr("_FillValue", AttrValue::I16(FILL));
        dataset.set_attr("units", AttrValue::String("K".to_string()));
        if let Some(chunk) = variable.chunk {
            dataset.with_chunks(chunk).with_deflate(1);
        }
    }
    file.add_group(product.finish());
    file.finish().expect("HDF5 пишется")
}

/// Одна величина `temperature`.
fn netcdf(shape: &[u64], chunk: Option<&[u64]>, values: &[i16]) -> Vec<u8> {
    netcdf_with(&[Variable { name: "temperature", shape, chunk, values }])
}

/// Где в файле лежат отсчёты величины: (строка чанка, смещение, длина) —
/// спрошено у читателя того же крейта. У непрерывной раскладки чанк один и
/// стои́т на нулевой строке.
fn nc_chunks(bytes: &[u8], name: &str) -> Vec<(u64, u64, u64)> {
    let file = hdf5_pure::File::from_bytes(bytes.to_vec()).expect("файл читается");
    // Величины фикстур лежат в `PRODUCT`; координаты — в корне, и зовутся с `../`.
    let path = match name.strip_prefix("../") {
        Some(root) => format!("/{root}"),
        None => format!("/PRODUCT/{name}"),
    };
    let dataset = file.dataset(&path).expect("величина на месте");
    match dataset.layout().expect("раскладка читается") {
        hdf5_pure::Layout::Chunked { chunk_shape, .. } => {
            // Строка чанка — по последней неединичной оси перед столбцами:
            // у формы [1, h, w] это вторая ось.
            let rows_axis = chunk_shape.len().saturating_sub(2);
            dataset
                .chunks()
                .expect("чанки перечислимы")
                .into_iter()
                .map(|chunk| (chunk.offset[rows_axis], chunk.address, chunk.storage_size))
                .collect()
        }
        hdf5_pure::Layout::Contiguous { address: Some(at), size } => vec![(0, at, size)],
        other => panic!("раскладка фикстуры: {other:?}"),
    }
}

/// Окна разбора величины — без окна шапки, которым общий `describe` узнаёт
/// формат по сигнатуре: оно одно на все форматы и у мелкой фикстуры накрывает
/// половину файла.
fn nc_windows() -> Vec<(u64, u64)> {
    windows().into_iter().filter(|win| *win != (0, WINDOW)).collect()
}

/// Сколько байт окон пришлось на отсчёты этих чанков.
fn read_of(windows: &[(u64, u64)], chunks: &[(u64, u64, u64)]) -> u64 {
    windows
        .iter()
        .map(|win| chunks.iter().map(|(_, at, len)| overlap_bytes(*win, (*at, *len))).sum::<u64>())
        .sum()
}

/// Сколько чанков задето хоть одним окном.
fn touched(windows: &[(u64, u64)], chunks: &[(u64, u64, u64)]) -> usize {
    chunks.iter().filter(|(_, at, len)| windows.iter().any(|win| overlaps(*win, (*at, *len)))).count()
}

/// Описание величины читает заголовки и до четырёх окон строк вразброс, а не
/// плоскость: у величины в 24 окна выборка — четыре из них по пять чанков,
/// то есть шестая часть отсчётов; окно — связка чанков по сто строк не длиннее
/// тайла.
#[test]
fn describing_a_netcdf_samples_four_row_windows() {
    fake::install();
    let (w, h) = (200u32, 12_000u32);
    let bytes = netcdf(&[u64::from(h), u64::from(w)], Some(&[100, u64::from(w)]), &nc_pattern(w, h));
    let chunks = nc_chunks(&bytes, "temperature");
    assert_eq!(chunks.len(), 120);
    let data: u64 = chunks.iter().map(|chunk| chunk.2).sum();
    let handle = fake::mount(bytes);

    let info = described(&handle);
    assert_eq!((info.width, info.height), (w, h));
    let Kind::Netcdf(layout) = &info.kind else { panic!("не NetCDF") };
    assert_eq!(layout.grid.chunk, (w, 500), "окно — связка чанков по сто строк не длиннее тайла");
    let asked = nc_windows();
    let read = read_of(&asked, &chunks);
    assert!(read > 0, "выборка не прочитала ни отсчёта");
    assert!(read * 5 <= data, "выборка прочитала {read} из {data} байт отсчётов — больше пятой части");
    assert_eq!(touched(&asked, &chunks), 4 * 5, "четыре окна по пять чанков");
    assert_eq!(info.level(0).expect("уровень есть").serve, Serve::Pointwise);
}

/// Тайл нулевого уровня читает окна строк под собой и ничего сверх: тайл
/// (0, 5) — строки 2560…3071, то есть окна 5 и 6 (строки 2500…3499), десять
/// чанков; чужих строк — ни байта. «Нет данных» — прозрачно.
#[test]
fn a_netcdf_tile_reads_its_own_rows_only() {
    fake::install();
    let (w, h) = (200u32, 12_000u32);
    let bytes = netcdf(&[u64::from(h), u64::from(w)], Some(&[100, u64::from(w)]), &nc_pattern(w, h));
    let chunks = nc_chunks(&bytes, "temperature");
    let handle = fake::mount(bytes);
    let info = described(&handle);
    let head = fake::reads().len();

    let tiles = produced(&handle, &info, 0, &[(0, 5)]);

    assert_eq!(tiles.len(), 1);
    let asked = &windows()[head..];
    let (own, foreign): (Vec<_>, Vec<_>) = chunks.iter().copied().partition(|(row, ..)| (2500..3500).contains(row));
    assert_eq!(read_of(asked, &foreign), 0, "прочитаны чужие строки");
    assert!(read_of(asked, &own) > 0);
    assert_eq!(touched(asked, &own), 10, "два окна по пять чанков");

    let rgba = &tiles[0].1;
    assert_eq!(rgba.len(), (w as usize) * (TILE as usize) * 4);
    assert_eq!(rgba[3], 0, "левый столбец — «нет данных» — прозрачен");
    assert_eq!(rgba[7], 255, "а соседний измерен");
}

/// Окно и проход сходятся на одной величине: тайлы нулевого уровня окном те
/// же, что отдаёт каскад прохода с нулевого.
#[test]
fn a_netcdf_window_equals_its_pass() {
    fake::install();
    let (w, h) = (200u32, 3_000u32);
    let bytes = netcdf(&[u64::from(h), u64::from(w)], Some(&[100, u64::from(w)]), &nc_pattern(w, h));
    let handle = fake::mount(bytes);
    let info = described(&handle);
    assert_eq!(info.level(0).expect("уровень есть").serve, Serve::Pointwise);
    let direct = produced(&handle, &info, 0, &[(0, 0), (0, 5)]);
    assert_eq!(direct.len(), 2);

    let Kind::Netcdf(layout) = &info.kind else { panic!("не NetCDF") };
    let mut pass = std::collections::BTreeMap::new();
    let mut emit = |lvl: u32, tx: u32, ty: u32, _w: u32, _h: u32, rgba: &[u8]| {
        if lvl == 0 {
            pass.insert((tx, ty), rgba.to_vec());
        }
        Ok(())
    };
    netcdf::produce_pass(handle.id, handle.size, &Rc::new(Cell::new(0)), &info, layout, &mut emit)
        .expect("проход идёт");
    assert_eq!(pass.len(), 6, "проход отдал все тайлы нулевого уровня");
    for ((_, tx, ty), rgba) in &direct {
        assert_eq!(Some(rgba), pass.get(&(*tx, *ty)), "тайл {tx}:{ty} окном и проходом разошёлся");
    }
}

/// Единичная нулевая ось (`[1, h, w]`, как время у Sentinel-5P) окну не
/// мешает: оно режется регионом по оси строк плоскости — связкой из пяти
/// чанков файла по сто строк, — описание читает свои окна, а тайл — свои,
/// и плоскость не читает никто.
#[test]
fn a_unit_leading_axis_windows_along_the_next_one() {
    fake::install();
    let (w, h) = (450u32, 6_000u32);
    let bytes = netcdf(&[1, u64::from(h), u64::from(w)], Some(&[1, 100, u64::from(w)]), &nc_pattern(w, h));
    let chunks = nc_chunks(&bytes, "temperature");
    assert_eq!(chunks.len(), 60);
    let handle = fake::mount(bytes);

    let info = described(&handle);
    assert_eq!((info.width, info.height), (w, h), "единичная ось отброшена");
    let Kind::Netcdf(layout) = &info.kind else { panic!("не NetCDF") };
    assert_eq!(layout.grid.chunk, (w, 500), "окно — пять чанков файла, не плоскость");
    let row = info.level(0).expect("уровень есть");
    assert_eq!(row.serve, Serve::Pointwise);
    // Таблица считает худший тайл: сдвинутый на границу окна, он задевает три.
    assert_eq!(row.pixels, u64::from(w) * 1500, "тайл стои́т до трёх окон, не плоскости");
    let asked = nc_windows();
    assert_eq!(touched(&asked, &chunks), 4 * 5, "выборка — четыре окна по пять чанков, не плоскость");

    let head = fake::reads().len();
    let tiles = produced(&handle, &info, 0, &[(0, 0)]);
    assert_eq!(tiles.len(), 1);
    let asked = &windows()[head..];
    assert_eq!(touched(asked, &chunks), 10, "тайл читает два своих окна — десять чанков из шестидесяти");
}

/// Файл координат полосы съёмки: широта и долгота плоскостями `rows`×`columns`
/// f32, чанк файла — `chunk_rows` строк во всю ширину.
fn coordinates(columns: u32, rows: u32, chunk_rows: u32) -> Vec<u8> {
    let mut file = FileBuilder::new();
    let cell = |at: usize, per: f32| (at as f32) * per;
    let lat: Vec<f32> = (0..rows * columns).map(|at| 40.0 + cell((at / columns) as usize, 0.01)).collect();
    let lon: Vec<f32> = (0..rows * columns).map(|at| 30.0 + cell((at % columns) as usize, 0.01)).collect();
    for (name, values, units) in [("latitude", &lat, "degrees_north"), ("longitude", &lon, "degrees_east")] {
        let dataset = file.create_dataset(name);
        dataset
            .with_f32_data(values)
            .with_shape(&[u64::from(rows), u64::from(columns)])
            .with_chunks(&[u64::from(chunk_rows), u64::from(columns)])
            .with_deflate(1);
        dataset.set_attr("units", AttrValue::String(units.to_string()));
    }
    file.finish().expect("HDF5 пишется")
}

/// Решётки координат читаются строками узлов, а не плоскостью: у сетки в
/// шестьсот строк узлов двадцать один ряд, и задеты ровно их чанки — у широты
/// и у долготы по двадцать одному из шестисот. Привязка при этом собирается
/// целиком, решётка в 21×21 узел.
#[test]
fn ties_read_the_rows_of_their_nodes_not_the_plane() {
    fake::install();
    let (w, h) = (300u32, 600u32);
    let bytes = coordinates(w, h, 1);
    let lat_chunks = nc_chunks(&bytes, "../latitude");
    let lon_chunks = nc_chunks(&bytes, "../longitude");
    assert_eq!((lat_chunks.len(), lon_chunks.len()), (600, 600));
    let handle = fake::mount(bytes);

    let ties = netcdf::geolocation(handle.id, handle.size, None, w, h).expect("привязка собирается");
    assert_eq!(ties.len(), 21 * 21, "решётка узлов");
    let asked = nc_windows();
    assert_eq!(touched(&asked, &lat_chunks), 21, "широта — только строки узлов");
    assert_eq!(touched(&asked, &lon_chunks), 21, "долгота — только строки узлов");
    // Углы решётки стоят на первой и последней строках и столбцах файла.
    let corner = ties.iter().find(|tie| tie.px < 1.0 && tie.py < 1.0).expect("угол");
    assert!((corner.lat - 40.0).abs() < 1e-3 && (corner.lon - 30.0).abs() < 1e-3);
    let far = ties.iter().find(|tie| tie.px > 299.0 && tie.py > 599.0).expect("дальний угол");
    assert!((far.lat - 45.99).abs() < 1e-2 && (far.lon - 32.99).abs() < 1e-2);
}

/// Чанк решётки координат распаковывается один раз, сколько бы строк узлов
/// на нём ни стояло: набор открыт на всю решётку, и его кэш держит строку
/// чанков. Чанк здесь крупнее кэша крейта по умолчанию (267 строк по 1500 —
/// 1,6 МБ, как у полукилометровой сетки SLSTR), так что кэш обязан быть
/// назначен под него; иначе каждая из 21 строки узлов читала бы свой чанк
/// снова.
#[test]
fn a_coordinate_chunk_is_inflated_once_however_many_node_rows_it_holds() {
    fake::install();
    let (w, h) = (1500u32, 600u32);
    let bytes = coordinates(w, h, 267);
    let lat_chunks = nc_chunks(&bytes, "../latitude");
    let lon_chunks = nc_chunks(&bytes, "../longitude");
    assert_eq!((lat_chunks.len(), lon_chunks.len()), (3, 3));
    let handle = fake::mount(bytes);

    let ties = netcdf::geolocation(handle.id, handle.size, None, w, h).expect("привязка собирается");
    assert_eq!(ties.len(), 21 * 24, "решётка узлов: 21 строка, 24 столбца");
    let asked = nc_windows();
    for chunks in [&lat_chunks, &lon_chunks] {
        let stored: u64 = chunks.iter().map(|chunk| chunk.2).sum();
        assert_eq!(touched(&asked, chunks), 3, "узлы стоят на всех трёх чанках");
        assert_eq!(read_of(&asked, chunks), stored, "каждый чанк прочитан ровно один раз");
    }
}

/// Величина одним чанком (SYNERGY): меньше плоскости не прочесть, и сетка
/// говорит то же, что у единичной оси.
#[test]
fn a_single_chunk_variable_costs_the_plane() {
    fake::install();
    let (w, h) = (400u32, 600u32);
    let bytes = netcdf(&[u64::from(h), u64::from(w)], Some(&[u64::from(h), u64::from(w)]), &nc_pattern(w, h));
    let chunks = nc_chunks(&bytes, "temperature");
    assert_eq!(chunks.len(), 1);
    let handle = fake::mount(bytes);

    let info = described(&handle);
    let Kind::Netcdf(layout) = &info.kind else { panic!("не NetCDF") };
    assert_eq!(layout.grid.chunk, (w, h));
    let row = info.level(0).expect("уровень есть");
    assert_eq!((row.serve, row.pixels), (Serve::Pointwise, u64::from(w) * u64::from(h)));
    assert!(read_of(&nc_windows(), &chunks) >= chunks[0].2);

    // Непрерывная раскладка окно режет тайлом — читать меньше чанка ей не
    // мешает ничто.
    fake::install();
    let plain = fake::mount(netcdf(&[u64::from(h), u64::from(w)], None, &nc_pattern(w, h)));
    let Kind::Netcdf(layout) = &described(&plain).kind else { panic!("не NetCDF") };
    assert_eq!(layout.grid.chunk, (w, TILE));
}

/// Однотонная величина — ответ последней очереди: измеренная соседка её
/// обходит, а без соседки она показывается, и выборка её читается один раз —
/// раскладка запоминается вместе с ней, перечитывать нечего.
#[test]
fn a_flat_variable_is_the_last_resort_and_is_read_once() {
    let (w, h) = (64u32, 128u32);
    let shape = [u64::from(h), u64::from(w)];
    let flat = vec![7i16; (w * h) as usize];

    fake::install();
    let paired = netcdf_with(&[
        Variable { name: "a_flat", shape: &shape, chunk: None, values: &flat },
        Variable { name: "b_measured", shape: &shape, chunk: None, values: &nc_pattern(w, h) },
    ]);
    let handle = fake::mount(paired);
    let Kind::Netcdf(layout) = &described(&handle).kind else { panic!("не NetCDF") };
    assert_eq!(layout.path(), "/PRODUCT/b_measured", "измеренная обходит однотонную");

    // Окон над отсчётами у одинокой однотонной столько же, сколько у одинокой
    // измеренной: одна выборка, без перечитывания в конце.
    let reads_over = |values: &[i16]| {
        fake::install();
        let file = netcdf_with(&[Variable { name: "alone", shape: &shape, chunk: None, values }]);
        let chunks = nc_chunks(&file, "alone");
        let handle = fake::mount(file);
        let Kind::Netcdf(layout) = &described(&handle).kind else { panic!("не NetCDF") };
        assert_eq!(layout.path(), "/PRODUCT/alone");
        nc_windows().iter().filter(|win| chunks.iter().any(|(_, at, len)| overlaps(**win, (*at, *len)))).count()
    };
    let measured = reads_over(&nc_pattern(w, h));
    assert!(measured > 0);
    assert_eq!(reads_over(&flat), measured, "однотонная прочитана столько же раз, сколько измеренная");
}

/// Величина, пустая в выборке, уступает следующей по порядку: показанная, она
/// дала бы прозрачный кадр без единого слова.
#[test]
fn an_empty_variable_yields_to_the_next() {
    fake::install();
    let (w, h) = (64u32, 128u32);
    let empty = vec![FILL; (w * h) as usize];
    let bytes = netcdf_with(&[
        Variable { name: "a_empty", shape: &[u64::from(h), u64::from(w)], chunk: None, values: &empty },
        Variable { name: "b_measured", shape: &[u64::from(h), u64::from(w)], chunk: None, values: &nc_pattern(w, h) },
    ]);
    let handle = fake::mount(bytes);

    let info = described(&handle);
    assert_eq!((info.width, info.height), (w, h));
    let Kind::Netcdf(layout) = &info.kind else { panic!("не NetCDF") };
    assert_eq!(layout.path(), "/PRODUCT/b_measured", "пустая первая по алфавиту уступила измеренной");
}

// ── Наперёд ────────────────────────────────────────────────────

/// Прямое чтение называет носителю чанки под заказанными тайлами наперёд —
/// ровно свои, по смещениям и длинам из каталога TIFF, одним заказом на проход,
/// — а описание не заказывает ничего.
#[test]
fn direct_reading_names_its_chunks_ahead() {
    fake::install();
    let (file, base_tiles, _) = tiled_cog(2 * TILE, 2 * TILE, 1);
    let handle = fake::mount(file);
    let info = described(&handle);
    assert!(fake::prefetches().is_empty(), "описание ничего не заказывает наперёд");

    produced(&handle, &info, 0, &[(1, 0), (0, 1)]);

    let asked = fake::prefetches();
    assert_eq!(asked.len(), 1, "один заказ на проход: {asked:?}");
    assert_eq!(asked[0].id, handle.id);
    let mut ranges = asked[0].ranges.clone();
    ranges.sort_unstable();
    let mut expected = vec![base_tiles[1], base_tiles[2]];
    expected.sort_unstable();
    assert_eq!(ranges, expected, "заказаны ровно тайлы (1,0) и (0,1) базового образа");
}

/// С индексом тайлы называют носителю наперёд пробы своих тайл-партов — одним
/// заказом на проход, — а за ними куски выдержек всех тайлов вторым заказом,
/// когда выдержки собраны по пробам: на подробном уровне это остаток
/// тайл-парта за пробой. По тайлу куски не заказываются — выдержка уже собрана.
#[test]
fn an_indexed_jp2_names_its_probes_and_pieces_ahead() {
    fake::install();
    let (w, h) = (4 * TILE, 4 * TILE);
    let rgb = noise(w, h);
    let file = addressed_j2k(w, h, TILE, 3, INDEXED, &rgb);
    let parts = tile_parts(&file);
    let handle = fake::mount(file);
    let info = described(&handle);
    assert!(fake::prefetches().is_empty());

    produced(&handle, &info, 0, &[(1, 1), (2, 1)]);

    let asked = fake::prefetches();
    assert_eq!(asked.len(), 2, "заказ проб и один заказ кусков на оба тайла: {asked:?}");
    assert!(asked.iter().all(|order| order.id == handle.id));
    let probes: Vec<(u64, u64)> = [5usize, 6].iter().map(|&at| (parts[at].0, PROBE.min(parts[at].1))).collect();
    assert_eq!(asked[0].ranges, probes, "первый заказ — пробы обоих тайлов");
    let pieces: Vec<(u64, u64)> = asked[1].ranges.clone();
    assert!(!pieces.is_empty(), "куски выдержки не заказаны");
    let in_part = |part: (u64, u64)| pieces.iter().any(|(at, len)| *at >= part.0 && at + len <= part.0 + part.1);
    assert!(in_part(parts[5]) && in_part(parts[6]), "куски обоих тайлов в одном заказе: {pieces:?}");
    let own = |(at, len): &(u64, u64)| [parts[5], parts[6]].iter().any(|part| *at >= part.0 && at + len <= part.0 + part.1);
    assert!(pieces.iter().all(own), "куски вне своих тайл-партов: {pieces:?}");
}
