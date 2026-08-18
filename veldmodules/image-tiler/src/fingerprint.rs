//! Отпечаток содержимого — идентичность источника для тайлового кэша.
//!
//! Выводится из самих байтов: длина, первые и последние 64 КБ. Поэтому у
//! файла на диске и удалённого объекта с теми же байтами отпечаток один —
//! кэш переживает переход «смотрел по сети → скачал», — а договора об
//! идентичности между модулями не существует: подделать нечего и не с чем
//! разойтись. Недокачанный `.part` честно даёт другой отпечаток: у него
//! другая длина.
//!
//! Голова и хвост выбраны не для стойкости, а по цене: они почти всегда и
//! так нужны декодеру (заголовки, IFD у TIFF живут с обеих сторон), и после
//! первого чтения лежат в блочном кэше network. Коллизия требует совпадения
//! длины, головы и хвоста при разной середине — для кэша превью этого
//! достаточно, и это осознанная граница.
//!
//! Суффикс кодирует раскладку и правила производства (`-t512q3`: сторона тайла
//! и ревизия декодирования): смена любого меняет ключ, и старые каталоги кэша
//! просто стареют до вытеснения — без версий и миграций.

use std::io::{Read, Seek, SeekFrom};

use super::pyramid::TILE;

/// Сколько байт берётся с каждого края.
const SAMPLE: u64 = 64 * 1024;

/// Ревизия правил декодирования. Поднимается правкой, меняющей содержимое
/// тайлов при тех же байтах источника — растяг широких форматов, ключевание
/// «нет данных», взвешивание ужатия, — иначе рядом с новыми тайлами из кэша
/// всплывали бы старые той же самой картинки.
const DECODE_REV: u32 = 3;

/// Отпечаток ресурса: `<fnv64 hex>-t<TILE>q<DECODE_REV>`.
pub fn fingerprint(resource_id: u64, len: u64) -> Result<String, String> {
    let mut reader = veldsdk::ResourceReader::new(resource_id, len);

    let mut hash = Fnv::new();
    hash.update(&len.to_le_bytes());

    let head = read_exact_at(&mut reader, 0, SAMPLE.min(len))?;
    hash.update(&head);
    // Хвост — только не перекрытый головой: у короткого файла второго куска
    // нет, а не «тот же кусок ещё раз».
    if len > SAMPLE {
        let from = (len - SAMPLE).max(SAMPLE);
        let tail = read_exact_at(&mut reader, from, len - from)?;
        hash.update(&tail);
    }

    Ok(format!("{:016x}-t{}q{}", hash.finish(), TILE, DECODE_REV))
}

fn read_exact_at(reader: &mut veldsdk::ResourceReader, from: u64, size: u64) -> Result<Vec<u8>, String> {
    reader.seek(SeekFrom::Start(from)).map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; size as usize];
    reader
        .read_exact(&mut buf)
        .map_err(|e| format!("отпечаток: чтение {} байт со смещения {}: {}", size, from, e))?;
    Ok(buf)
}

/// FNV-1a, 64 бита. Свой, а не крейт: двадцать строк против зависимости,
/// а криптостойкость отпечатку не нужна по построению (см. выше).
struct Fnv(u64);

impl Fnv {
    fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }

    fn update(&mut self, data: &[u8]) {
        for &byte in data {
            self.0 ^= u64::from(byte);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }

    fn finish(&self) -> u64 {
        self.0
    }
}

/// Та же свёртка по готовым срезам — для тестов: у них нет ресурса, но
/// правило «длина ∥ голова ∥ хвост» обязано быть одним.
#[cfg(test)]
fn of_slices(len: u64, head: &[u8], tail: &[u8]) -> String {
    let mut hash = Fnv::new();
    hash.update(&len.to_le_bytes());
    hash.update(head);
    hash.update(tail);
    format!("{:016x}-t{}q{}", hash.finish(), TILE, DECODE_REV)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_edges_same_print() {
        let a = of_slices(1000, b"head", b"tail");
        let b = of_slices(1000, b"head", b"tail");
        assert_eq!(a, b);
    }

    #[test]
    fn every_ingredient_matters() {
        let base = of_slices(1000, b"head", b"tail");
        assert_ne!(base, of_slices(1001, b"head", b"tail"), "длина");
        assert_ne!(base, of_slices(1000, b"heaD", b"tail"), "голова");
        assert_ne!(base, of_slices(1000, b"head", b"taiL"), "хвост");
        // Граница между кусками не теряется: (head, tail) ≠ (headt, ail)
        // было бы неверно требовать от свёртки подряд идущих байт — важно,
        // что ДЛИНА разводит такие пары раньше самих байт.
        assert_ne!(of_slices(8, b"head", b"tail"), of_slices(9, b"headt", b"ail!"));
    }

    #[test]
    fn suffix_pins_layout_and_decode_revision() {
        assert!(of_slices(1, b"a", b"").ends_with(&format!("-t{}q{}", TILE, DECODE_REV)));
    }
}
