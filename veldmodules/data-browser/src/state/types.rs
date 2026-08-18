//! Вид — то, что показывается в одной вкладке.
//!
//! Своё состояние держит сам вариант `ViewKind`, а не модуль: два открытых
//! Browse — это две независимые папки, а не один экран с общей переменной.

use crate::module::components::last_segment;
use super::listing::{choice, Choice};
use super::browse::{BrowseState, Children};
use super::globe::GlobeState;
use super::layout::PaneId;
use super::listing::ListingState;
use super::preview::PreviewState;
use super::search::SearchState;

choice! {
    /// Куда двигать слой в наборе. Перечислением, а не «вверх: да/нет»: у сдвига
    /// два равноправных направления, и одно из них не является отрицанием
    /// другого — оно просто другое.
    Shift {
        Up   = "up",   "Выше";
        Down = "down", "Ниже";
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
    /// Заведена, но ещё ничего не показывает: спрашивает, чем её наполнить.
    /// Своего состояния у неё нет — весь её вид и есть этот вопрос.
    ///
    /// Вкладкой, а не пустой панелью: панель без вкладок в дереве не живёт
    /// (см. `state::layout`), а место, где человек ещё не решил, что смотреть,
    /// нужно — с него начинается всякое «открыть рядом».
    Empty,
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
            ViewKind::Empty => NewTab::Empty,
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
            ViewKind::Empty | ViewKind::Preview(_) | ViewKind::Globe(_) | ViewKind::Shown => None,
        }
    }

    /// Они же на чтение — их спрашивает сборка контуров: что в этом списке
    /// отмечено (см. handlers::outline).
    pub fn listing(&self) -> Option<&ListingState> {
        match self {
            ViewKind::Search(search) => Some(&search.listing),
            ViewKind::Browse(browse) => Some(&browse.listing),
            ViewKind::Downloaded(listing) => Some(listing),
            ViewKind::Empty | ViewKind::Preview(_) | ViewKind::Globe(_) | ViewKind::Shown => None,
        }
    }

    /// Содержимое раскрытых папок. Есть там, где строки приходят из каталога
    /// провайдера: у скачанного файлы снимка уже под рукой, листать нечего.
    pub fn children_mut(&mut self) -> Option<&mut Children> {
        match self {
            ViewKind::Search(search) => Some(&mut search.children),
            ViewKind::Browse(browse) => Some(&mut browse.children),
            ViewKind::Empty
            | ViewKind::Downloaded(_)
            | ViewKind::Preview(_)
            | ViewKind::Globe(_)
            | ViewKind::Shown => None,
        }
    }
}

pub struct View {
    pub id: ViewId,
    pub kind: ViewKind,
    /// В какой панели лежит эта вкладка. Поле у вида, а не список вкладок у
    /// панели: id уникален на всё окно, и поиск по нему обязан иметь один
    /// ответ — а список у каждой панели отвечал бы на него перебором всех, да
    /// ещё и допускал бы вкладку сразу в двух местах.
    pub pane: PaneId,
}

/// Сколько знаков заголовка помещается на вкладке. Имена продуктов Copernicus
/// доходят до восьмидесяти знаков, и вкладка по ширине имени — это одна
/// вкладка на всё окно; полное имя видно в заголовке самого вида.
const TITLE_LIMIT: usize = 22;

impl ViewKind {
    /// Заголовок вкладки — последний сегмент пути, обрезанный до `TITLE_LIMIT`.
    pub fn title(&self) -> String {
        match self {
            // Подпись у пустой вкладки та же, которой её предлагает «плюс»:
            // это одна и та же вещь до и после нажатия.
            ViewKind::Empty => crate::module::NewTab::Empty.title().to_string(),
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

fn ellipsize(name: &str) -> String {
    crate::module::components::format::ellipsize(name, TITLE_LIMIT)
}
