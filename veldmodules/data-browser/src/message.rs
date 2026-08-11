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

use crate::module::state::listing::{Choice, Filter, Grouping, Menu, Sorting};
use crate::module::state::ViewId;
use veld_ui_service_wrap::UiMessage;

pub enum Msg {
    // -- Вкладки --
    /// Вкладка адресуется идентификатором, а не позицией: позиция меняется,
    /// когда закрывают соседа.
    TabSelect(ViewId),
    TabClose(ViewId),
    /// Меню «плюса» в полосе вкладок: оно не принадлежит списку, поэтому и
    /// открывается своим сообщением.
    TabMenu(bool),
    NewBrowse,
    NewSearch,
    NewDownloaded,

    // -- Показ списка --
    /// Раскрыть меню или закрыть открытое (`Menu::Closed`).
    OpenMenu(Menu),
    Filter(Filter),
    Group(Grouping),
    Sort(Sorting),
    /// Набранное в поле фильтра — его подставляет рендерер, а не разметка.
    Query(String),
    Page(usize),

    // -- Каталог --
    /// Перейти в папку по ключу провайдера; пустой ключ — перечитать текущую.
    Enter(String),
    Up,

    // -- Записи --
    /// Скачать, докачать или приостановить — по ключу провайдера.
    Download(String),
    /// Отменить закачку — по имени записи библиотеки.
    Cancel(String),
    Delete(String),
    /// Смотреть скачанное — по имени записи библиотеки.
    Preview(String),
    /// Смотреть ещё не скачанное — по ключу провайдера.
    PreviewRemote(String),
    /// Масштаб показа снимка: 0 — вписать в окно.
    Zoom(f32),
}

impl UiMessage for Msg {
    fn encode(&self) -> (String, String) {
        let (method, value) = match self {
            Msg::TabSelect(id) => ("tab_select", id.to_string()),
            Msg::TabClose(id) => ("tab_close", id.to_string()),
            Msg::TabMenu(open) => ("tab_menu", open.to_string()),
            Msg::NewBrowse => ("new_browse", String::new()),
            Msg::NewSearch => ("new_search", String::new()),
            Msg::NewDownloaded => ("new_downloaded", String::new()),
            Msg::OpenMenu(menu) => ("open_menu", menu.key()),
            Msg::Filter(filter) => ("filter", filter.key().to_string()),
            Msg::Group(grouping) => ("group", grouping.key().to_string()),
            Msg::Sort(sorting) => ("sort", sorting.key().to_string()),
            Msg::Query(query) => ("query", query.clone()),
            Msg::Page(page) => ("page", page.to_string()),
            Msg::Enter(path) => ("enter", path.clone()),
            Msg::Up => ("up", String::new()),
            Msg::Download(identifier) => ("download", identifier.clone()),
            Msg::Cancel(name) => ("cancel", name.clone()),
            Msg::Delete(name) => ("delete", name.clone()),
            Msg::Preview(name) => ("preview", name.clone()),
            Msg::PreviewRemote(identifier) => ("preview_remote", identifier.clone()),
            Msg::Zoom(zoom) => ("zoom", zoom.to_string()),
        };
        (method.to_string(), value)
    }

    fn decode(method: &str, value: &str) -> Option<Self> {
        Some(match method {
            // Разбор идентификатора вкладки — здесь и только здесь: обратно он
            // приезжает строкой, и место, где строка снова становится ViewId,
            // должно быть одно.
            "tab_select" => Msg::TabSelect(value.parse().ok()?),
            "tab_close" => Msg::TabClose(value.parse().ok()?),
            "tab_menu" => Msg::TabMenu(value == "true"),
            "new_browse" => Msg::NewBrowse,
            "new_search" => Msg::NewSearch,
            "new_downloaded" => Msg::NewDownloaded,
            "open_menu" => Msg::OpenMenu(Menu::from_key(value)),
            "filter" => Msg::Filter(Filter::from_key(value)?),
            "group" => Msg::Group(Grouping::from_key(value)?),
            "sort" => Msg::Sort(Sorting::from_key(value)?),
            "query" => Msg::Query(value.to_string()),
            "page" => Msg::Page(value.parse().ok()?),
            "enter" => Msg::Enter(value.to_string()),
            "up" => Msg::Up,
            "download" => Msg::Download(value.to_string()),
            "cancel" => Msg::Cancel(value.to_string()),
            "delete" => Msg::Delete(value.to_string()),
            "preview" => Msg::Preview(value.to_string()),
            "preview_remote" => Msg::PreviewRemote(value.to_string()),
            "zoom" => Msg::Zoom(value.parse().ok()?),
            _ => return None,
        })
    }
}
