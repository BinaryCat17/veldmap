pub mod types;
pub use types::*;

use veldsdk::Correlator;

pub struct State {
    /// Открытые виды в порядке вкладок. Порядок — свойство разметки, а не
    /// набора: закрытие соседа не должно менять, где стоит остальное.
    views: Vec<View>,
    /// Активный вид. `None` — все вкладки закрыты; это законное состояние,
    /// а не ошибка, поэтому пустой экран рисуется явно.
    active: Option<ViewId>,
    next_id: u64,
    /// Кэш состояния библиотеки — один на модуль: он приходит рассылкой и
    /// нужен всем видам сразу (см. state::library).
    pub library: library::LibraryState,
    /// Отказ библиотеки — единственное, что относится к модулю целиком, а не
    /// к какому-то одному виду. Ход своей работы каждый вид показывает у себя:
    /// строка состояния одна, а видов много, и «12 items» из фоновой вкладки
    /// под заголовком активной — неправда.
    pub error: Option<String>,
    /// Render-таргет нашего окна: аллоцируется в ответ на app/window_resized
    /// и делегируется рендереру (см. handlers::window).
    pub window_surface: Option<veldsdk::surface::Delegated>,
    /// Размер окна в физических пикселях (app/window_resized). Нужен как
    /// потолок для превью: рисовать картинку крупнее окна незачем.
    pub window: (u32, u32),
    /// Масштаб интерфейса оттуда же. Вместе с размером даёт ширину окна в
    /// точках разметки — по ней считается, сколько знаков имени влезает в свою
    /// колонку (см. components::table).
    pub scale: f32,
    /// Раскрыто ли меню «плюса» в полосе вкладок. Не в `ListingState`: полоса
    /// вкладок общая, а списков много.
    pub tab_menu: bool,
    /// Скорость закачки, байт в секунду: выводится из двух соседних состояний
    /// библиотеки (см. handlers::library). Своего поля у библиотеки под это
    /// нет — она рассылает то, что есть сейчас, а не то, как быстро оно росло.
    pub speed: f32,
    /// Прошлый замер: когда и сколько было скачано.
    pub measured: Option<(i64, u64)>,
    /// Что сейчас очерчено на шаре. Не в состоянии вкладки глобуса: контуры —
    /// свойство найденного, а не экрана, и уезжают они к рисующему независимо
    /// от того, открыта ли вкладка (см. handlers::search::show_on_globe).
    pub shown: Option<globe::Shown>,

    // -- Маршруты ответов --
    //
    // Таблица заводится на топик ответа, а не на вид (см. veldsdk::Correlator):
    // «чей это id» должно иметь один ответ. Перебор по видам на этот вопрос не
    // отвечает — вид могли закрыть, пока ответ шёл, а приехавший в нём ресурс
    // всё равно наш, и опознать его можно только здесь.
    /// Превью: одна корреляция на оба шага цепочки — открытие ресурса
    /// (data-library или data-provider) и декодирование (image-loader).
    pub previews: Correlator<ViewId>,
    /// data-provider/on_list_path_result.
    pub listings: Correlator<ViewId>,
    /// data-provider/on_search_result.
    pub searches: Correlator<ViewId>,
    /// globe/on_probe — «что под указателем». Не таблица, а последний вопрос:
    /// ответ на предыдущий уже не нужен, указатель с тех пор уехал.
    pub probe: veldsdk::Latest,
}

impl State {
    pub fn new(config: crate::module::handlers::Config) -> anyhow::Result<Self> {
        let mut state = Self {
            views: Vec::new(),
            active: None,
            next_id: 1,
            library: library::LibraryState::default(),
            error: None,
            window_surface: None,
            window: (0, 0),
            scale: 1.0,
            tab_menu: false,
            speed: 0.0,
            measured: None,
            shown: None,
            previews: Correlator::new(),
            listings: Correlator::new(),
            searches: Correlator::new(),
            probe: veldsdk::Latest::default(),
        };

        // Стартовая вкладка — из конфига; умолчание и поведение при неизвестном
        // значении — Search (этот вид существует всегда).
        let kind = match config.initial_view.as_deref() {
            None | Some("search") => ViewKind::Search(search::SearchState::default()),
            Some("browse") => ViewKind::Browse(browse::BrowseState::default()),
            Some("downloaded") => ViewKind::Downloaded(listing::ListingState::default()),
            Some(other) => {
                veldsdk::log::warn!(target: "system", "unknown initial_view '{}', falling back to Search", other);
                ViewKind::Search(search::SearchState::default())
            }
        };
        state.open(kind);
        Ok(state)
    }

    // -- Вкладки --

    /// Открывает вид и делает его активным. Открытие — это всегда новая
    /// вкладка: «переиспользовать похожую» решает вызывающий, у него для
    /// этого есть [`Self::find`].
    pub fn open(&mut self, kind: ViewKind) -> ViewId {
        let id = ViewId(self.next_id);
        self.next_id += 1;
        self.views.push(View { id, kind });
        self.active = Some(id);
        id
    }

    /// Убирает вид из набора и отдаёт его вызывающему: закрытие превью гасит
    /// декодирование, а публиковать состояние не вправе — исходящая связь
    /// модуля должна быть видна в схеме (см. handlers::nav).
    pub fn close(&mut self, id: ViewId) -> Option<View> {
        let at = self.views.iter().position(|view| view.id == id)?;
        let view = self.views.remove(at);
        if self.active == Some(id) {
            // Соседняя вкладка: та, что заняла освободившийся индекс, иначе
            // левая. Пустой набор оставляет активной None.
            self.active = self
                .views
                .get(at)
                .or_else(|| at.checked_sub(1).and_then(|left| self.views.get(left)))
                .map(|view| view.id);
        }
        Some(view)
    }

    pub fn focus(&mut self, id: ViewId) {
        if self.views.iter().any(|view| view.id == id) {
            self.active = Some(id);
        }
    }

    pub fn views(&self) -> &[View] {
        &self.views
    }

    pub fn active_id(&self) -> Option<ViewId> {
        self.active
    }

    pub fn active(&self) -> Option<&ViewKind> {
        let id = self.active?;
        self.views.iter().find(|view| view.id == id).map(|view| &view.kind)
    }

    pub fn get(&self, id: ViewId) -> Option<&ViewKind> {
        self.views.iter().find(|view| view.id == id).map(|view| &view.kind)
    }

    pub fn get_mut(&mut self, id: ViewId) -> Option<&mut ViewKind> {
        self.views.iter_mut().find(|view| view.id == id).map(|view| &mut view.kind)
    }

    /// Первый вид, удовлетворяющий условию, — для «открыть или показать уже
    /// открытое».
    pub fn find(&self, matches: impl Fn(&ViewKind) -> bool) -> Option<ViewId> {
        self.views.iter().find(|view| matches(&view.kind)).map(|view| view.id)
    }

    // -- Доступ к состоянию активного вида --
    //
    // Источник события виджета — активный вид: виден ровно он.

    /// Ширина окна в точках разметки — то, чем меряется место под колонки.
    pub fn logical_width(&self) -> f32 {
        self.window.0 as f32 / self.scale.max(1.0)
    }

    /// Настройки показа активного списка: их правят сообщения, одинаковые для
    /// всех трёх видов.
    pub fn active_listing_mut(&mut self) -> Option<&mut listing::ListingState> {
        let id = self.active?;
        self.get_mut(id)?.listing_mut()
    }

    pub fn active_browse_mut(&mut self) -> Option<(ViewId, &mut browse::BrowseState)> {
        let id = self.active?;
        match self.get_mut(id)? {
            ViewKind::Browse(browse) => Some((id, browse)),
            _ => None,
        }
    }

    pub fn active_search_mut(&mut self) -> Option<(ViewId, &mut search::SearchState)> {
        let id = self.active?;
        match self.get_mut(id)? {
            ViewKind::Search(search) => Some((id, search)),
            _ => None,
        }
    }

    /// Глобус активного вида. `None` — сверху не он: события области приходят
    /// только от видимой вкладки, но между отправкой и приходом её могли
    /// сменить.
    pub fn active_globe_mut(&mut self) -> Option<&mut globe::GlobeState> {
        let id = self.active?;
        match self.get_mut(id)? {
            ViewKind::Globe(globe) => Some(globe),
            _ => None,
        }
    }

    /// Превью названного вида. `None` — вкладку закрыли, пока ответ шёл;
    /// приехавший ресурс всё равно наш, освобождает его вызывающий.
    pub fn preview_mut(&mut self, id: ViewId) -> Option<&mut preview::PreviewState> {
        match self.get_mut(id)? {
            ViewKind::Preview(preview) => Some(preview),
            _ => None,
        }
    }

    /// Выбранный на шаре снимок. `None` — не выбран ни один или закрыли вид, из
    /// которого он взялся: контуры на шаре его переживают, а сам он — нет.
    pub fn picked(&self) -> Option<&crate::proto::data_provider::DataProduct> {
        let shown = self.shown.as_ref()?;
        let selected = shown.selected.as_ref()?;
        let ViewKind::Search(search) = self.get(shown.view)? else { return None };
        search.results.iter().find(|product| &product.identifier == selected)
    }
}

pub mod search;
pub mod browse;
pub mod globe;
pub mod library;
pub mod listing;
pub mod preview;

pub use browse::BrowseState;
pub use globe::GlobeState;
pub use library::LibraryState;
pub use preview::PreviewState;
pub use search::SearchState;
