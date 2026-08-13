//! Раскладка кэша — единственное место, где она известна.
//!
//! Каталог на источник, файл на тайл: fs пишет атомарно и целиком, поэтому
//! пак-файл переписывался бы весь при каждом пополнении, а файлы-тайлы
//! дописываются и теряются независимо — потерянный означает промах, не
//! поломку. meta.json — крошечный маркер каталога: его mtime — это «когда
//! источником пользовались», по нему выбирает жертв вытеснение.

/// Корень кэша. В .gitignore вместе со всем runtime/data.
pub const ROOT: &str = "data/tiles";

/// Имя маркера в каталоге источника.
pub const META: &str = "meta.json";

/// Содержимое маркера. Постоянное: перезапись того же текста двигает mtime —
/// ровно то, ради чего он и переписывается на каждом использовании.
pub const META_BODY: &[u8] = b"{\"veldmap\":\"tile-cache\"}\n";

/// Санитарный потолок стороны тайла — общий записи и чтению. Сторона тайла —
/// константа производителя, но кэш переживает её смену, а текстуру под тайл
/// всё равно выделять — граница нужна своя.
pub const MAX_TILE_SIDE: u32 = 4096;

pub fn source_dir(key: &str) -> String {
    format!("{}/{}", ROOT, key)
}

pub fn meta_path(key: &str) -> String {
    format!("{}/{}/{}", ROOT, key, META)
}

pub fn tile_path(key: &str, level: u32, x: u32, y: u32) -> String {
    format!("{}/{}/{}_{}_{}.qoi", ROOT, key, level, x, y)
}

/// Ключ приходит сообщением и попадает в путь, поэтому алфавит закрыт:
/// строчные шестнадцатеричные цифры, латиница и дефис — ровно то, что
/// производит fingerprint тайлера. Всё остальное — не ключ, а попытка
/// пройтись по чужому пути.
pub fn valid_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 64
        && key.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_flat_and_predictable() {
        assert_eq!(tile_path("ab12-t512q", 3, 0, 7), "data/tiles/ab12-t512q/3_0_7.qoi");
        assert_eq!(meta_path("ab12-t512q"), "data/tiles/ab12-t512q/meta.json");
        assert_eq!(source_dir("k"), "data/tiles/k");
    }

    #[test]
    fn key_alphabet_is_closed() {
        assert!(valid_key("0123456789abcdef-t512q"));
        assert!(!valid_key(""));
        assert!(!valid_key("../etc"));
        assert!(!valid_key("a/b"));
        assert!(!valid_key("AB12"));
        assert!(!valid_key("a b"));
        assert!(!valid_key(&"a".repeat(65)));
    }
}
