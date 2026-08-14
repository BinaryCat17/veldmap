//! Вид — то, что показывается в одной вкладке.
//!
//! Своё состояние держит сам вариант `ViewKind`, а не модуль: два открытых
//! Browse — это две независимые папки, а не один экран с общей переменной.

use super::browse::BrowseState;
use super::globe::GlobeState;
use super::listing::ListingState;
use super::preview::PreviewState;
use super::search::SearchState;

/// Половина экрана. Их две и всегда две: «разделить надвое» — законченный
/// жест, и его хватает, чтобы держать рядом список и то, чем он управляет.
/// Произвольное дерево панелей потребовало бы способа его собирать и
/// разбирать, то есть целого языка вместо одной кнопки.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Half {
    First,
    Second,
}

impl Half {
    pub const BOTH: [Half; 2] = [Half::First, Half::Second];

    pub fn other(self) -> Half {
        match self {
            Half::First => Half::Second,
            Half::Second => Half::First,
        }
    }

    pub(super) fn index(self) -> usize {
        match self {
            Half::First => 0,
            Half::Second => 1,
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Half::First => "first",
            Half::Second => "second",
        }
    }

    pub fn from_key(key: &str) -> Option<Half> {
        match key {
            "first" => Some(Half::First),
            "second" => Some(Half::Second),
            _ => None,
        }
    }
}

/// Где встаёт вторая половина. Первая всегда там, где была, — иначе
/// разделение экрана двигало бы то, на что человек смотрит.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Placement {
    Right,
    Left,
    Below,
}

/// Куда двигать слой в наборе. Перечислением, а не «вверх: да/нет»: у сдвига
/// два равноправных направления, и одно из них не является отрицанием другого
/// — оно просто другое.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Shift {
    Up,
    Down,
}

impl Shift {
    pub fn key(self) -> &'static str {
        match self {
            Shift::Up => "up",
            Shift::Down => "down",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        match key {
            "up" => Some(Shift::Up),
            "down" => Some(Shift::Down),
            _ => None,
        }
    }
}

impl Placement {
    pub const ALL: [Placement; 3] = [Placement::Right, Placement::Left, Placement::Below];

    pub fn key(self) -> &'static str {
        match self {
            Placement::Right => "right",
            Placement::Left => "left",
            Placement::Below => "below",
        }
    }

    pub fn from_key(key: &str) -> Option<Placement> {
        Placement::ALL.into_iter().find(|placement| placement.key() == key)
    }

    pub fn title(self) -> &'static str {
        match self {
            Placement::Right => "Открыть справа",
            Placement::Left => "Открыть слева",
            Placement::Below => "Открыть снизу",
        }
    }
}

/// Идентификатор открытого вида. Уникален в пределах сессии и не
/// переиспользуется: по нему адресуется и вкладка в разметке, и ответ,
/// приехавший на запрос этого вида, — а вид к тому времени могли закрыть,
/// и переиспользованный id отдал бы чужой ресурс живой вкладке.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ViewId(pub(super) u64);

impl std::fmt::Display for ViewId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Вкладка возвращает свой id строкой в `UiEventResponse.value` — разобрать
/// его обратно нужно там же, где принимается нажатие.
impl std::str::FromStr for ViewId {
    type Err = std::num::ParseIntError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse().map(ViewId)
    }
}

pub enum ViewKind {
    Search(SearchState),
    Browse(BrowseState),
    /// Строки берутся из `LibraryState` — он один на модуль и приходит
    /// рассылкой (см. state::library); своё здесь только то, как их показать.
    Downloaded(ListingState),
    Preview(PreviewState),
    /// Шар рисует отдельный модуль; здесь только место под него и жест.
    Globe(GlobeState),
    /// Что сейчас лежит на шаре. Своего состояния у вида нет: слои живут в
    /// `State::overlays` — они свойство шара, а не экрана, и переживают
    /// закрытие этой вкладки, как переживают его контуры.
    Shown,
}

impl ViewKind {
    /// Каким родом вкладки этот вид заводят. `None` — просмотр снимка: его не
    /// открывают из меню, его открывает строка списка.
    ///
    /// Нужно затем, чтобы подпись и значок рода жили в одном месте: вкладка и
    /// пункт меню обозначают одно и то же, и разъехавшиеся значки читались бы
    /// как две разные вещи.
    pub fn opened_as(&self) -> Option<crate::module::NewTab> {
        use crate::module::NewTab;
        Some(match self {
            ViewKind::Search(_) => NewTab::Search,
            ViewKind::Browse(_) => NewTab::Browse,
            ViewKind::Downloaded(_) => NewTab::Downloaded,
            ViewKind::Globe(_) => NewTab::Globe,
            ViewKind::Shown => NewTab::Shown,
            ViewKind::Preview(_) => return None,
        })
    }

    /// Настройки показа списка — они есть у всех видов, кроме превью, и
    /// правятся одними и теми же сообщениями. Без этого каждое из них
    /// разбирало бы `ViewKind` заново, по-своему и с тремя ветками.
    pub fn listing_mut(&mut self) -> Option<&mut ListingState> {
        match self {
            ViewKind::Search(search) => Some(&mut search.listing),
            ViewKind::Browse(browse) => Some(&mut browse.listing),
            ViewKind::Downloaded(listing) => Some(listing),
            ViewKind::Preview(_) | ViewKind::Globe(_) | ViewKind::Shown => None,
        }
    }
}

pub struct View {
    pub id: ViewId,
    pub kind: ViewKind,
    /// В какой половине экрана лежит эта вкладка. Поле у вида, а не два списка
    /// в состоянии: id уникален на всё окно, и поиск по нему обязан иметь один
    /// ответ — а два списка отвечали бы на него перебором обоих.
    pub half: Half,
}

/// Сколько знаков заголовка помещается на вкладке. Имена продуктов Copernicus
/// доходят до восьмидесяти знаков, и вкладка по ширине имени — это одна
/// вкладка на всё окно; полное имя видно в заголовке самого вида.
const TITLE_LIMIT: usize = 22;

impl ViewKind {
    /// Заголовок вкладки — последний сегмент пути, обрезанный до `TITLE_LIMIT`.
    pub fn title(&self) -> String {
        match self {
            ViewKind::Search(_) => "Поиск снимков".to_string(),
            ViewKind::Browse(browse) => match last_segment(&browse.current_path) {
                "" => "Каталог".to_string(),
                name => ellipsize(name),
            },
            ViewKind::Downloaded(_) => "Скачанное".to_string(),
            ViewKind::Preview(preview) => match last_segment(&preview.label) {
                "" => "Просмотр".to_string(),
                name => ellipsize(name),
            },
            ViewKind::Globe(_) => "Глобус".to_string(),
            ViewKind::Shown => "На просмотре".to_string(),
        }
    }
}

/// Последний непустой сегмент пути — им подписана вкладка.
fn last_segment(path: &str) -> &str {
    path.split('/').filter(|part| !part.is_empty()).next_back().unwrap_or("")
}

fn ellipsize(name: &str) -> String {
    crate::module::components::format::ellipsize(name, TITLE_LIMIT)
}
