pub mod types;
pub use types::*;

use veldsdk::Correlator;

pub struct State {
    /// Открытые виды в порядке вкладок. Порядок — свойство разметки, а не
    /// набора: закрытие соседа не должно менять, где стоит остальное. Половина,
    /// в которой вкладка лежит, — её собственное поле (см. `View::half`).
    views: Vec<View>,
    /// Активный вид каждой половины. `None` — в половине пусто; это законное
    /// состояние, а не ошибка: пустая половина предлагает, что в неё положить.
    active: [Option<ViewId>; 2],
    /// Половина, чья активная вкладка получает события виджетов. Событие
    /// приходит от виджета, а не от половины, и назвать себя умеют не все —
    /// поэтому у экрана есть одна половина, которая «сейчас под рукой».
    focus: Half,
    /// Разделён ли экран и где стоит вторая половина. `None` — не разделён, и
    /// вторая половина не показывается, даже если в ней что-то осталось.
    split: Option<Placement>,
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
    /// Чей «плюс» раскрыт. Не в `ListingState`: полоса вкладок принадлежит
    /// половине, а списков в ней много. Взаимоисключение со всеми прочими меню
    /// держит [`State::close_menus`] — раскрытым бывает только одно.
    pub tab_menu: Option<Half>,
    /// У какой вкладки раскрыто её собственное меню: разделить, перенести,
    /// закрыть.
    pub tab_options: Option<ViewId>,
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
    /// Наложения снимков на шар — то, что собирается или уже показано
    /// (см. handlers::overlay). Порядок — порядок слоёв снизу вверх, тот же,
    /// которым его понимает глобус; список «На просмотре» переворачивает его
    /// сам, потому что «сверху новые» — свойство экрана, а не набора.
    pub overlays: Vec<overlay::OverlayState>,

    // -- Маршруты ответов --
    //
    // Таблица заводится на топик ответа, а не на вид (см. veldsdk::Correlator):
    // «чей это id» должно иметь один ответ. Перебор по видам на этот вопрос не
    // отвечает — вид могли закрыть, пока ответ шёл, а приехавший в нём ресурс
    // всё равно наш, и опознать его можно только здесь.
    /// Превью: открытие ресурса (data-library или data-provider). Дальше
    /// показ ведёт канва, и правда о нём приходит рассылкой on_view_state,
    /// адресованной именем вида, — корреляций там нет.
    pub previews: Correlator<ViewId>,
    /// Растры наложений: открытие ресурса у провайдера. Контекст — ключ
    /// наложения и роль растра; сборка помнит свои корреляции, чтобы снять их
    /// отсюда, когда наложение убирают (см. overlay::Assembly).
    pub opens: Correlator<(String, crate::proto::globe::OverlayRole)>,
    /// data-provider/on_imagery_result — какие растры у продукта. Контекст —
    /// ключ наложения, которое их ждёт.
    pub imageries: Correlator<String>,
    /// Тот же топик, но для превью: снимок, лежащий папкой, открыть напрямую
    /// нельзя (GET по пути каталога отвечает 404), и растр внутри него
    /// выбирает провайдер. Контекст — вкладка превью, которая его ждёт.
    ///
    /// Отдельной таблицей, а не общей с наложениями: ответ один и тот же, а
    /// ждущих двое, и «чей это id» должно иметь ровно один ответ.
    pub preview_imagery: Correlator<ViewId>,
    /// data-provider/on_list_path_result.
    pub listings: Correlator<ViewId>,
    /// data-provider/on_search_result.
    pub searches: Correlator<ViewId>,
    /// globe/on_probe — «что под указателем». Не таблица, а последний вопрос:
    /// ответ на предыдущий уже не нужен, указатель с тех пор уехал.
    pub probe: veldsdk::Latest,
    /// data-provider/on_locate_result — продукт по ключу для «показать на
    /// шаре» из каталога и загрузок. Последний вопрос, как probe: показывают
    /// один продукт, и второй щелчок отменяет первый.
    pub locates: veldsdk::Latest,
}

impl State {
    pub fn new(config: crate::module::handlers::Config) -> anyhow::Result<Self> {
        let mut state = Self {
            views: Vec::new(),
            active: [None, None],
            focus: Half::First,
            split: None,
            next_id: 1,
            library: library::LibraryState::default(),
            error: None,
            window_surface: None,
            window: (0, 0),
            scale: 1.0,
            tab_menu: None,
            tab_options: None,
            speed: 0.0,
            measured: None,
            shown: None,
            overlays: Vec::new(),
            previews: Correlator::new(),
            opens: Correlator::new(),
            imageries: Correlator::new(),
            preview_imagery: Correlator::new(),
            listings: Correlator::new(),
            searches: Correlator::new(),
            probe: veldsdk::Latest::default(),
            locates: veldsdk::Latest::default(),
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

    /// Открывает вид в названной половине и делает его в ней активным, а саму
    /// половину — той, что под рукой. Открытие — это всегда новая вкладка:
    /// «переиспользовать похожую» решает вызывающий, у него для этого есть
    /// [`Self::find`].
    pub fn open_in(&mut self, half: Half, kind: ViewKind) -> ViewId {
        let id = ViewId(self.next_id);
        self.next_id += 1;
        self.views.push(View { id, kind, half });
        self.active[half.index()] = Some(id);
        self.focus = half;
        id
    }

    /// То же в половине, которая сейчас под рукой, — так открывается всё, что
    /// заводят не глядя на разделение экрана: превью из строки, глобус по
    /// показу снимка.
    pub fn open(&mut self, kind: ViewKind) -> ViewId {
        self.open_in(self.focus, kind)
    }

    /// Убирает вид из набора и отдаёт его вызывающему: закрытие превью гасит
    /// показ у канвы, а публиковать состояние не вправе — исходящая связь
    /// модуля должна быть видна в схеме (см. handlers::nav).
    pub fn close(&mut self, id: ViewId) -> Option<View> {
        let at = self.views.iter().position(|view| view.id == id)?;
        let view = self.views.remove(at);
        if self.active[view.half.index()] == Some(id) {
            self.active[view.half.index()] = self.neighbour(view.half, at);
        }
        Some(view)
    }

    /// Кто занимает место закрытой вкладки: следующая в той же половине, иначе
    /// предыдущая. Пустая половина оставляет `None` — и предлагает, что в неё
    /// положить.
    ///
    /// Считается по общему списку, а не по отфильтрованному: индексы в нём и
    /// есть порядок вкладок, а соседство — это соседство внутри своей половины.
    fn neighbour(&self, half: Half, at: usize) -> Option<ViewId> {
        let right = self.views.iter().skip(at).find(|view| view.half == half);
        let left = self.views.iter().take(at).rev().find(|view| view.half == half);
        right.or(left).map(|view| view.id)
    }

    /// Делает вид активным в его половине, а её — той, что под рукой.
    pub fn focus(&mut self, id: ViewId) {
        let Some(view) = self.views.iter().find(|view| view.id == id) else { return };
        let half = view.half;
        self.active[half.index()] = Some(id);
        self.focus = half;
    }

    /// Половина, чьи виджеты сейчас отвечают за события без адресата.
    pub fn focused(&self) -> Half {
        self.focus
    }

    /// Разделён ли экран и где стоит вторая половина.
    pub fn split(&self) -> Option<Placement> {
        self.split
    }

    /// Разделить экран: с названной стороны открывается пустая половина, а
    /// вкладка, из меню которой позвали, остаётся там, где стояла. Уезжала бы
    /// она — с экрана уходило бы то, на что смотрели, а делят его ровно затем,
    /// чтобы это не уходило; заодно переезд стоил бы ей прокрутки и каретки:
    /// состояние виджетов рендерер держит по месту в дереве разметки.
    pub fn split_off(&mut self, id: ViewId, placement: Placement) {
        self.split = Some(placement);
        self.focus(id);
    }

    /// Свести половины обратно в одну: всё уезжает в первую, порядок вкладок
    /// сохраняется — он свойство списка, а не половины.
    pub fn unsplit(&mut self) {
        self.split = None;
        for view in &mut self.views {
            view.half = Half::First;
        }
        // Активной остаётся та, что была под рукой: на неё и смотрели. Если под
        // рукой была пустая половина — первая же из оставшихся: вкладки есть, и
        // «все вкладки закрыты» вместо них было бы неправдой.
        self.active[0] = self.active[self.focus.index()]
            .or(self.active[0])
            .or(self.active[1])
            .or_else(|| self.views.first().map(|view| view.id));
        self.active[1] = None;
        self.focus = Half::First;
    }

    /// Перенести вкладку в другую половину — и туда же перевести взгляд.
    pub fn move_to(&mut self, id: ViewId, half: Half) {
        let Some(at) = self.views.iter().position(|view| view.id == id) else { return };
        let from = self.views[at].half;
        if from == half {
            return;
        }
        self.views[at].half = half;
        if self.active[from.index()] == Some(id) {
            self.active[from.index()] = self.neighbour(from, at);
        }
        self.active[half.index()] = Some(id);
        self.focus = half;
    }

    pub fn views(&self) -> &[View] {
        &self.views
    }

    /// Вкладки одной половины, в порядке их списка.
    pub fn views_in(&self, half: Half) -> impl Iterator<Item = &View> {
        self.views.iter().filter(move |view| view.half == half)
    }

    /// Активная вкладка названной половины — то, что в ней показано.
    pub fn active_in(&self, half: Half) -> Option<ViewId> {
        self.active[half.index()]
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

    // -- Доступ к состоянию названного вида --
    //
    // Именно названного, а не активного: половин на экране две, и «активный
    // вид» на вопрос «чей это щелчок» больше не отвечает. Кто именно — говорит
    // само сообщение (см. `Msg::In`).

    /// Ширина окна в точках разметки.
    pub fn logical_width(&self) -> f32 {
        self.window.0 as f32 / self.scale.max(1.0)
    }

    /// Ширина половины — то, чем меряется место под колонки списка. Считать
    /// его по окну нельзя: половина вдвое уже, а колонки от этого не худеют, и
    /// тянущееся имя схлопнулось бы в ноль (см. `table::fit`).
    ///
    /// Делит только разделение вбок: снизу половина остаётся во всю ширину.
    pub fn pane_width(&self) -> f32 {
        match self.split {
            Some(Placement::Right | Placement::Left) => self.logical_width() / 2.0,
            Some(Placement::Below) | None => self.logical_width(),
        }
    }

    /// Настройки показа списка: их правят сообщения, одинаковые для всех
    /// списочных видов.
    pub fn listing_mut(&mut self, id: ViewId) -> Option<&mut listing::ListingState> {
        self.get_mut(id)?.listing_mut()
    }

    /// Закрывает всё раскрытое: меню половин, меню вкладки и меню каждого
    /// списка. Одним движением, потому что раскрытым бывает только одно, — а
    /// держать это правило присвоениями по обработчикам значит однажды забыть
    /// одно. По всем спискам, а не по активному: раскрытое в другой половине
    /// осталось бы висеть.
    pub fn close_menus(&mut self) {
        self.tab_menu = None;
        self.tab_options = None;
        for view in &mut self.views {
            if let Some(listing) = view.kind.listing_mut() {
                listing.menu = listing::Menu::Closed;
            }
        }
    }

    pub fn browse_mut(&mut self, id: ViewId) -> Option<&mut browse::BrowseState> {
        match self.get_mut(id)? {
            ViewKind::Browse(browse) => Some(browse),
            _ => None,
        }
    }

    pub fn search_mut(&mut self, id: ViewId) -> Option<&mut search::SearchState> {
        match self.get_mut(id)? {
            ViewKind::Search(search) => Some(search),
            _ => None,
        }
    }

    /// Глобус названного вида. `None` — вкладку закрыли или подменили, пока
    /// событие шло.
    pub fn globe_mut(&mut self, id: ViewId) -> Option<&mut globe::GlobeState> {
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

    /// Он же одним ключом — то, чем его узнаю́т списки: строку с этим ключом
    /// они отмечают у себя. Пусто — не выбрано ничего.
    pub fn picked_key(&self) -> &str {
        self.picked().map(|product| product.identifier.as_str()).unwrap_or_default()
    }
}

pub mod search;
pub mod browse;
pub mod globe;
pub mod library;
pub mod listing;
pub mod overlay;
pub mod preview;

pub use browse::BrowseState;
pub use globe::GlobeState;
pub use library::LibraryState;
pub use preview::PreviewState;
pub use search::SearchState;
