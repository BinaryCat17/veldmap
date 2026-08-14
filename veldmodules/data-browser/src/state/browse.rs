//! Сетевой каталог: что показано в папке и что подгружено под раскрытыми
//! строками.

use std::collections::{HashMap, HashSet};

pub struct BrowseState {
    pub current_path: String,
    pub items: Vec<BrowseItem>,
    /// Содержимое раскрытых папок этого вида.
    pub children: Children,
    /// Как показывать этот список — отбор, порядок, страница.
    pub listing: super::listing::ListingState,
    /// Последняя ошибка list_path от data-provider (пусто, если запрос успешен)
    pub error: Option<String>,
    /// Ожидание ответа на data-provider/on_list_path. Актуален только
    /// последний: два быстрых перехода по папкам дают два ответа, и содержимое
    /// первого под заголовком второго — враньё, а не устаревшие данные.
    pub request: veldsdk::Latest,
}

pub struct BrowseItem {
    /// Путь продукта вместе с именем бакета (`eodata/Sentinel-2/…`) — в этом
    /// виде его принимают все топики data-provider. Ключ объекта в бакете
    /// живёт под этим префиксом, но срезает его провайдер, а не мы.
    pub identifier: String,
    pub name: String,
    pub is_folder: bool,
    /// Снимок, внутри которого лежит запись; пусто — она в пути к снимкам.
    /// Совпал с `identifier` без завершающего слэша — запись и есть снимок.
    /// Сказал провайдер: вывести границу снимка из ключа здесь не из чего
    /// (см. `ListEntry.product`).
    pub product: String,
    /// Размер объекта в байтах; 0 — папка или размер неизвестен.
    pub size: u64,
    /// Время объекта, unix-секунды; 0 — неизвестно.
    pub modified: i64,
}

impl BrowseState {
    /// Показывает ли вкладка эту папку прямо сейчас — то есть можно ли
    /// обойтись без сетевого хода. Только что открытая не показывает ничего,
    /// даже когда путь совпал: у пустой вкладки он тоже пустой.
    pub fn shows(&self, path: &str) -> bool {
        self.current_path == path && (!self.items.is_empty() || self.request.is_pending())
    }
}

impl Default for BrowseState {
    fn default() -> Self {
        Self {
            current_path: String::new(),
            items: Vec::new(),
            children: Children::default(),
            listing: Default::default(),
            error: None,
            request: veldsdk::Latest::new(),
        }
    }
}

/// Содержимое раскрытых папок: ключ папки → её записи.
///
/// Одно устройство на каталог и на поиск: раскрытая строка там и там
/// показывает один и тот же листинг провайдера, и второй способ его помнить
/// разошёлся бы с первым. Листается лениво — папку спрашивают, когда её
/// раскрыли, а не когда показали строку: в корне бакета папок сотни, и обход
/// каждой стоил бы сотни запросов ради одной раскрытой.
///
/// «Спрашивали ли» и «ответили ли» — разные вопросы, и носители у них разные.
/// На первый отвечает наличие ключа: по нему видно, надо ли идти в сеть. На
/// второй — `waiting`, и нужен он одному показу: раскрытая пустая папка и
/// раскрытая, чей ответ ещё идёт, выглядят одинаково, и вторая секунду
/// читается как «здесь ничего нет».
#[derive(Default)]
pub struct Children {
    /// Ключ есть — папку спросили; ключа нет — не спрашивали.
    loaded: HashMap<String, Vec<BrowseItem>>,
    /// Папки, чей листинг ещё в пути.
    waiting: HashSet<String>,
}

impl Children {
    /// Записи раскрытой папки; пусто — ещё не приехали или её нечем наполнить.
    pub fn get(&self, key: &str) -> &[BrowseItem] {
        self.loaded.get(key).map_or(&[], Vec::as_slice)
    }

    /// Спрашивали ли уже эту папку. `false` — надо спросить.
    pub fn known(&self, key: &str) -> bool {
        self.loaded.contains_key(key)
    }

    /// Ждём ли ещё ответа по этой папке — то, что показывают строкой ожидания.
    pub fn waiting(&self, key: &str) -> bool {
        self.waiting.contains(key)
    }

    /// Листинг папки пошёл.
    pub fn begin(&mut self, key: String) {
        self.loaded.insert(key.clone(), Vec::new());
        self.waiting.insert(key);
    }

    /// Ответ приехал целиком — или не приехал вовсе. Ждать больше нечего.
    pub fn settle(&mut self, key: &str) {
        self.waiting.remove(key);
    }

    /// Очередная страница листинга. `false` — папку успели забыть: ушли из
    /// неё или сменилась выдача.
    ///
    /// Именно `false`, а не тихая вставка: страницы приходят по одной, и
    /// брошенная цепочка продолжается с того токена, где её оставили. Заведи
    /// она ключ заново — в наборе оказался бы хвост листинга без головы, и
    /// раскрытая папка показала бы содержимое, начинающееся с середины.
    pub fn extend(&mut self, key: &str, items: impl IntoIterator<Item = BrowseItem>) -> bool {
        let Some(into) = self.loaded.get_mut(key) else { return false };
        into.extend(items);
        true
    }

    /// Забыть одну папку. Так уходит та, чей листинг отказал: «спрашивали» —
    /// не то же, что «спросили», и оставленный ключ значил бы «пусто навсегда».
    pub fn forget(&mut self, key: &str) {
        self.loaded.remove(key);
        self.waiting.remove(key);
    }

    /// Всё забыть — так уходит содержимое, принадлежавшее прошлой папке или
    /// прошлой выдаче.
    pub fn clear(&mut self) {
        self.loaded.clear();
        self.waiting.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(name: &str) -> BrowseItem {
        BrowseItem {
            identifier: format!("eodata/{}", name),
            name: name.to_string(),
            is_folder: false,
            product: String::new(),
            size: 0,
            modified: 0,
        }
    }

    /// «Спрашивали» — это «ключ здесь есть», и заводится он в тот же миг, когда
    /// листинг пошёл: иначе раскрытая строка спрашивала бы содержимое заново на
    /// каждую пересборку разметки.
    #[test]
    fn asking_and_empty_are_told_apart_by_the_key_alone() {
        let mut children = Children::default();
        assert!(!children.known("a/"), "не спрашивали");

        children.begin("a/".to_string());
        assert!(children.known("a/"), "спросили — второй раз не пойдём");
        assert!(children.get("a/").is_empty(), "ответа ещё нет");
    }

    /// Страница брошенной цепочки ключ не воскрешает: цепочка продолжается с
    /// того токена, где её оставили, и заведи она папку заново — в наборе лёг
    /// бы хвост листинга без головы.
    #[test]
    fn a_page_of_a_forgotten_folder_is_dropped() {
        let mut children = Children::default();
        children.begin("a/".to_string());
        assert!(children.extend("a/", [item("one")]), "папка на месте");
        assert_eq!(children.get("a/").len(), 1);

        children.clear();
        assert!(!children.extend("a/", [item("two")]), "папку забыли");
        assert!(!children.known("a/"), "и хвост её не воскресил");
    }

    /// Отказ листинга папку забывает: оставленный пустой ключ значил бы
    /// «спрашивали, и там пусто», то есть закрыл бы дорогу повтору навсегда.
    #[test]
    fn a_forgotten_folder_can_be_asked_again() {
        let mut children = Children::default();
        children.begin("a/".to_string());
        children.forget("a/");
        assert!(!children.known("a/"));
    }
}
