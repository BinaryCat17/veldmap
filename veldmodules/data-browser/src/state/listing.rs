//! state/listing.rs — то, чем список отличается от самого себя: отбор,
//! группировка, порядок, страница и открытое меню.
//!
//! Одно на все три вида: сетевой каталог, поиск и скачанное — это один и тот же
//! список из разных источников, и настройки показа у них общие по устройству, а
//! не по совпадению. Своя копия у каждого вида, потому что настройка
//! принадлежит вкладке: в соседней открыта другая папка со своим отбором.

use crate::module::components::RowStatus;

/// Значение выпадающего списка: конечный набор, который ездит в разметку и
/// обратно строкой. Один трейт на все три списка — иначе у каждого свои
/// `all/label/parse`, и они расходятся.
pub trait Choice: Copy + PartialEq + Sized + 'static {
    /// Все значения по порядку — им же выложено меню.
    const ALL: &'static [Self];
    /// Ключ в сообщении разметки: имя, а не индекс, чтобы порядок в меню можно
    /// было менять.
    fn key(self) -> &'static str;
    /// Подпись в меню — полная.
    fn title(self) -> &'static str;
    /// Подпись на самом чипе — короче: рядом уже написано, чего она касается.
    fn label(self) -> &'static str;

    fn from_key(key: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|choice| choice.key() == key)
    }
}

/// Отбор по состоянию записи.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum Filter {
    #[default]
    All,
    Downloading,
    Interrupted,
    OnDisk,
    Remote,
}

impl Filter {
    pub fn matches(self, status: &RowStatus) -> bool {
        match self {
            Filter::All => true,
            Filter::Downloading => matches!(status, RowStatus::Downloading { .. }),
            Filter::Interrupted => matches!(status, RowStatus::Paused { .. }),
            Filter::OnDisk => matches!(status, RowStatus::Complete | RowStatus::Partial { .. }),
            Filter::Remote => matches!(status, RowStatus::Remote),
        }
    }
}

impl Choice for Filter {
    const ALL: &'static [Self] = &[Filter::All, Filter::Downloading, Filter::Interrupted, Filter::OnDisk, Filter::Remote];

    fn key(self) -> &'static str {
        match self {
            Filter::All => "all",
            Filter::Downloading => "downloading",
            Filter::Interrupted => "interrupted",
            Filter::OnDisk => "on-disk",
            Filter::Remote => "remote",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Filter::All => "Все",
            Filter::Downloading => "Скачиваются",
            Filter::Interrupted => "Прервано",
            Filter::OnDisk => "На диске",
            Filter::Remote => "В хранилище",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Filter::All => "все",
            Filter::Downloading => "скачиваются",
            Filter::Interrupted => "прервано",
            Filter::OnDisk => "на диске",
            Filter::Remote => "в хранилище",
        }
    }
}

/// Во что складывать строки одной папки.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum Grouping {
    #[default]
    None,
    /// Заголовок на папку целиком — одна ступень.
    Folder,
    /// Заголовок на каждый сегмент пути — сколько их, столько и ступеней.
    Tree,
}

impl Choice for Grouping {
    const ALL: &'static [Self] = &[Grouping::None, Grouping::Folder, Grouping::Tree];

    fn key(self) -> &'static str {
        match self {
            Grouping::None => "none",
            Grouping::Folder => "folder",
            Grouping::Tree => "tree",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Grouping::None => "Без группировки",
            Grouping::Folder => "По папкам",
            Grouping::Tree => "Полное дерево",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Grouping::None => "нет",
            Grouping::Folder => "по папкам",
            Grouping::Tree => "дерево",
        }
    }
}

/// Порядок строк.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum Sorting {
    #[default]
    Newest,
    Name,
    Size,
}

impl Choice for Sorting {
    const ALL: &'static [Self] = &[Sorting::Newest, Sorting::Name, Sorting::Size];

    fn key(self) -> &'static str {
        match self {
            Sorting::Newest => "newest",
            Sorting::Name => "name",
            Sorting::Size => "size",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Sorting::Newest => "Сначала новые",
            Sorting::Name => "По имени",
            Sorting::Size => "По размеру",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Sorting::Newest => "новые",
            Sorting::Name => "имя",
            Sorting::Size => "размер",
        }
    }
}

/// Какое меню сейчас раскрыто. Одно поле, а не флаг на каждое: два раскрытых
/// меню сразу — состояние, которого не бывает, и выражать его нечем.
#[derive(Clone, PartialEq, Eq, Default)]
pub enum Menu {
    #[default]
    Closed,
    Filter,
    Grouping,
    Sorting,
    /// Меню полосы запроса у поиска. Живут здесь вместе с остальными потому,
    /// что раскрытое меню бывает одно на вид, а полоса запроса стоит в том же
    /// виде, что и полоса отбора.
    Mission,
    Period,
    Cloud,
    /// Меню строки, названной своим ключом (см. `Row::key`).
    Row(String),
}

impl Menu {
    /// Ключ в сообщении разметки. Меню строки названо её ключом — им же оно и
    /// адресуется, второго имени у строки нет.
    pub fn key(&self) -> String {
        match self {
            Menu::Closed => "closed".to_string(),
            Menu::Filter => "filter".to_string(),
            Menu::Grouping => "grouping".to_string(),
            Menu::Sorting => "sorting".to_string(),
            Menu::Mission => "mission".to_string(),
            Menu::Period => "period".to_string(),
            Menu::Cloud => "cloud".to_string(),
            Menu::Row(row) => format!("row:{}", row),
        }
    }

    pub fn from_key(key: &str) -> Menu {
        match key {
            "filter" => Menu::Filter,
            "grouping" => Menu::Grouping,
            "sorting" => Menu::Sorting,
            "mission" => Menu::Mission,
            "period" => Menu::Period,
            "cloud" => Menu::Cloud,
            // Неизвестное имя — закрытое меню: показывать нечего, а падать тут
            // не за что.
            other => match other.strip_prefix("row:") {
                Some(row) => Menu::Row(row.to_string()),
                None => Menu::Closed,
            },
        }
    }
}

/// Умолчание собирается из умолчаний полей: у каждого оно своё и названо там,
/// где живёт сам тип, — вторая копия этого списка расходилась бы с ними молча.
#[derive(Default)]
pub struct ListingState {
    pub filter: Filter,
    pub grouping: Grouping,
    pub sorting: Sorting,
    /// Отбор по имени — то, что набрано в поле фильтра.
    pub query: String,
    /// Страница, считая с нуля. Любая правка отбора возвращает на первую:
    /// «страница 3» списка из одной строки — не состояние, а недоразумение.
    pub page: usize,
    pub menu: Menu,
    /// Строки, раскрытые в своё содержимое. Множество имён, а не флаг у
    /// строки: строки пересобираются на каждый кадр из библиотеки и из
    /// листинга, а «я это раскрыл» — свойство экрана и переживает пересборку.
    pub expanded: std::collections::HashSet<String>,
    /// Снимки, отмеченные пакетным выделением: их контуры лежат на шаре.
    /// Множество ключей по той же причине, что и `expanded`, — и своё у каждой
    /// вкладки: очерчивают из списка, а списков много (см. handlers::outline).
    pub selected: std::collections::HashSet<String>,
    /// Строка, к которой привёл переход: подсвечена, и её страница открыта.
    /// `None` — пришли сюда сами, и выделять нечего.
    pub target: Option<String>,
    /// Номер просьбы навести прокрутку — растёт на каждый переход к строке.
    ///
    /// Считать «навести» по самой строке нельзя: к одной и той же приводят
    /// дважды подряд, и разметка во второй раз выходит та же — рендерер видит
    /// прежнее смещение и решает, что уже навёл (см. `Scrollable.scroll_to` в
    /// ui-service/types.proto).
    pub aim: u64,
}

impl ListingState {
    /// Смена отбора: меню закрывается, страница сбрасывается. Общий хвост у
    /// всех трёх списков — вызывается после присвоения самого значения.
    pub fn refine(&mut self) {
        self.page = 0;
        self.menu = Menu::Closed;
    }

    /// Раскрыть строку в её содержимое или свернуть обратно. `true` — теперь
    /// раскрыта: содержимое папки подгружается лениво, и спрашивать его надо
    /// только при раскрытии (см. handlers::browse::request_children).
    pub fn expand(&mut self, key: String) -> bool {
        if self.expanded.remove(&key) {
            return false;
        }
        self.expanded.insert(key);
        true
    }

    /// Отметить снимок или снять отметку.
    pub fn select(&mut self, key: String) {
        if !self.selected.remove(&key) {
            self.selected.insert(key);
        }
    }
}
