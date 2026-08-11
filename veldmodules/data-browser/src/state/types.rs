//! Вид — то, что показывается в одной вкладке.
//!
//! Своё состояние держит сам вариант `ViewKind`, а не модуль: два открытых
//! Browse — это две независимые папки, а не один экран с общей переменной.

use super::browse::BrowseState;
use super::listing::ListingState;
use super::preview::PreviewState;
use super::search::SearchState;

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
}

impl ViewKind {
    /// Настройки показа списка — они есть у всех видов, кроме превью, и
    /// правятся одними и теми же сообщениями. Без этого каждое из них
    /// разбирало бы `ViewKind` заново, по-своему и с тремя ветками.
    pub fn listing_mut(&mut self) -> Option<&mut ListingState> {
        match self {
            ViewKind::Search(search) => Some(&mut search.listing),
            ViewKind::Browse(browse) => Some(&mut browse.listing),
            ViewKind::Downloaded(listing) => Some(listing),
            ViewKind::Preview(_) => None,
        }
    }
}

pub struct View {
    pub id: ViewId,
    pub kind: ViewKind,
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
