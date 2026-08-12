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

/// Имена методов. Каждым пользуются ровно две ветви — encode и decode, — и
/// написанные там литералами они расходились бы опечаткой молча: нажатие
/// превращалось бы в «непонятное сообщение разметки» в логе, а не в ошибку
/// компиляции. Константа связывает пару.
mod method {
    pub const TAB_SELECT: &str = "tab_select";
    pub const TAB_CLOSE: &str = "tab_close";
    pub const TAB_MENU: &str = "tab_menu";
    pub const NEW_BROWSE: &str = "new_browse";
    pub const NEW_SEARCH: &str = "new_search";
    pub const NEW_DOWNLOADED: &str = "new_downloaded";
    pub const NEW_GLOBE: &str = "new_globe";
    pub const OPEN_MENU: &str = "open_menu";
    pub const FILTER: &str = "filter";
    pub const GROUP: &str = "group";
    pub const SORT: &str = "sort";
    pub const QUERY: &str = "query";
    pub const PAGE: &str = "page";
    pub const SEARCH_QUERY: &str = "search_query";
    pub const SEARCH_MISSION: &str = "search_mission";
    pub const SEARCH_PERIOD: &str = "search_period";
    pub const SEARCH_CLOUD: &str = "search_cloud";
    pub const RUN_SEARCH: &str = "run_search";
    pub const ENTER: &str = "enter";
    pub const UP: &str = "up";
    pub const DOWNLOAD: &str = "download";
    pub const DELETE: &str = "delete";
    pub const PREVIEW: &str = "preview";
    pub const PREVIEW_REMOTE: &str = "preview_remote";
    pub const ZOOM: &str = "zoom";
    pub const GLOBE_RESIZED: &str = "globe_resized";
    pub const GLOBE_POINTER: &str = "globe_pointer";
    pub const GLOBE_SHOW: &str = "globe_show";
}

impl UiMessage for Msg {
    fn encode(&self) -> (String, String) {
        let (method, value) = match self {
            Msg::TabSelect(id) => (method::TAB_SELECT, id.to_string()),
            Msg::TabClose(id) => (method::TAB_CLOSE, id.to_string()),
            Msg::TabMenu(open) => (method::TAB_MENU, open.to_string()),
            Msg::NewBrowse => (method::NEW_BROWSE, String::new()),
            Msg::NewSearch => (method::NEW_SEARCH, String::new()),
            Msg::NewDownloaded => (method::NEW_DOWNLOADED, String::new()),
            Msg::NewGlobe => (method::NEW_GLOBE, String::new()),
            Msg::OpenMenu(menu) => (method::OPEN_MENU, menu.key()),
            Msg::Filter(filter) => (method::FILTER, filter.key().to_string()),
            Msg::Group(grouping) => (method::GROUP, grouping.key().to_string()),
            Msg::Sort(sorting) => (method::SORT, sorting.key().to_string()),
            Msg::Query(query) => (method::QUERY, query.clone()),
            Msg::Page(page) => (method::PAGE, page.to_string()),
            Msg::SearchQuery(query) => (method::SEARCH_QUERY, query.clone()),
            Msg::SearchMission(mission) => (method::SEARCH_MISSION, mission.key().to_string()),
            Msg::SearchPeriod(period) => (method::SEARCH_PERIOD, period.key().to_string()),
            Msg::SearchCloud(cloud) => (method::SEARCH_CLOUD, cloud.key().to_string()),
            Msg::RunSearch => (method::RUN_SEARCH, String::new()),
            Msg::Enter(path) => (method::ENTER, path.clone()),
            Msg::Up => (method::UP, String::new()),
            Msg::Download(identifier) => (method::DOWNLOAD, identifier.clone()),
            Msg::Delete(name) => (method::DELETE, name.clone()),
            Msg::Preview(name) => (method::PREVIEW, name.clone()),
            Msg::PreviewRemote(identifier) => (method::PREVIEW_REMOTE, identifier.clone()),
            Msg::Zoom(zoom) => (method::ZOOM, zoom.to_string()),
            // Значение здесь не едет вовсе: разметка объявляет только имя, а
            // нагрузку подставит рендерер.
            Msg::GlobeResized(_) => (method::GLOBE_RESIZED, String::new()),
            Msg::GlobePointer(_) => (method::GLOBE_POINTER, String::new()),
            Msg::GlobeShow(identifier) => (method::GLOBE_SHOW, identifier.clone()),
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
            method::TAB_SELECT => Msg::TabSelect(value.parse().ok()?),
            method::TAB_CLOSE => Msg::TabClose(value.parse().ok()?),
            method::TAB_MENU => Msg::TabMenu(value == "true"),
            method::NEW_BROWSE => Msg::NewBrowse,
            method::NEW_SEARCH => Msg::NewSearch,
            method::NEW_DOWNLOADED => Msg::NewDownloaded,
            method::NEW_GLOBE => Msg::NewGlobe,
            method::OPEN_MENU => Msg::OpenMenu(Menu::from_key(value)),
            method::FILTER => Msg::Filter(Filter::from_key(value)?),
            method::GROUP => Msg::Group(Grouping::from_key(value)?),
            method::SORT => Msg::Sort(Sorting::from_key(value)?),
            method::QUERY => Msg::Query(value.to_string()),
            method::PAGE => Msg::Page(value.parse().ok()?),
            method::SEARCH_QUERY => Msg::SearchQuery(value.to_string()),
            method::SEARCH_MISSION => Msg::SearchMission(Mission::from_key(value)?),
            method::SEARCH_PERIOD => Msg::SearchPeriod(Period::from_key(value)?),
            method::SEARCH_CLOUD => Msg::SearchCloud(Cloud::from_key(value)?),
            method::RUN_SEARCH => Msg::RunSearch,
            method::ENTER => Msg::Enter(value.to_string()),
            method::UP => Msg::Up,
            method::DOWNLOAD => Msg::Download(value.to_string()),
            method::DELETE => Msg::Delete(value.to_string()),
            method::PREVIEW => Msg::Preview(value.to_string()),
            method::PREVIEW_REMOTE => Msg::PreviewRemote(value.to_string()),
            method::ZOOM => Msg::Zoom(value.parse().ok()?),
            method::GLOBE_RESIZED => Msg::GlobeResized(event.size()?.clone()),
            method::GLOBE_POINTER => Msg::GlobePointer(event.pointer()?.clone()),
            method::GLOBE_SHOW => Msg::GlobeShow(value.to_string()),
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::ui_service::ui_event_response::Payload as Wire;

    /// Ответ ровно той формы, в какой его собирает ui-service: имя метода из
    /// encode и строковая нагрузка.
    fn wire(method: String, value: String) -> UiEventResponse {
        UiEventResponse { method, payload: Some(Wire::Value(value)) }
    }

    /// Каждый вариант обязан пережить полный круг: encode → шина → decode →
    /// encode без изменений. Ровно тот класс расхождений двух match'ей, ради
    /// которого заведены константы `method`.
    #[test]
    fn every_variant_survives_the_wire() {
        let id: ViewId = "7".parse().expect("ViewId из строки");
        let mut messages = vec![
            Msg::TabSelect(id),
            Msg::TabClose(id),
            Msg::TabMenu(true),
            Msg::NewBrowse,
            Msg::NewSearch,
            Msg::NewDownloaded,
            Msg::NewGlobe,
            Msg::OpenMenu(Menu::Closed),
            Msg::Query("s2a".into()),
            Msg::Page(3),
            Msg::SearchQuery("msil2a".into()),
            Msg::RunSearch,
            Msg::Enter("eodata/Sentinel-2/".into()),
            Msg::Up,
            Msg::Download("продукт".into()),
            Msg::Delete("запись".into()),
            Msg::Preview("запись".into()),
            Msg::PreviewRemote("продукт".into()),
            Msg::Zoom(1.5),
            Msg::GlobeShow("продукт".into()),
        ];
        messages.extend(Filter::ALL.iter().map(|choice| Msg::Filter(*choice)));
        messages.extend(Grouping::ALL.iter().map(|choice| Msg::Group(*choice)));
        messages.extend(Sorting::ALL.iter().map(|choice| Msg::Sort(*choice)));
        messages.extend(Mission::ALL.iter().map(|choice| Msg::SearchMission(*choice)));
        messages.extend(Period::ALL.iter().map(|choice| Msg::SearchPeriod(*choice)));
        messages.extend(Cloud::ALL.iter().map(|choice| Msg::SearchCloud(*choice)));

        for message in messages {
            let (method, value) = message.encode();
            let decoded = Msg::decode(&wire(method.clone(), value.clone()))
                .unwrap_or_else(|| panic!("«{}» не разобрался", method));
            assert_eq!(decoded.encode(), (method, value));
        }
    }

    /// Нагрузку области подставляет рендерер — разбор этих двоих идёт не через
    /// строку, а через свой вид Payload.
    #[test]
    fn viewport_payloads_are_carried_whole() {
        use crate::proto::ui_service::{PointerEvent, ViewportSize};

        let size = ViewportSize { width: 800, height: 600 };
        let event = UiEventResponse {
            method: method::GLOBE_RESIZED.to_string(),
            payload: Some(Wire::Size(size.clone())),
        };
        assert!(matches!(Msg::decode(&event), Some(Msg::GlobeResized(carried)) if carried == size));

        let pointer = PointerEvent::default();
        let event = UiEventResponse {
            method: method::GLOBE_POINTER.to_string(),
            payload: Some(Wire::Pointer(pointer.clone())),
        };
        assert!(matches!(Msg::decode(&event), Some(Msg::GlobePointer(carried)) if carried == pointer));
    }

    /// Незнакомое имя — `None`: warn в module.rs, а не паника и не чужой смысл.
    #[test]
    fn unknown_method_is_none() {
        assert!(Msg::decode(&wire("неведомое".into(), String::new())).is_none());
    }
}
