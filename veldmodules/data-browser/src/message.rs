//! Сообщения разметки: что виджет говорит модулю.
//!
//! Один тип на все нажатия и весь ввод. Отправляет его view (`on_press`,
//! `on_input`), принимает `module::on_ui_event` исчерпывающим match'ем — то
//! есть завести вариант и забыть его обработать нельзя, как нельзя и обработать
//! несуществующий.
//!
//! По шине едет имя метода и нагрузка (см. `UiMessage`). Имена — приватная
//! проводка модуля: ui-service возвращает их эхом, смысла их не зная и не
//! проверяя. Нагрузка у большинства сообщений — та строка, которую назвала сама
//! разметка; у ввода и у области её подставляет рендерер.

use crate::module::state::listing::{Choice, Filter, Grouping, Menu, Sorting};
use crate::module::state::search::{Cloud, Mission, Period};
use crate::module::state::ViewId;
use crate::proto::ui_service::{PointerEvent, UiEventResponse, ViewportSize};
use veld_ui_service_wrap::{Payload, UiMessage};

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
    NewGlobe,

    // -- Показ списка --
    /// Раскрыть меню или закрыть открытое (`Menu::Closed`).
    OpenMenu(Menu),
    Filter(Filter),
    Group(Grouping),
    Sort(Sorting),
    /// Набранное в поле фильтра — его подставляет рендерер, а не разметка.
    Query(String),
    Page(usize),

    // -- Поиск по каталогу --
    //
    // Отбор в списке (`Query`) и запрос к каталогу — разные вещи, и путать их
    // нельзя: первый сужает уже найденное, второй идёт по сети и меняет то,
    // что вообще нашлось.
    /// Набранное в поле запроса.
    SearchQuery(String),
    /// Чем сузить запрос. Каждое из трёх отправляет его заново: выбор сделан
    /// одним нажатием, и спрашивать после него ещё и подтверждения не за что.
    SearchMission(Mission),
    SearchPeriod(Period),
    SearchCloud(Cloud),
    /// Отправить запрос каталогу.
    RunSearch,

    // -- Каталог --
    /// Перейти в папку по ключу провайдера; пустой ключ — перечитать текущую.
    Enter(String),
    Up,

    // -- Записи --
    /// Скачать, докачать или приостановить — по ключу провайдера.
    Download(String),
    /// Выбросить запись: удалить скачанное или отказаться от начатого. Одно
    /// сообщение на оба, потому что оставляют они после себя одно и то же —
    /// ничего; разными их делает только подпись в меню.
    Delete(String),
    /// Смотреть скачанное — по имени записи библиотеки.
    Preview(String),
    /// Смотреть ещё не скачанное — по ключу провайдера.
    PreviewRemote(String),
    /// Масштаб показа снимка: 0 — вписать в окно.
    Zoom(f32),

    // -- Глобус --
    //
    // Нагрузку у обоих подставляет рендерер, поэтому в разметке они объявлены
    // конструкторами, а не готовыми значениями (см. `viewport`).
    /// Области под шар досталось новое место — в пикселях её текстуры.
    GlobeResized(ViewportSize),
    /// Указатель над областью, в тех же пикселях.
    GlobePointer(PointerEvent),
    /// Показать снимок на шаре — по ключу провайдера. Единственное сообщение
    /// глобуса, у которого нагрузка своя: приходит оно не от области, а из
    /// меню строки списка.
    GlobeShow(String),
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
            Msg::NewGlobe => ("new_globe", String::new()),
            Msg::OpenMenu(menu) => ("open_menu", menu.key()),
            Msg::Filter(filter) => ("filter", filter.key().to_string()),
            Msg::Group(grouping) => ("group", grouping.key().to_string()),
            Msg::Sort(sorting) => ("sort", sorting.key().to_string()),
            Msg::Query(query) => ("query", query.clone()),
            Msg::Page(page) => ("page", page.to_string()),
            Msg::SearchQuery(query) => ("search_query", query.clone()),
            Msg::SearchMission(mission) => ("search_mission", mission.key().to_string()),
            Msg::SearchPeriod(period) => ("search_period", period.key().to_string()),
            Msg::SearchCloud(cloud) => ("search_cloud", cloud.key().to_string()),
            Msg::RunSearch => ("run_search", String::new()),
            Msg::Enter(path) => ("enter", path.clone()),
            Msg::Up => ("up", String::new()),
            Msg::Download(identifier) => ("download", identifier.clone()),
            Msg::Delete(name) => ("delete", name.clone()),
            Msg::Preview(name) => ("preview", name.clone()),
            Msg::PreviewRemote(identifier) => ("preview_remote", identifier.clone()),
            Msg::Zoom(zoom) => ("zoom", zoom.to_string()),
            // Значение здесь не едет вовсе: разметка объявляет только имя, а
            // нагрузку подставит рендерер.
            Msg::GlobeResized(_) => ("globe_resized", String::new()),
            Msg::GlobePointer(_) => ("globe_pointer", String::new()),
            Msg::GlobeShow(identifier) => ("globe_show", identifier.clone()),
        };
        (method.to_string(), value)
    }

    fn decode(event: &UiEventResponse) -> Option<Self> {
        // Строковая нагрузка нужна почти всем, и берётся она один раз: у
        // события другого вида она пуста, и разбор такого значения сам вернёт
        // `None` — второй проверки на вид нагрузки не нужно.
        let value = event.value();
        Some(match event.method.as_str() {
            // Разбор идентификатора вкладки — здесь и только здесь: обратно он
            // приезжает строкой, и место, где строка снова становится ViewId,
            // должно быть одно.
            "tab_select" => Msg::TabSelect(value.parse().ok()?),
            "tab_close" => Msg::TabClose(value.parse().ok()?),
            "tab_menu" => Msg::TabMenu(value == "true"),
            "new_browse" => Msg::NewBrowse,
            "new_search" => Msg::NewSearch,
            "new_downloaded" => Msg::NewDownloaded,
            "new_globe" => Msg::NewGlobe,
            "open_menu" => Msg::OpenMenu(Menu::from_key(value)),
            "filter" => Msg::Filter(Filter::from_key(value)?),
            "group" => Msg::Group(Grouping::from_key(value)?),
            "sort" => Msg::Sort(Sorting::from_key(value)?),
            "query" => Msg::Query(value.to_string()),
            "page" => Msg::Page(value.parse().ok()?),
            "search_query" => Msg::SearchQuery(value.to_string()),
            "search_mission" => Msg::SearchMission(Mission::from_key(value)?),
            "search_period" => Msg::SearchPeriod(Period::from_key(value)?),
            "search_cloud" => Msg::SearchCloud(Cloud::from_key(value)?),
            "run_search" => Msg::RunSearch,
            "enter" => Msg::Enter(value.to_string()),
            "up" => Msg::Up,
            "download" => Msg::Download(value.to_string()),
            "delete" => Msg::Delete(value.to_string()),
            "preview" => Msg::Preview(value.to_string()),
            "preview_remote" => Msg::PreviewRemote(value.to_string()),
            "zoom" => Msg::Zoom(value.parse().ok()?),
            "globe_resized" => Msg::GlobeResized(event.size()?.clone()),
            "globe_pointer" => Msg::GlobePointer(event.pointer()?.clone()),
            "globe_show" => Msg::GlobeShow(value.to_string()),
            _ => return None,
        })
    }
}
