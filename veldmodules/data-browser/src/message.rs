//! Сообщения разметки: что виджет говорит модулю.
//!
//! Один тип на все нажатия и весь ввод. Отправляет его view (`on_press`,
//! `on_input`), принимает `module::on_ui_event` исчерпывающим match'ем — то
//! есть завести вариант и забыть его обработать нельзя, как нельзя и обработать
//! несуществующий.
//!
//! По шине едет имя метода, нагрузка и адресат (см. `UiMessage`). Имена —
//! приватная проводка модуля: ui-service возвращает их эхом, смысла их не зная
//! и не проверяя.
//!
//! **Адресат — отдельно от нагрузки.** Панелей на экране сколько угодно, и
//! «активный вид» на вопрос «чей это щелчок» не отвечает: щёлкнуть могли в той
//! панели, что не под рукой. Поэтому всё, что рождается в теле вкладки, едет обёрнутым в
//! [`Msg::In`], а вкладка называет себя полем `Handler.key` — тем самым, которое
//! не подменяет рендерер. Сложить её в нагрузку было бы нельзя: у поля ввода,
//! области и ползунка нагрузку изготавливает он.

use crate::module::state::listing::{choice, Choice, Filter, Grouping, Menu, Sorting};
use crate::module::state::search::{Cloud, Mission, Period};
use crate::module::state::{PaneId, Shift, Side, SplitId, ViewId};
use crate::proto::ui_service::{DropEvent, PointerEvent, UiEventResponse, ViewportSize};
use veld_ui_service_wrap::{Payload, UiMessage};

choice! {
    /// Что можно открыть новой вкладкой. Один вариант вместо пяти сообщений:
    /// различаются они только тем, какой вид завести, а панель, куда его
    /// класть, — у всех одна и та же забота.
    ///
    /// Порядок — порядок пунктов «плюса». Пустая вкладка последней: она не про
    /// то, что смотреть, а про место, где это решат потом.
    NewTab {
        Browse     = "browse",     "Сетевой каталог";
        Search     = "search",     "Поиск снимков";
        Downloaded = "downloaded", "Скачанное";
        Globe      = "globe",      "Глобус";
        Shown      = "shown",      "На просмотре";
        /// Вкладка, которая ещё ничего не показывает (см. `ViewKind::Empty`).
        Empty      = "empty",      "Пустая вкладка";
    }
}

impl NewTab {
    /// Чем наполняют пустую вкладку — та же пятёрка без неё самой: пустое
    /// пустым не наполняют.
    pub const KINDS: [NewTab; 5] =
        [NewTab::Browse, NewTab::Search, NewTab::Downloaded, NewTab::Globe, NewTab::Shown];
}

pub enum Msg {
    // -- Вкладки --
    //
    // Вкладка адресуется идентификатором, а не позицией: позиция меняется,
    // когда закрывают соседа.
    TabSelect(ViewId),
    TabClose(ViewId),
    /// Меню «плюса» названной панели; `None` — закрыть раскрытое.
    TabMenu(Option<PaneId>),
    /// Меню самой вкладки: перенести, закрыть. `None` — закрыть.
    TabOptions(Option<ViewId>),
    /// Завести вкладку в названной панели.
    NewTab(PaneId, NewTab),
    /// Перенести вкладку туда, где человек видит эту сторону: в соседнюю
    /// панель либо в заведённую с этой стороны (см. `State::move_aside`).
    TabMove(ViewId, Side),
    /// Свести все панели в одну.
    TabCollapse,
    /// Границу деления потянули: сдвиг в точках разметки. Что это в долях,
    /// знает только тот, кто их задавал (см. `State::divide`).
    Divide(SplitId, f32),
    /// Границу отпустили. Отдельным сообщением, а не концом потока сдвигов:
    /// «перетаскивание кончилось» — момент, а не отсутствие событий, и ждать
    /// его молчанием значило бы не дождаться вовсе.
    Divided,
    /// В панель бросили вкладку. Что принесли и в какой край панели попали,
    /// изготавливает рендерер: где какая зона, знает только он (см. `Drop` в
    /// ui-service/types.proto). Нагрузка едет как есть — разбирает её
    /// обработчик, ровно как у указателя над областью.
    TabDrop(PaneId, DropEvent),

    // -- Записи --
    //
    // Действуют не на вид, а на библиотеку, поэтому вкладки-источника не несут:
    // скачанное одно на всё окно, из какой панели его ни попроси.
    /// Скачать, докачать или приостановить — по ключу провайдера. Второе поле —
    /// снимок, к которому файл относится (пусто — сам по себе): библиотека
    /// пишет его в свой снимок, а вывести это из ключа не может.
    Download(String, String),
    /// Выбросить запись: удалить скачанное или отказаться от начатого. Одно
    /// сообщение на оба, потому что оставляют они после себя одно и то же —
    /// ничего; разными их делает только подпись в меню.
    Delete(String),
    /// Выбросить снимок целиком — все его файлы. Ключом здесь снимок, а не
    /// запись: «снимок» — понятие показа (библиотека ведёт учёт файлам), и
    /// разворачивает его в записи тот, кто его и собрал (см.
    /// `handlers::library::on_delete_snapshot`).
    DeleteSnapshot(String),
    /// Скачать снимок целиком — по ключу продукта. Список файлов приезжает
    /// рекурсивным листингом от провайдера: раскладку .SAFE знает только он, а
    /// закачка идёт по одному файлу (см. `handlers::library::on_download_snapshot`).
    DownloadSnapshot(String),
    /// Приостановить закачку снимка целиком. Ключом снимок — по той же
    /// причине, что и у выброса: разворачивает его в файлы тот, кто его собрал.
    PauseSnapshot(String),
    /// Показать запись в файловом менеджере. Путь считает библиотека — раскладка
    /// хранения её, — а показывает платформа.
    Reveal(String),

    // -- Шар --
    /// Снять с шара всё: и наложения, и контуры. Порознь их не снять ничем, а
    /// разделять их пользователю не по чему — он видит один шар.
    GlobeClear,

    // -- На просмотре --
    //
    // Слой адресуется ключом продукта, и едет он тем же полем, что вкладка у
    // `In`: вопрос у них один — «с кем», — и отвечать на него двумя способами
    // было бы незачем.
    /// Прозрачность слоя, 0..1.
    OverlayOpacity(String, f32),
    /// Скрыть слой или вернуть его на шар.
    OverlayHidden(String, bool),
    /// Убрать слой совсем: ресурсы отпускаются, вернуть его — это открыть
    /// снимок заново.
    OverlayRemove(String),
    /// Подвинуть слой в наборе: порядок набора — это порядок слоёв на шаре
    /// снизу вверх, и другого способа сказать «этот поверх того» нет.
    OverlayShift(String, Shift),
    /// Скрыть или показать все слои разом.
    OverlayHideAll(bool),
    /// Меню слоя: порядок и переходы к снимку. `None` — закрыть раскрытое.
    OverlayMenu(Option<String>),

    // -- Контуры --
    //
    // Контур адресуется тем же ключом снимка, что и слой, и тем же полем: на
    // шаре они про один и тот же снимок, просто с разной подробностью.
    /// Убрать контур: снять отметку во всех списках сразу.
    OutlineRemove(String),
    /// Навести шар на контур и выбрать его.
    OutlineFocus(String),

    /// Сообщение, рождённое в теле названной вкладки.
    In(ViewId, ViewMsg),
}

/// То, что говорит виджет внутри вкладки. Отдельный тип, а не два десятка
/// вариантов с `ViewId` первым полем: адресат у них общий и приписывается один
/// раз — в [`Msg::In`].
pub enum ViewMsg {
    /// Чем наполнить пустую вкладку. Ей, а не панели: вкладка становится
    /// выбранным видом на месте, не заводя рядом вторую и не оставляя после
    /// себя пустую (см. `handlers::nav::fill`).
    Fill(NewTab),

    // -- Показ списка --
    /// Раскрыть меню или закрыть раскрытое (`None`).
    OpenMenu(Option<Menu>),
    Filter(Filter),
    Group(Grouping),
    Sort(Sorting),
    /// Набранное в поле фильтра — его подставляет рендерер, а не разметка.
    Query(String),
    Page(usize),
    /// Раскрыть строку в её содержимое или свернуть обратно.
    Expand(String),
    /// Отметить снимок или снять отметку: отмеченные очерчены на шаре.
    Check(String),
    /// Отметить или снять отметку разом со всего, что показано, — коробочка в
    /// шапке. Действует ровно на то, о чём говорит: на снимки этой страницы.
    CheckShown(bool),
    /// Снять все отметки списка, включая сделанные в другой папке, — кнопка в
    /// заголовке. Отдельное сообщение, а не `CheckShown(false)`: отметка
    /// переживает переход по папкам, и «снять видимое» оставило бы контуры,
    /// убрать которые стало бы нечем.
    CheckClear,

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
    /// Края своего интервала — набранное в полях дат (см. `Period::Custom`).
    SearchFrom(String),
    SearchTo(String),
    SearchCloud(Cloud),
    /// Отправить запрос каталогу.
    RunSearch,

    // -- Каталог --
    /// Перейти в папку по ключу провайдера; пустой ключ — перечитать текущую.
    Enter(String),
    Up,
    /// Показать названную запись в каталоге: открыть папку, в которой она
    /// лежит, встать на её страницу и подсветить её.
    ///
    /// Отдельно от [`ViewMsg::Enter`], потому что вопрос другой: тот называет
    /// папку и ведёт в неё, этот называет запись и ведёт к ней. Каталог при
    /// этом переиспользуется — новую вкладку заводит только тот, кому её
    /// попросили завести (см. `handlers::nav::catalog`).
    InCatalog(String),

    // -- Просмотр снимка --
    //
    // Открывают новую вкладку — в той же половине, где стоит строка: смотреть
    // рядом со списком и есть то, ради чего экран делят.
    /// Смотреть скачанное — по имени записи библиотеки.
    Preview(String),
    /// Смотреть ещё не скачанное — по ключу провайдера.
    PreviewRemote(String),
    /// Показать снимок, лежащий папкой: растр внутри выбирает провайдер
    /// (см. `handlers::preview::on_view_product_pressed`).
    PreviewProduct(String),

    // -- Канва превью --
    //
    // Нагрузку области подставляет рендерер (как у глобуса); кнопки тулбара
    // едут своими сообщениями.
    /// Области под кадр досталось новое место — в пикселях её текстуры.
    PreviewResized(ViewportSize),
    /// Указатель над канвой, в тех же пикселях.
    PreviewPointer(PointerEvent),
    /// Вписать снимок в канву.
    PreviewFit,
    /// Шаг масштаба вокруг центра: знак — направление.
    PreviewZoom(f32),

    // -- Глобус --
    //
    // Нагрузку у обоих подставляет рендерер, поэтому в разметке они объявлены
    // конструкторами, а не готовыми значениями (см. `viewport`).
    /// Области под шар досталось новое место — в пикселях её текстуры.
    GlobeResized(ViewportSize),
    /// Указатель над областью, в тех же пикселях.
    GlobePointer(PointerEvent),
    /// Положить снимок на шар — по ключу провайдера. Нагрузка своя: приходит
    /// не от области, а из строки списка.
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
    pub const TAB_OPTIONS: &str = "tab_options";
    pub const NEW_TAB: &str = "new_tab";
    pub const TAB_MOVE: &str = "tab_move";
    pub const TAB_COLLAPSE: &str = "tab_collapse";
    pub const DIVIDE: &str = "divide";
    pub const DIVIDED: &str = "divided";
    pub const TAB_DROP: &str = "tab_drop";
    pub const DOWNLOAD: &str = "download";
    pub const DELETE: &str = "delete";
    pub const DELETE_SNAPSHOT: &str = "delete_snapshot";
    pub const DOWNLOAD_SNAPSHOT: &str = "download_snapshot";
    pub const PAUSE_SNAPSHOT: &str = "pause_snapshot";
    pub const REVEAL: &str = "reveal";
    pub const GLOBE_CLEAR: &str = "globe_clear";
    pub const OVERLAY_OPACITY: &str = "overlay_opacity";
    pub const OVERLAY_HIDDEN: &str = "overlay_hidden";
    pub const OVERLAY_REMOVE: &str = "overlay_remove";
    pub const OVERLAY_SHIFT: &str = "overlay_shift";
    pub const OVERLAY_HIDE_ALL: &str = "overlay_hide_all";
    pub const OVERLAY_MENU: &str = "overlay_menu";
    pub const OUTLINE_REMOVE: &str = "outline_remove";
    pub const OUTLINE_FOCUS: &str = "outline_focus";

    pub const FILL: &str = "fill";
    pub const OPEN_MENU: &str = "open_menu";
    pub const FILTER: &str = "filter";
    pub const GROUP: &str = "group";
    pub const SORT: &str = "sort";
    pub const QUERY: &str = "query";
    pub const PAGE: &str = "page";
    pub const EXPAND: &str = "expand";
    pub const CHECK: &str = "check";
    pub const CHECK_SHOWN: &str = "check_shown";
    pub const CHECK_CLEAR: &str = "check_clear";
    pub const SEARCH_QUERY: &str = "search_query";
    pub const SEARCH_MISSION: &str = "search_mission";
    pub const SEARCH_PERIOD: &str = "search_period";
    pub const SEARCH_FROM: &str = "search_from";
    pub const SEARCH_TO: &str = "search_to";
    pub const SEARCH_CLOUD: &str = "search_cloud";
    pub const RUN_SEARCH: &str = "run_search";
    pub const ENTER: &str = "enter";
    pub const UP: &str = "up";
    pub const IN_CATALOG: &str = "in_catalog";
    pub const PREVIEW: &str = "preview";
    pub const PREVIEW_REMOTE: &str = "preview_remote";
    pub const PREVIEW_PRODUCT: &str = "preview_product";
    pub const PREVIEW_RESIZED: &str = "preview_resized";
    pub const PREVIEW_POINTER: &str = "preview_pointer";
    pub const PREVIEW_FIT: &str = "preview_fit";
    pub const PREVIEW_ZOOM: &str = "preview_zoom";
    pub const GLOBE_RESIZED: &str = "globe_resized";
    pub const GLOBE_POINTER: &str = "globe_pointer";
    pub const GLOBE_SHOW: &str = "globe_show";
}

impl ViewMsg {
    fn encode(&self) -> (&'static str, String) {
        match self {
            ViewMsg::Fill(kind) => (method::FILL, kind.key().to_string()),
            ViewMsg::OpenMenu(menu) => {
                (method::OPEN_MENU, menu.as_ref().map(Menu::key).unwrap_or_default())
            }
            ViewMsg::Filter(filter) => (method::FILTER, filter.key().to_string()),
            ViewMsg::Group(grouping) => (method::GROUP, grouping.key().to_string()),
            ViewMsg::Sort(sorting) => (method::SORT, sorting.key().to_string()),
            ViewMsg::Query(query) => (method::QUERY, query.clone()),
            ViewMsg::Page(page) => (method::PAGE, page.to_string()),
            ViewMsg::Expand(key) => (method::EXPAND, key.clone()),
            ViewMsg::Check(key) => (method::CHECK, key.clone()),
            ViewMsg::CheckShown(on) => (method::CHECK_SHOWN, on.to_string()),
            ViewMsg::CheckClear => (method::CHECK_CLEAR, String::new()),
            ViewMsg::SearchQuery(query) => (method::SEARCH_QUERY, query.clone()),
            ViewMsg::SearchMission(mission) => (method::SEARCH_MISSION, mission.key().to_string()),
            ViewMsg::SearchPeriod(period) => (method::SEARCH_PERIOD, period.key().to_string()),
            ViewMsg::SearchFrom(value) => (method::SEARCH_FROM, value.clone()),
            ViewMsg::SearchTo(value) => (method::SEARCH_TO, value.clone()),
            ViewMsg::SearchCloud(cloud) => (method::SEARCH_CLOUD, cloud.key().to_string()),
            ViewMsg::RunSearch => (method::RUN_SEARCH, String::new()),
            ViewMsg::Enter(path) => (method::ENTER, path.clone()),
            ViewMsg::Up => (method::UP, String::new()),
            ViewMsg::InCatalog(key) => (method::IN_CATALOG, key.clone()),
            ViewMsg::Preview(name) => (method::PREVIEW, name.clone()),
            ViewMsg::PreviewRemote(identifier) => (method::PREVIEW_REMOTE, identifier.clone()),
            ViewMsg::PreviewProduct(identifier) => (method::PREVIEW_PRODUCT, identifier.clone()),
            // Нагрузку области подставляет рендерер — в разметке одни имена.
            ViewMsg::PreviewResized(_) => (method::PREVIEW_RESIZED, String::new()),
            ViewMsg::PreviewPointer(_) => (method::PREVIEW_POINTER, String::new()),
            ViewMsg::PreviewFit => (method::PREVIEW_FIT, String::new()),
            ViewMsg::PreviewZoom(direction) => (method::PREVIEW_ZOOM, direction.to_string()),
            ViewMsg::GlobeResized(_) => (method::GLOBE_RESIZED, String::new()),
            ViewMsg::GlobePointer(_) => (method::GLOBE_POINTER, String::new()),
            ViewMsg::GlobeShow(identifier) => (method::GLOBE_SHOW, identifier.clone()),
        }
    }

    /// `None` — метод не наш: это сообщение вкладке не принадлежит.
    fn decode(event: &UiEventResponse) -> Option<ViewMsg> {
        // Строковая нагрузка нужна почти всем, и берётся она один раз: у
        // события другого вида она пуста, и разбор такого значения сам вернёт
        // `None` — второй проверки на вид нагрузки не нужно.
        let value = event.value();
        Some(match event.method.as_str() {
            method::FILL => ViewMsg::Fill(NewTab::from_key(value)?),
            method::OPEN_MENU => ViewMsg::OpenMenu(Menu::from_key(value)),
            method::FILTER => ViewMsg::Filter(Filter::from_key(value)?),
            method::GROUP => ViewMsg::Group(Grouping::from_key(value)?),
            method::SORT => ViewMsg::Sort(Sorting::from_key(value)?),
            method::QUERY => ViewMsg::Query(value.to_string()),
            method::PAGE => ViewMsg::Page(value.parse().ok()?),
            method::EXPAND => ViewMsg::Expand(value.to_string()),
            method::CHECK => ViewMsg::Check(value.to_string()),
            method::CHECK_SHOWN => ViewMsg::CheckShown(value == "true"),
            method::CHECK_CLEAR => ViewMsg::CheckClear,
            method::SEARCH_QUERY => ViewMsg::SearchQuery(value.to_string()),
            method::SEARCH_MISSION => ViewMsg::SearchMission(Mission::from_key(value)?),
            method::SEARCH_PERIOD => ViewMsg::SearchPeriod(Period::from_key(value)?),
            method::SEARCH_FROM => ViewMsg::SearchFrom(value.to_string()),
            method::SEARCH_TO => ViewMsg::SearchTo(value.to_string()),
            method::SEARCH_CLOUD => ViewMsg::SearchCloud(Cloud::from_key(value)?),
            method::RUN_SEARCH => ViewMsg::RunSearch,
            method::ENTER => ViewMsg::Enter(value.to_string()),
            method::UP => ViewMsg::Up,
            method::IN_CATALOG => ViewMsg::InCatalog(value.to_string()),
            method::PREVIEW => ViewMsg::Preview(value.to_string()),
            method::PREVIEW_REMOTE => ViewMsg::PreviewRemote(value.to_string()),
            method::PREVIEW_PRODUCT => ViewMsg::PreviewProduct(value.to_string()),
            method::PREVIEW_RESIZED => ViewMsg::PreviewResized(event.size()?.clone()),
            method::PREVIEW_POINTER => ViewMsg::PreviewPointer(event.pointer()?.clone()),
            method::PREVIEW_FIT => ViewMsg::PreviewFit,
            method::PREVIEW_ZOOM => ViewMsg::PreviewZoom(value.parse().ok()?),
            method::GLOBE_RESIZED => ViewMsg::GlobeResized(event.size()?.clone()),
            method::GLOBE_POINTER => ViewMsg::GlobePointer(event.pointer()?.clone()),
            method::GLOBE_SHOW => ViewMsg::GlobeShow(value.to_string()),
            _ => return None,
        })
    }
}

impl UiMessage for Msg {
    fn encode(&self) -> (String, String) {
        let (method, value) = match self {
            Msg::TabSelect(_) => (method::TAB_SELECT, String::new()),
            Msg::TabClose(_) => (method::TAB_CLOSE, String::new()),
            Msg::TabMenu(_) => (method::TAB_MENU, String::new()),
            Msg::TabOptions(_) => (method::TAB_OPTIONS, String::new()),
            Msg::NewTab(_, kind) => (method::NEW_TAB, kind.key().to_string()),
            // Ключ занят вкладкой, поэтому сторона едет нагрузкой.
            Msg::TabMove(_, side) => (method::TAB_MOVE, side.key().to_string()),
            Msg::TabCollapse => (method::TAB_COLLAPSE, String::new()),
            // Сдвиг подставит рендерер, деление уедет ключом — сказать нечего.
            Msg::Divide(..) => (method::DIVIDE, String::new()),
            Msg::Divided => (method::DIVIDED, String::new()),
            // Принесённое подставит рендерер, панель уедет ключом.
            Msg::TabDrop(..) => (method::TAB_DROP, String::new()),
            Msg::Download(identifier, _) => (method::DOWNLOAD, identifier.clone()),
            Msg::Delete(name) => (method::DELETE, name.clone()),
            Msg::DeleteSnapshot(product) => (method::DELETE_SNAPSHOT, product.clone()),
            Msg::DownloadSnapshot(product) => (method::DOWNLOAD_SNAPSHOT, product.clone()),
            Msg::PauseSnapshot(product) => (method::PAUSE_SNAPSHOT, product.clone()),
            Msg::Reveal(name) => (method::REVEAL, name.clone()),
            Msg::GlobeClear => (method::GLOBE_CLEAR, String::new()),
            // Число подставит рендерер, слой уедет ключом — сказать нечего.
            Msg::OverlayOpacity(..) => (method::OVERLAY_OPACITY, String::new()),
            Msg::OverlayHidden(_, hidden) => (method::OVERLAY_HIDDEN, hidden.to_string()),
            Msg::OverlayRemove(_) => (method::OVERLAY_REMOVE, String::new()),
            // Ключ занят слоем, поэтому направление едет нагрузкой.
            Msg::OverlayShift(_, shift) => (method::OVERLAY_SHIFT, shift.key().to_string()),
            Msg::OverlayHideAll(hidden) => (method::OVERLAY_HIDE_ALL, hidden.to_string()),
            // Слой едет ключом — как у остальных сообщений про слой; сказать
            // нагрузкой нечего.
            Msg::OverlayMenu(_) => (method::OVERLAY_MENU, String::new()),
            Msg::OutlineRemove(_) => (method::OUTLINE_REMOVE, String::new()),
            Msg::OutlineFocus(_) => (method::OUTLINE_FOCUS, String::new()),
            Msg::In(_, view) => view.encode(),
        };
        (method.to_string(), value)
    }

    /// Кого адресует сообщение: вкладку, панель или слой на шаре.
    ///
    /// Одинаковых виджетов на экране столько же, сколько вкладок и снимков, и
    /// различить их по имени метода нельзя — а нагрузка у половины из них
    /// занята рендерером (см. `Handler.key` в ui-service/types.proto).
    fn key(&self) -> String {
        match self {
            Msg::TabSelect(id) | Msg::TabClose(id) | Msg::TabMove(id, _) | Msg::In(id, _) => {
                id.to_string()
            }
            Msg::TabOptions(id) => id.map(|id| id.to_string()).unwrap_or_default(),
            Msg::Divide(split, _) => split.to_string(),
            Msg::TabDrop(pane, _) => pane.to_string(),
            Msg::TabMenu(pane) => pane.map(|pane| pane.to_string()).unwrap_or_default(),
            Msg::NewTab(pane, _) => pane.to_string(),
            Msg::OverlayOpacity(key, _)
            | Msg::OverlayHidden(key, _)
            | Msg::OverlayRemove(key)
            | Msg::OverlayShift(key, _)
            | Msg::OutlineRemove(key)
            | Msg::OutlineFocus(key) => key.clone(),
            Msg::OverlayMenu(key) => key.clone().unwrap_or_default(),
            // Снимок едет ключом: нагрузку занял сам файл, а вопрос «к чему его
            // отнести» — это тот же вопрос «с кем», на который ключ и отвечает.
            Msg::Download(_, product) => product.clone(),
            _ => String::new(),
        }
    }

    fn decode(event: &UiEventResponse) -> Option<Self> {
        let value = event.value();
        // Видовые сообщения разбираются первыми и опознаются своим именем
        // метода; вкладку они называют ключом — иначе нагрузку у области и поля
        // ввода пришлось бы делить надвое с адресатом.
        if let Some(view) = ViewMsg::decode(event) {
            return Some(Msg::In(event.key.parse().ok()?, view));
        }
        Some(match event.method.as_str() {
            // Разбор идентификатора вкладки — здесь и только здесь: обратно он
            // приезжает строкой, и место, где строка снова становится ViewId,
            // должно быть одно.
            method::TAB_SELECT => Msg::TabSelect(event.key.parse().ok()?),
            method::TAB_CLOSE => Msg::TabClose(event.key.parse().ok()?),
            method::TAB_MENU => Msg::TabMenu(event.key.parse().ok()),
            method::TAB_OPTIONS => Msg::TabOptions(event.key.parse().ok()),
            method::NEW_TAB => Msg::NewTab(event.key.parse().ok()?, NewTab::from_key(value)?),
            method::TAB_MOVE => Msg::TabMove(event.key.parse().ok()?, Side::from_key(value)?),
            method::TAB_COLLAPSE => Msg::TabCollapse,
            method::DIVIDE => Msg::Divide(event.key.parse().ok()?, value.parse().ok()?),
            method::DIVIDED => Msg::Divided,
            method::TAB_DROP => Msg::TabDrop(event.key.parse().ok()?, event.drop()?.clone()),
            method::DOWNLOAD => Msg::Download(value.to_string(), event.key.clone()),
            method::DELETE => Msg::Delete(value.to_string()),
            method::DELETE_SNAPSHOT => Msg::DeleteSnapshot(value.to_string()),
            method::DOWNLOAD_SNAPSHOT => Msg::DownloadSnapshot(value.to_string()),
            method::PAUSE_SNAPSHOT => Msg::PauseSnapshot(value.to_string()),
            method::REVEAL => Msg::Reveal(value.to_string()),
            method::GLOBE_CLEAR => Msg::GlobeClear,
            method::OVERLAY_OPACITY => Msg::OverlayOpacity(event.key.clone(), value.parse().ok()?),
            method::OVERLAY_HIDDEN => Msg::OverlayHidden(event.key.clone(), value == "true"),
            method::OVERLAY_REMOVE => Msg::OverlayRemove(event.key.clone()),
            method::OVERLAY_SHIFT => Msg::OverlayShift(event.key.clone(), Shift::from_key(value)?),
            method::OVERLAY_HIDE_ALL => Msg::OverlayHideAll(value == "true"),
            method::OUTLINE_REMOVE => Msg::OutlineRemove(event.key.clone()),
            method::OUTLINE_FOCUS => Msg::OutlineFocus(event.key.clone()),
            // Пустой ключ — «закрыть»: имени у слоя пустым не бывает.
            method::OVERLAY_MENU => {
                Msg::OverlayMenu(Some(event.key.clone()).filter(|key| !key.is_empty()))
            }
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::ui_service::ui_event_response::Payload as Wire;

    /// Ответ ровно той формы, в какой его собирает ui-service для обработчика,
    /// чью нагрузку назвала сама разметка: имя метода, нагрузка и адресат —
    /// каждое из своего поля (см. `UiEventResponse::declared`).
    fn wire(message: &Msg) -> UiEventResponse {
        let (method, value) = message.encode();
        UiEventResponse { method, key: message.key(), payload: Some(Wire::Value(value)) }
    }

    fn id() -> ViewId {
        "7".parse().expect("ViewId из строки")
    }

    fn pane() -> PaneId {
        "3".parse().expect("PaneId из строки")
    }

    /// Каждый вариант обязан пережить полный круг: encode → шина → decode →
    /// encode без изменений. Ровно тот класс расхождений двух match'ей, ради
    /// которого заведены константы `method`.
    #[test]
    fn every_variant_survives_the_wire() {
        let mut messages = vec![
            Msg::TabSelect(id()),
            Msg::TabClose(id()),
            Msg::TabMenu(Some(pane())),
            Msg::TabMenu(None),
            Msg::TabOptions(Some(id())),
            Msg::TabOptions(None),
            Msg::TabCollapse,
            Msg::Divided,
            Msg::Download("продукт".into(), "снимок".into()),
            Msg::Delete("запись".into()),
            Msg::DeleteSnapshot("снимок".into()),
            Msg::DownloadSnapshot("снимок".into()),
            Msg::PauseSnapshot("снимок".into()),
            Msg::Reveal("запись".into()),
            Msg::GlobeClear,
            Msg::OverlayHidden("продукт".into(), true),
            Msg::OverlayHidden("продукт".into(), false),
            Msg::OverlayRemove("продукт".into()),
            Msg::OverlayShift("продукт".into(), Shift::Up),
            Msg::OverlayShift("продукт".into(), Shift::Down),
            Msg::OverlayHideAll(true),
            Msg::OverlayMenu(Some("продукт".into())),
            Msg::OverlayMenu(None),
            Msg::OutlineRemove("продукт".into()),
            Msg::OutlineFocus("продукт".into()),
            Msg::In(id(), ViewMsg::OpenMenu(None)),
            Msg::In(id(), ViewMsg::OpenMenu(Some(Menu::Filter))),
            Msg::In(id(), ViewMsg::OpenMenu(Some(Menu::Row("снимок".into())))),
            Msg::In(id(), ViewMsg::Query("s2a".into())),
            Msg::In(id(), ViewMsg::Page(3)),
            Msg::In(id(), ViewMsg::Expand("снимок".into())),
            Msg::In(id(), ViewMsg::Check("снимок".into())),
            Msg::In(id(), ViewMsg::CheckShown(true)),
            Msg::In(id(), ViewMsg::CheckShown(false)),
            Msg::In(id(), ViewMsg::CheckClear),
            Msg::In(id(), ViewMsg::InCatalog("eodata/Sentinel-2/S2B.SAFE".into())),
            Msg::In(id(), ViewMsg::SearchQuery("msil2a".into())),
            Msg::In(id(), ViewMsg::SearchFrom("2026-08-01".into())),
            Msg::In(id(), ViewMsg::SearchTo("2026-08-13".into())),
            Msg::In(id(), ViewMsg::RunSearch),
            Msg::In(id(), ViewMsg::Enter("eodata/Sentinel-2/".into())),
            Msg::In(id(), ViewMsg::Up),
            Msg::In(id(), ViewMsg::Preview("запись".into())),
            Msg::In(id(), ViewMsg::PreviewRemote("продукт".into())),
            Msg::In(id(), ViewMsg::PreviewProduct("снимок.SAFE".into())),
            Msg::In(id(), ViewMsg::PreviewFit),
            Msg::In(id(), ViewMsg::PreviewZoom(1.0)),
            Msg::In(id(), ViewMsg::GlobeShow("продукт".into())),
        ];
        messages.extend(NewTab::ALL.iter().map(|kind| Msg::NewTab(pane(), *kind)));
        messages.extend(NewTab::KINDS.iter().map(|kind| Msg::In(id(), ViewMsg::Fill(*kind))));
        messages.extend(Side::ALL.iter().map(|side| Msg::TabMove(id(), *side)));
        messages.extend(Filter::ALL.iter().map(|choice| Msg::In(id(), ViewMsg::Filter(*choice))));
        messages.extend(Grouping::ALL.iter().map(|choice| Msg::In(id(), ViewMsg::Group(*choice))));
        messages.extend(Sorting::ALL.iter().map(|choice| Msg::In(id(), ViewMsg::Sort(*choice))));
        messages.extend(
            Mission::ALL.iter().map(|choice| Msg::In(id(), ViewMsg::SearchMission(*choice))),
        );
        messages
            .extend(Period::ALL.iter().map(|choice| Msg::In(id(), ViewMsg::SearchPeriod(*choice))));
        messages
            .extend(Cloud::ALL.iter().map(|choice| Msg::In(id(), ViewMsg::SearchCloud(*choice))));

        for message in messages {
            let event = wire(&message);
            let decoded =
                Msg::decode(&event).unwrap_or_else(|| panic!("«{}» не разобрался", event.method));
            // Нагрузка и адресат сверяются порознь: они и едут порознь, и
            // сообщение, потерявшее адресата, нагрузкой этого не выдаёт.
            assert_eq!(decoded.encode(), message.encode(), "нагрузка «{}»", event.method);
            assert_eq!(decoded.key(), message.key(), "адресат «{}»", event.method);
        }
    }

    /// Нагрузку области подставляет рендерер, а адресат приезжает отдельным
    /// полем — иначе виджет, стоящий во второй половине экрана, назвать себя не
    /// может: нагрузка у него занята.
    #[test]
    fn substituted_payloads_still_name_their_tab() {
        let size = ViewportSize { width: 800, height: 600 };
        let pointer = PointerEvent::default();
        let cases: Vec<(&str, Wire)> = vec![
            (method::GLOBE_RESIZED, Wire::Size(size.clone())),
            (method::PREVIEW_RESIZED, Wire::Size(size.clone())),
            (method::GLOBE_POINTER, Wire::Pointer(pointer.clone())),
            (method::PREVIEW_POINTER, Wire::Pointer(pointer.clone())),
        ];
        for (name, payload) in cases {
            let event = UiEventResponse {
                method: name.to_string(),
                key: "7".to_string(),
                payload: Some(payload),
            };
            let Some(Msg::In(view, carried)) = Msg::decode(&event) else {
                panic!("«{}» разобралось не тем", name)
            };
            assert_eq!(view, id(), "адресат «{}»", name);
            match carried {
                ViewMsg::GlobeResized(got) | ViewMsg::PreviewResized(got) => {
                    assert_eq!(got, size, "размер «{}»", name)
                }
                ViewMsg::GlobePointer(got) | ViewMsg::PreviewPointer(got) => {
                    assert_eq!(got, pointer, "указатель «{}»", name)
                }
                _ => panic!("«{}» разобралось не тем", name),
            }
        }
    }

    /// Ползунок называет свой слой, а нагрузку отдаёт числу.
    #[test]
    fn slider_names_its_layer_apart_from_its_value() {
        let event = UiEventResponse {
            method: method::OVERLAY_OPACITY.to_string(),
            key: "eodata/Sentinel-2/S2B.SAFE".to_string(),
            payload: Some(Wire::Value("0.62".to_string())),
        };
        match Msg::decode(&event) {
            Some(Msg::OverlayOpacity(key, value)) => {
                assert_eq!(key, "eodata/Sentinel-2/S2B.SAFE");
                assert!((value - 0.62).abs() < 1e-6, "{}", value);
            }
            other => panic!("разобралось не тем: {:?}", other.map(|m| m.encode())),
        }
    }

    /// Граница называет своё деление тем же полем, что ползунок — свой слой:
    /// нагрузку у обоих занял рендерер, и сказать «чей сдвиг» больше нечем.
    #[test]
    fn divider_names_its_split_apart_from_its_delta() {
        let event = UiEventResponse {
            method: method::DIVIDE.to_string(),
            key: "4".to_string(),
            payload: Some(Wire::Value("-12.5".to_string())),
        };
        match Msg::decode(&event) {
            Some(Msg::Divide(split, delta)) => {
                assert_eq!(split.to_string(), "4");
                assert!((delta + 12.5).abs() < 1e-6, "{}", delta);
            }
            other => panic!("разобралось не тем: {:?}", other.map(|m| m.encode())),
        }
    }

    /// Адресат едет своим полем, а не внутри нагрузки: имя продукта содержит и
    /// точки, и слэши, и склеивать его с состоянием значило бы разбирать строку
    /// на обеих сторонах.
    #[test]
    fn hidden_carries_state_and_layer_apart() {
        let key = "eodata/Sentinel-1/S1A_IW_GRDH_1SDV_054821_06AC1F.SAFE";
        for hidden in [true, false] {
            let message = Msg::OverlayHidden(key.into(), hidden);
            assert_eq!(message.encode().1, hidden.to_string(), "нагрузка — только состояние");
            assert_eq!(message.key(), key, "адресат — только слой");
            match Msg::decode(&wire(&message)) {
                Some(Msg::OverlayHidden(decoded, state)) => {
                    assert_eq!((decoded.as_str(), state), (key, hidden));
                }
                other => panic!("разобралось не тем: {:?}", other.map(|m| m.encode())),
            }
        }
    }

    /// Видовое сообщение без имени вкладки — не сообщение: адресовать его
    /// некуда, и «активного вида» на этот вопрос больше нет.
    #[test]
    fn view_message_without_a_tab_is_none() {
        let event = UiEventResponse {
            method: method::RUN_SEARCH.to_string(),
            key: String::new(),
            payload: Some(Wire::Value(String::new())),
        };
        assert!(Msg::decode(&event).is_none());
    }

    /// Незнакомое имя — `None`: warn в module.rs, а не паника и не чужой смысл.
    #[test]
    fn unknown_method_is_none() {
        let event = UiEventResponse {
            method: "неведомое".to_string(),
            key: String::new(),
            payload: Some(Wire::Value(String::new())),
        };
        assert!(Msg::decode(&event).is_none());
    }
}
