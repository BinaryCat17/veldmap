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
    /// Подпись на самом чипе. По умолчанию та же, что в меню: короче она
    /// бывает не у всякого набора, а разная там, где рядом уже написано, чего
    /// она касается.
    fn label(self) -> &'static str {
        self.title()
    }

    fn from_key(key: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|choice| choice.key() == key)
    }
}

/// Объявляет набор вместе с его ключом провода и подписями.
///
/// Одним местом на четыре списка: сами варианты, порядок в меню, ключ, которым
/// вариант ездит в разметку и обратно, и то, как он назван человеку. Написанные
/// порознь четырьмя `match`, они расходятся молча — забытая ветка `key` даёт не
/// ошибку компиляции, а нажатие, которое не разобралось.
///
/// Первый вариант — умолчание набора. Подпись на чипе необязательна: не
/// написана — та же, что в меню.
macro_rules! choice {
    (
        $(#[$outer:meta])*
        $name:ident {
            $(#[$head_doc:meta])* $head:ident = $head_key:literal, $head_title:literal
                $(, $head_label:literal)? ;
            $( $(#[$doc:meta])* $variant:ident = $key:literal, $title:literal
                $(, $label:literal)? ; )*
        }
    ) => {
        $(#[$outer])*
        #[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
        pub enum $name {
            #[default]
            $(#[$head_doc])*
            $head,
            $( $(#[$doc])* $variant, )*
        }

        impl $crate::module::state::listing::Choice for $name {
            const ALL: &'static [Self] = &[$name::$head, $( $name::$variant ),*];

            fn key(self) -> &'static str {
                match self {
                    $name::$head => $head_key,
                    $( $name::$variant => $key, )*
                }
            }

            fn title(self) -> &'static str {
                match self {
                    $name::$head => $head_title,
                    $( $name::$variant => $title, )*
                }
            }

            fn label(self) -> &'static str {
                match self {
                    $name::$head => choice!(@label $head_title $(, $head_label)?),
                    $( $name::$variant => choice!(@label $title $(, $label)?), )*
                }
            }
        }
    };
    (@label $title:literal) => { $title };
    (@label $title:literal, $label:literal) => { $label };
}

pub(crate) use choice;

choice! {
    /// Отбор по состоянию записи.
    Filter {
        All         = "all",         "Все",         "все";
        Downloading = "downloading", "Скачиваются", "скачиваются";
        Interrupted = "interrupted", "Прервано",    "прервано";
        OnDisk      = "on-disk",     "На диске",    "на диске";
        Remote      = "remote",      "В хранилище", "в хранилище";
    }
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

choice! {
    /// Во что складывать строки одной папки.
    Grouping {
        None = "none", "Без группировки", "нет";
        /// Заголовок на папку целиком — одна ступень.
        Folder = "folder", "По папкам", "по папкам";
        /// Заголовок на каждый сегмент пути — сколько их, столько и ступеней.
        Tree = "tree", "Полное дерево", "дерево";
    }
}

choice! {
    /// Порядок строк.
    Sorting {
        Newest = "newest", "Сначала новые", "новые";
        Name   = "name",   "По имени",      "имя";
        Size   = "size",   "По размеру",    "размер";
    }
}

/// Какое из меню списка. Что раскрыто хоть что-то, говорит не этот тип, а
/// `State::open`: «ничего не раскрыто» — свойство экрана, а не списка.
#[derive(Clone, PartialEq, Eq)]
pub enum Menu {
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
            Menu::Filter => "filter".to_string(),
            Menu::Grouping => "grouping".to_string(),
            Menu::Sorting => "sorting".to_string(),
            Menu::Mission => "mission".to_string(),
            Menu::Period => "period".to_string(),
            Menu::Cloud => "cloud".to_string(),
            Menu::Row(row) => format!("row:{}", row),
        }
    }

    /// `None` — закрыть раскрытое: пустым именем едет и «закрой», и
    /// неизвестное имя, а показывать по нему нечего.
    pub fn from_key(key: &str) -> Option<Menu> {
        Some(match key {
            "filter" => Menu::Filter,
            "grouping" => Menu::Grouping,
            "sorting" => Menu::Sorting,
            "mission" => Menu::Mission,
            "period" => Menu::Period,
            "cloud" => Menu::Cloud,
            other => Menu::Row(other.strip_prefix("row:")?.to_string()),
        })
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
    /// Строки, раскрытые в своё содержимое. Множество имён, а не флаг у
    /// строки: строки пересобираются на каждый кадр из библиотеки и из
    /// листинга, а «я это раскрыл» — свойство экрана и переживает пересборку.
    pub expanded: std::collections::HashSet<String>,
    /// Выбранные строки: ключ строки → что за ней стои́т. Своё у каждой
    /// вкладки и переживает пересборку по той же причине, что и `expanded`.
    pub selected: std::collections::HashMap<String, Chosen>,
    /// Номер просьбы навести прокрутку — растёт на каждый переход к строке.
    ///
    /// Считать «навести» по самой строке нельзя: к одной и той же приводят
    /// дважды подряд, и разметка во второй раз выходит та же — рендерер видит
    /// прежнее смещение и решает, что уже навёл (см. `Scrollable.scroll_to` в
    /// ui-service/types.proto).
    pub aim: u64,
}

/// Что стои́т за выбранной строкой.
///
/// Род, а не флаг «очерчивается»: выбирают строку, чтобы что-то с ней сделать —
/// удалить, скачать, — и это разное действие у снимка и у отдельного файла.
/// Снимок библиотека не знает вовсе, его разворачивают в файлы (см.
/// `handlers::library::files_of`), а файл адресуют его собственными ключами.
/// Про шар род ничего не говорит: контур и показ — свои состояния снимка
/// (см. handlers::outline).
///
/// Ключи файл несёт с собой, а не берутся из строки в момент действия: выбор
/// переживает переход в другую папку, и строки к тому времени может уже не
/// быть на экране.
#[derive(Clone, PartialEq, Eq)]
pub enum Chosen {
    /// Снимок — одна логическая единица каталога. Ключ выбора у него и есть
    /// ключ снимка, поэтому своих полей ему не нужно.
    Snapshot,
    /// Файл сам по себе: скачанный вне снимка либо снимку неизвестный.
    File {
        /// Ключ провайдера; пусто — скачивать нечем.
        identifier: String,
        /// Снимок, в который файл ложится: границу снимка знает провайдер, и
        /// закачка спрашивает её вместе с ключом.
        product: String,
        /// Имя записи в библиотеке; пусто — на диске её нет, удалять нечего.
        name: String,
    },
}

impl ListingState {
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

    /// Выбрать строку или снять выбор.
    pub fn select(&mut self, key: String, chosen: Chosen) {
        if self.selected.remove(&key).is_none() {
            self.selected.insert(key, chosen);
        }
    }
}
