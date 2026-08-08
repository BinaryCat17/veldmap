//! Раскладка хранения — единственное место, где она известна.
//!
//! Три соглашения: каталог, суффикс `.part` у недокачанного и сидкар
//! `.origin`. Наружу не выходит ни одно — в контракте библиотеки есть только
//! имя записи, и представление о раскладке хранения не знает.

/// Каталог, в который складываются закачки.
pub const DATA_DIR: &str = "data/dem/source";

/// Суффикс недокачанного файла. Заводит его не библиотека, а host-модуль
/// network (см. network/download.rs): он пишет в `<путь>.part` и по нему же
/// возобновляет закачку с оборванного байта. Здесь этот суффикс только
/// читается — как признак «начато, но не доведено».
pub const PART_SUFFIX: &str = ".part";

/// Суффикс сидкара с происхождением файла.
pub const ORIGIN_SUFFIX: &str = ".origin";

pub fn file_path(name: &str) -> String {
    format!("{}/{}", DATA_DIR, name)
}

pub fn origin_path(name: &str) -> String {
    format!("{}/{}{}", DATA_DIR, name, ORIGIN_SUFFIX)
}

pub fn part_path(name: &str) -> String {
    format!("{}/{}{}", DATA_DIR, name, PART_SUFFIX)
}

/// Имя, под которым продукт ложится на диск — последний сегмент ключа
/// провайдера. И старт закачки, и вывод записи обязаны считать его одинаково,
/// поэтому функция одна.
pub fn name_from_identifier(identifier: &str) -> String {
    identifier.split('/').last().unwrap_or("file").to_string()
}

/// Содержимое сидкара `<имя>.origin` рядом со скачанным файлом. Пишется ДО
/// старта закачки, поэтому переживает сбой, случившийся до появления первых
/// байт: сидкар без данных — не мусор, а запись о намерении пользователя, и
/// она остаётся записью каталога.
///
/// `provider` — имя сервиса, у которого просить продукт. Сегодня он один, но
/// в сидкаре он не для будущего, а потому что это факт о файле: скачан он
/// откуда-то конкретно. `#[serde(default)]` на `total_bytes` — сидкары,
/// записанные до появления поля, должны читаться как и раньше.
#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct OriginSidecar {
    pub provider: String,
    pub identifier: String,
    #[serde(default)]
    pub total_bytes: Option<u64>,
}

/// Провайдер, которому адресуются закачки. Одно место на модуль; когда
/// провайдеров станет больше, выбор пойдёт по полю `provider` сидкара.
pub const PROVIDER_NAME: &str = "data-provider";

/// Факт о файле на диске — ровно то, что вернул fs/on_list, без домыслов.
pub struct LocalFile {
    /// Путь фактической записи, включая `.part`. Именно он идёт в fs/on_delete.
    pub path: String,
    /// Имя записи — без `.part`, то же, что будет после докачки.
    pub name: String,
    pub size: u64,
    pub is_partial: bool,
}

impl LocalFile {
    /// Разбирает запись листинга. `None` — это сидкар, а не файл: он описывает
    /// запись, а не является ею.
    pub fn from_entry(name: &str, size: u64) -> Option<Self> {
        if name.ends_with(ORIGIN_SUFFIX) {
            return None;
        }
        Some(Self {
            path: file_path(name),
            name: name.strip_suffix(PART_SUFFIX).unwrap_or(name).to_string(),
            size,
            is_partial: name.ends_with(PART_SUFFIX),
        })
    }
}
