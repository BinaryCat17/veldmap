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

/// Где лежат данные записи: недокачанные — под `.part`, доведённые — под самим
/// именем. Одна функция на оба случая, потому что выбор между ними — это один
/// факт (доведена ли закачка), а не два независимых.
pub fn data_path(name: &str, is_partial: bool) -> String {
    if is_partial { part_path(name) } else { file_path(name) }
}

/// Имя, под которым продукт ложится на диск: у файла из снимка — имя снимка и
/// путь внутри него (`S2A_….SAFE/GRANULE/…/T36UYA_….jp2`), у всего остального —
/// последний сегмент ключа.
///
/// Путём, а не последним сегментом, потому что имя записи — её ключ, и второй
/// файл под тем же ключом означал бы не второй файл, а тот же самый:
/// `quick-look.png` и `manifest.safe` лежат в каждом продукте, и скачанный
/// вторым сносил бы первый. Снимок целиком отвечает за уникальность своей
/// половины ключа, путь внутри — за свою.
///
/// Границу снимка называет провайдер (`product`); вывести её отсюда не из
/// чего — раскладка бакета известна только ему.
pub fn name_from_identifier(identifier: &str, product: &str) -> String {
    let identifier = identifier.trim_end_matches('/');
    let product = product.trim_end_matches('/');
    let last = |path: &str| path.rsplit('/').next().unwrap_or("file").to_string();
    if product.is_empty() {
        return last(identifier);
    }
    match identifier.strip_prefix(product).and_then(|rest| rest.strip_prefix('/')) {
        // Файл внутри снимка: снимок папкой, путь внутри — как в хранилище.
        Some(inside) => format!("{}/{}", last(product), inside),
        // Снимок одним объектом (архив, гранула): ключ и есть его корень.
        None => last(identifier),
    }
}

/// Содержимое сидкара `<имя>.origin` рядом со скачанным файлом. Пишется ДО
/// старта закачки, поэтому переживает сбой, случившийся до появления первых
/// байт: сидкар без данных — не мусор, а запись о намерении пользователя, и
/// она остаётся записью каталога.
///
/// `provider` — имя сервиса, у которого просить продукт. Сегодня он один, но
/// в сидкаре он не для будущего, а потому что это факт о файле: скачан он
/// откуда-то конкретно.
///
/// Необязательны здесь те поля, которых у файла может не быть вовсе: полный
/// размер неизвестен, пока его не назвал сервер, а снимка у файла нет, если
/// его скачали сам по себе. Отсутствие такого поля в сидкаре — законное
/// состояние, а не повод счесть сидкар негодным.
#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct OriginSidecar {
    pub provider: String,
    pub identifier: String,
    #[serde(default)]
    pub total_bytes: Option<u64>,
    /// Снимок, к которому файл относится; пусто — сам по себе. Приходит с
    /// запросом закачки: границу снимка знает провайдер, а не мы, — и живёт
    /// здесь ровно потому, что второго места, где этот факт переживёт рестарт,
    /// у нас нет.
    #[serde(default)]
    pub product: String,
    /// Сколько файлов в снимке всего; 0 — неизвестно. По нему и только по нему
    /// видно, скачан ли снимок целиком: сколько его файлов доведено, мы знаем
    /// сами, а сколько их должно быть — нет.
    ///
    /// Рядом с `product`, потому что это тот же род знания: факт о снимке,
    /// добытый провайдером и приписанный файлу, — и переживает рестарт он тем
    /// же способом.
    #[serde(default)]
    pub siblings: u32,
}

/// Провайдер, которому адресуются закачки. Одно место на модуль; когда
/// провайдеров станет больше, выбор пойдёт по полю `provider` сидкара.
pub const PROVIDER_NAME: &str = "data-provider";

/// Факт о файле на диске — ровно то, что вернул fs/on_list, без домыслов.
///
/// Имени здесь нет: оно ключ записи в снимке, и второе его место означало бы
/// две правды об одном. Пути тоже нет — он выводится из имени и `is_partial`
/// (см. [`data_path`]), а хранить его рядом значит завести способ сказать
/// «доведена» и «лежит под .part» по-разному.
pub struct LocalFile {
    pub size: u64,
    pub is_partial: bool,
    /// Время файла на диске, unix-секунды; 0 — файловая система его не отдала.
    pub modified: i64,
}

impl LocalFile {
    /// Разбирает запись листинга в пару «имя записи → что под ним лежит».
    /// `None` — это сидкар, а не файл: он описывает запись, а не является ею.
    pub fn from_entry(name: &str, size: u64, modified: i64) -> Option<(String, Self)> {
        if name.ends_with(ORIGIN_SUFFIX) {
            return None;
        }
        let is_partial = name.ends_with(PART_SUFFIX);
        let entry = Self { size, is_partial, modified };
        Some((name.strip_suffix(PART_SUFFIX).unwrap_or(name).to_string(), entry))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ключ снимка и путь внутри него — в имени записи оба, потому что
    /// уникальным его делают только они вместе.
    #[test]
    fn имя_из_ключа_внутри_снимка() {
        let product = "eodata/Sentinel-2/MSI/L2A/2026/08/13/S2A_MSIL2A_20260813.SAFE";
        let file = format!("{}/GRANULE/L2A_T36UYA/IMG_DATA/R10m/T36UYA_TCI_10m.jp2", product);
        assert_eq!(
            name_from_identifier(&file, product),
            "S2A_MSIL2A_20260813.SAFE/GRANULE/L2A_T36UYA/IMG_DATA/R10m/T36UYA_TCI_10m.jp2"
        );
    }

    /// Та беда, ради которой имя и перестало быть последним сегментом:
    /// `quick-look.png` лежит в каждом продукте.
    #[test]
    fn одноимённые_файлы_разных_снимков_не_сходятся() {
        let (a, b) = ("eodata/S1/A.SAFE", "eodata/S1/B.SAFE");
        let left = name_from_identifier(&format!("{}/preview/quick-look.png", a), a);
        let right = name_from_identifier(&format!("{}/preview/quick-look.png", b), b);
        assert_ne!(left, right);
        assert_eq!(left, "A.SAFE/preview/quick-look.png");
    }

    /// Снимок одним объектом: ключ и есть его корень, вкладывать не во что.
    #[test]
    fn снимок_одним_объектом_кладётся_плоско() {
        let key = "eodata/Landsat-5/L5.tar.gz";
        assert_eq!(name_from_identifier(key, key), "L5.tar.gz");
    }

    /// Файл сам по себе — снимка нет, и путь ему брать неоткуда.
    #[test]
    fn файл_без_снимка_кладётся_по_последнему_сегменту() {
        assert_eq!(name_from_identifier("eodata/dem/N00_E013.tif", ""), "N00_E013.tif");
    }

    /// Путь к данным и путь к сидкару выводятся из имени, и вложенность на них
    /// не влияет: раскладка одна на любую глубину.
    #[test]
    fn пути_выводятся_из_имени_на_любой_глубине() {
        let name = "A.SAFE/preview/quick-look.png";
        assert_eq!(data_path(name, false), "data/dem/source/A.SAFE/preview/quick-look.png");
        assert_eq!(data_path(name, true), "data/dem/source/A.SAFE/preview/quick-look.png.part");
        assert_eq!(origin_path(name), "data/dem/source/A.SAFE/preview/quick-look.png.origin");
    }

    /// Разбор листинга не знает про вложенность вовсе — имя приезжает путём, и
    /// суффиксы читаются у его хвоста.
    #[test]
    fn запись_листинга_разбирается_путём() {
        let (name, file) = LocalFile::from_entry("A.SAFE/x.jp2.part", 10, 5).expect("это файл");
        assert_eq!(name, "A.SAFE/x.jp2");
        assert!(file.is_partial);
        assert!(LocalFile::from_entry("A.SAFE/x.jp2.origin", 1, 1).is_none());
    }
}
