//! Вид — то, что показывается в одной вкладке.
//!
//! Своё состояние держит сам вариант `ViewKind`, а не модуль: два открытых
//! Browse — это две независимые папки, а не один экран с общей переменной.

use super::browse::BrowseState;
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
    /// Своего состояния нет: экран целиком выводится из `LibraryState`, а он
    /// один на модуль и приходит рассылкой (см. state::library).
    Downloaded,
    Preview(PreviewState),
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
            ViewKind::Search(_) => "Search".to_string(),
            ViewKind::Browse(browse) => match last_segment(&browse.current_path) {
                "" => "/".to_string(),
                name => ellipsize(name),
            },
            ViewKind::Downloaded => "Local Files".to_string(),
            ViewKind::Preview(preview) => match last_segment(&preview.current_path) {
                "" => "Preview".to_string(),
                name => ellipsize(name),
            },
        }
    }
}

/// Последний непустой сегмент пути — им подписана вкладка.
fn last_segment(path: &str) -> &str {
    path.split('/').filter(|part| !part.is_empty()).next_back().unwrap_or("")
}

/// Обрезает хвост, а не середину: имена снимков различаются как раз началом
/// (миссия, уровень, дата), и голова информативнее хвоста.
fn ellipsize(name: &str) -> String {
    if name.chars().count() <= TITLE_LIMIT {
        return name.to_string();
    }
    let head: String = name.chars().take(TITLE_LIMIT - 1).collect();
    format!("{}…", head)
}
