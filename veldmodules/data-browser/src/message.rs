//! Сообщения разметки: что виджет говорит модулю.
//!
//! Один тип на все нажатия и весь ввод. Отправляет его view (`on_press`,
//! `on_input`), принимает `module::on_ui_event` исчерпывающим match'ем — то
//! есть завести вариант и забыть его обработать нельзя, как нельзя и обработать
//! несуществующий.
//!
//! По шине едет пара строк: имя метода и значение (см. `UiMessage`). Имена —
//! приватная проводка модуля: ui-service возвращает их эхом, смысла их не зная
//! и не проверяя.

use crate::module::state::ViewId;
use veld_ui_service_wrap::UiMessage;

pub enum Msg {
    /// Кнопки шапки: показать уже открытый вид такого рода или завести его.
    NavBrowse,
    NavSearch,
    NavDownloaded,
    /// Вкладка адресуется идентификатором, а не позицией: позиция меняется,
    /// когда закрывают соседа.
    TabSelect(ViewId),
    TabClose(ViewId),
    /// Перейти в папку по ключу провайдера; пустой ключ — перечитать текущую.
    Browse(String),
    BrowseUp,
    Search,
    /// Набранный текст — его подставляет рендерер, а не разметка.
    SearchInput(String),
    /// Скачать или приостановить — по ключу провайдера.
    Download(String),
    /// Смотреть скачанное — по имени записи библиотеки.
    ViewLocal(String),
    /// Смотреть ещё не скачанное — по ключу провайдера.
    ViewRemote(String),
    /// Удалить запись библиотеки — по её имени.
    Delete(String),
}

impl UiMessage for Msg {
    fn encode(&self) -> (String, String) {
        let (method, value) = match self {
            Msg::NavBrowse => ("nav_browse", String::new()),
            Msg::NavSearch => ("nav_search", String::new()),
            Msg::NavDownloaded => ("nav_downloaded", String::new()),
            Msg::TabSelect(id) => ("tab_select", id.to_string()),
            Msg::TabClose(id) => ("tab_close", id.to_string()),
            Msg::Browse(path) => ("browse", path.clone()),
            Msg::BrowseUp => ("browse_up", String::new()),
            Msg::Search => ("search", String::new()),
            Msg::SearchInput(query) => ("search_input", query.clone()),
            Msg::Download(identifier) => ("download", identifier.clone()),
            Msg::ViewLocal(name) => ("view_local", name.clone()),
            Msg::ViewRemote(identifier) => ("view_remote", identifier.clone()),
            Msg::Delete(name) => ("delete", name.clone()),
        };
        (method.to_string(), value)
    }

    fn decode(method: &str, value: &str) -> Option<Self> {
        Some(match method {
            "nav_browse" => Msg::NavBrowse,
            "nav_search" => Msg::NavSearch,
            "nav_downloaded" => Msg::NavDownloaded,
            // Разбор идентификатора вкладки — здесь и только здесь: обратно он
            // приезжает строкой, и место, где строка снова становится ViewId,
            // должно быть одно.
            "tab_select" => Msg::TabSelect(value.parse().ok()?),
            "tab_close" => Msg::TabClose(value.parse().ok()?),
            "browse" => Msg::Browse(value.to_string()),
            "browse_up" => Msg::BrowseUp,
            "search" => Msg::Search,
            "search_input" => Msg::SearchInput(value.to_string()),
            "download" => Msg::Download(value.to_string()),
            "view_local" => Msg::ViewLocal(value.to_string()),
            "view_remote" => Msg::ViewRemote(value.to_string()),
            "delete" => Msg::Delete(value.to_string()),
            _ => return None,
        })
    }
}
