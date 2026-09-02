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
//! Суффикс кодирует раскладку и правила производства (`-t<сторона тайла>q<ревизия
//! декодирования>`): смена любого меняет ключ, и старые каталоги кэша просто
//! стареют до вытеснения — без версий и миграций.

use std::io::{Read, Seek, SeekFrom};

use super::pyramid::TILE;

/// Сколько байт берётся с каждого края.
const SAMPLE: u64 = 64 * 1024;

/// Ревизия правил декодирования. Поднимается правкой, меняющей содержимое
/// тайлов при тех же байтах источника — растяг широких форматов, ключевание
/// «нет данных», взвешивание ужатия, — иначе рядом с новыми тайлами из кэша
/// всплывали бы старые той же самой картинки.
const DECODE_REV: u32 = 12;

/// Отпечаток ресурса: `<fnv64 hex>-t<TILE>q<DECODE_REV>`.
pub fn fingerprint(resource_id: u64, len: u64) -> Result<String, String> {
    let mut reader = veldsdk::ResourceReader::new(resource_id, len);
    let (head, tail) = edges(len);
    let head = read_exact_at(&mut reader, head.0, head.1)?;
    let tail = match tail {
        Some((from, size)) => read_exact_at(&mut reader, from, size)?,
        None => Vec::new(),
    };
    Ok(hash_of(len, &head, &tail))
}

/// Свёртка «длина ∥ голова ∥ хвост» по готовым срезам — одна и для чтения
/// ресурса, и для тестов, у которых ресурса нет.
fn hash_of(len: u64, head: &[u8], tail: &[u8]) -> String {
    let mut hash = Fnv::new();
    hash.update(&len.to_le_bytes());
    hash.update(head);
    hash.update(tail);
    format!("{:016x}-t{}q{}", hash.finish(), TILE, DECODE_REV)
}

/// Какие куски файла попадают в отпечаток: голова и не перекрытый ею хвост.
///
/// Хвост отдельным ответом, а не вторым куском наравне с головой: у короткого
/// файла второго куска нет вовсе, а не «тот же кусок ещё раз». Перехлёста быть
/// не должно — не ради скорости, а ради смысла: перехлестнувшись, отпечаток
/// хэшировал бы общие байты дважды и переставал отвечать заявленному правилу
/// «длина ∥ голова ∥ хвост». Тихо: файлы 64–128 КиБ сменили бы отпечаток, и
/// весь кэш тайлов для них промахнулся бы навсегда.
fn edges(len: u64) -> ((u64, u64), Option<(u64, u64)>) {
    let head = (0, SAMPLE.min(len));
    let tail = (len > SAMPLE).then(|| {
        let from = (len - SAMPLE).max(SAMPLE);
        (from, len - from)
    });
    (head, tail)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_edges_same_print() {
        let a = hash_of(1000, b"head", b"tail");
        let b = hash_of(1000, b"head", b"tail");
        assert_eq!(a, b);
    }

    #[test]
    fn every_ingredient_matters() {
        let base = hash_of(1000, b"head", b"tail");
        assert_ne!(base, hash_of(1001, b"head", b"tail"), "длина");
        assert_ne!(base, hash_of(1000, b"heaD", b"tail"), "голова");
        assert_ne!(base, hash_of(1000, b"head", b"taiL"), "хвост");
        // Граница между кусками не теряется: (head, tail) ≠ (headt, ail)
        // было бы неверно требовать от свёртки подряд идущих байт — важно,
        // что ДЛИНА разводит такие пары раньше самих байт.
        assert_ne!(hash_of(8, b"head", b"tail"), hash_of(9, b"headt", b"ail!"));
    }

    /// Голова и хвост не перехлёстываются и не оставляют дыры посередине
    /// раньше, чем файл станет длиннее двух выборок.
    #[test]
    fn edges_never_overlap_and_never_leave_a_gap_too_early() {
        for len in [0, 1, SAMPLE - 1, SAMPLE, SAMPLE + 1, 2 * SAMPLE - 1, 2 * SAMPLE, 2 * SAMPLE + 1] {
            let ((head_at, head_len), tail) = edges(len);
            assert_eq!((head_at, head_len), (0, SAMPLE.min(len)), "голова при {}", len);

            let Some((tail_at, tail_len)) = tail else {
                assert!(len <= SAMPLE, "хвост потерян при {}", len);
                continue;
            };
            assert!(tail_at >= head_len, "перехлёст при длине {}: хвост с {}", len, tail_at);
            assert_eq!(tail_at + tail_len, len, "хвост не доходит до конца при {}", len);
            // Дыра посередине появляется только там, где ей и место, — когда
            // файл длиннее двух выборок.
            assert_eq!(tail_at > head_len, len > 2 * SAMPLE, "дыра не там при {}", len);
        }
    }

    /// Прочитанное — ровно то, что назвали края: всякий байт файла короче двух
    /// выборок попадает в свёртку ровно один раз.
    #[test]
    fn short_files_are_read_whole_and_once() {
        for len in [1_u64, SAMPLE / 2, SAMPLE, SAMPLE + 1, 2 * SAMPLE] {
            let ((_, head_len), tail) = edges(len);
            let read = head_len + tail.map_or(0, |(_, size)| size);
            assert_eq!(read, len, "длина {}: прочитано {}", len, read);
        }
    }

    #[test]
    fn suffix_pins_layout_and_decode_revision() {
        assert!(hash_of(1, b"a", b"").ends_with(&format!("-t{}q{}", TILE, DECODE_REV)));
    }
}
