pub mod types;
pub use types::*;

use veldsdk::Correlator;

/// Кому едет ответ на data-provider/on_list_path. Топик один, а спрашивают его
/// трое, и «чей это id» должно иметь ровно один ответ — перебором по видам на
/// него не ответить.
pub enum Listing {
    /// Содержимое папки, которую показывает вкладка.
    Path(ViewId),
    /// Содержимое раскрытой строки: вкладка и ключ её папки.
    Children(ViewId, String),
    /// Все файлы снимка — под закачку целиком. Вкладки здесь нет: качают в
    /// библиотеку, а она одна на окно. Счётчики едут с запросом, а не лежат в
    /// состоянии: снимков могут качать несколько сразу, и общий счётчик
    /// оборвал бы второй на середине первого.
    ///
    /// `files` — сколько файлов снимка прошло мимо нас, `queued` — сколько из
    /// них поставлено в закачку. Первое считается ради библиотеки: обойти
    /// снимок в хранилище может только этот обход, и его итог — единственный
    /// ответ на «сколько файлов должно быть».
    Snapshot { product: String, files: u32, queued: usize },
}

/// Кому едет ответ на data-provider/on_locate_result. Ждут его двое, и по
/// содержимому их не различить: продукт в обоих случаях один и тот же.
pub enum Locate {
    /// Положить снимок на шар — нажали значок глобуса в строке. Тем же ходом
    /// наполняется и кэш геометрии: контур этого снимка спросил бы у каталога
    /// то же самое (см. `handlers::overlay::on_toggle_pressed`).
    Overlay(String),
    /// Геометрия отмеченного снимка: очертить его. Ключ — тот, которым он
    /// отмечен в списке; он же ключ кэша (см. `State::located`).
    Outline(String),
}

/// Что известно про продукт по ключу строки.
///
/// Одной таблицей на все три состояния, а не «кэш ответов» плюс «спрошенное»:
/// это одно знание в трёх положениях, и два множества под него держались бы
/// договором «ключ не бывает в обоих» — договором, который однажды нарушат.
pub enum Located {
    /// Спрошено, ответа ещё нет. Второй раз спрашивать незачем: сборка контуров
    /// идёт на каждую отметку, и без этого один ключ уехал бы в каталог десяток
    /// раз.
    Asking,
    /// Каталог его не знает. Тоже ответ, и запомнить его надо: иначе
    /// ненайденное спрашивалось бы вечно.
    Missing,
    /// Спросить не вышло — сеть или отказ службы. Про сам снимок это не
    /// говорит ничего, поэтому метка временная: её снимает следующая отметка
    /// той же строки, и «щёлкни ещё раз» и есть повтор. Само по себе не
    /// повторяется — иначе ответ на сорвавшийся запрос звал бы следующий, и
    /// так до конца запуска.
    Failed,
    Found(crate::proto::data_provider::DataProduct),
}

/// Что сейчас раскрыто на экране.
///
/// Одним полем на весь экран, а не полем у каждого носителя: раскрытым бывает
/// только одно, и правило это про экран, а не про панель, вкладку или слой.
/// Разложенное по носителям, оно держалось бы присвоениями в каждом
/// обработчике — а забыть одно из них значит оставить два раскрытых меню, и
/// заметить это можно только глазами.
#[derive(Default, PartialEq, Eq)]
pub enum Open {
    #[default]
    Nothing,
    /// «Плюс» полосы вкладок названной панели.
    TabMenu(PaneId),
    /// Меню самой вкладки: куда её деть.
    TabOptions(ViewId),
    /// Меню слоя «На просмотре». Слой назван ключом снимка — им же он
    /// адресуется везде ещё.
    Layer(String),
    /// Меню внутри списка названной вкладки: чип полосы или строка
    /// (см. [`listing::Menu`]).
    Listing(ViewId, listing::Menu),
    /// Список величин файла под кадром превью названной вкладки.
    Variables(ViewId),
    /// Список величин лежащего подробным файла в строке слоя «На просмотре»,
    /// названного ключом снимка.
    LayerVariables(String),
}

/// Что на экране главное сейчас — один снимок на весь экран.
///
/// Ставят подсветку двое: переход к строке и щелчок по контуру на шаре. Вопрос
/// у них один, «что здесь главное сейчас», и двух ответов у него не бывает —
/// поэтому и поле одно. Разложенная по носителям, подсветка держалась бы
/// гашением из каждого обработчика, и подсвеченными оставались бы обе строки:
/// та, к которой привели полчаса назад, и та, которую выбрали сейчас.
pub struct Highlight {
    /// Ключ снимка. По нему подсвечена его строка в каждом списке, где она
    /// есть: отмечают в одном, а видно должно быть везде.
    pub key: String,
    /// Вкладка, чей переход к этой строке привёл: в ней она ещё и выведена на
    /// свою страницу и подкручена под глаз. `None` — подсветку поставил шар, и
    /// вкладки у неё нет.
    pub view: Option<ViewId>,
    /// Обведён ли снимок на шаре лентой. Ставит её щелчок по контуру; переход
    /// к строке ленту не зажигает — на шар в этот момент не смотрят.
    pub on_globe: bool,
}

/// Насколько узкой человек вправе сделать панель, в точках разметки. Уже этого
/// в ней не видно ни полосы вкладок, ни того, что в них.
pub const MIN_PANE: f32 = 180.0;

pub struct State {
    /// Открытые виды в порядке вкладок. Порядок — свойство разметки, а не
    /// набора: закрытие соседа не должно менять, где стоит остальное. Панель,
    /// в которой вкладка лежит, — её собственное поле (см. `View::pane`), а
    /// порядок внутри панели — порядок этого же списка.
    views: Vec<View>,
    /// Как сложен экран: дерево панелей с их долями и активными вкладками
    /// (см. state::layout).
    layout: Layout,
    /// Панель, чья активная вкладка получает события виджетов. Событие приходит
    /// от виджета, а не от панели, и назвать себя умеют не все — поэтому у
    /// экрана есть одна панель, которая «сейчас под рукой».
    focus: PaneId,
    next_id: u64,
    /// С чего начинать, когда вспоминать нечего, — из конфига. Сохранённая
    /// раскладка старше: конфиг говорит, с чего начать первый запуск, а не что
    /// показывать вместо того, на чём человек остановился.
    pub start: crate::module::NewTab,
    /// Отпечаток последней записанной раскладки. По нему видно, изменилось ли
    /// что-то с тех пор: раскладку пишут на каждое действие с вкладками, а
    /// большинство действий её не трогает (см. handlers::persist).
    pub last_saved: u64,
    /// Кэш состояния библиотеки — один на модуль: он приходит рассылкой и
    /// нужен всем видам сразу (см. state::library).
    pub library: library::LibraryState,
    /// Отказ библиотеки. Ставит и гасит его она сама: она рассылает состояние
    /// на каждое изменение, и «нет ответа» в этой рассылке значит «жалобы
    /// больше нет». Ход своей работы каждый вид показывает у себя: строка
    /// состояния одна, а видов много, и «12 items» из фоновой вкладки под
    /// заголовком активной — неправда.
    pub error: Option<String>,
    /// Что не вышло по нажатию: снимок без растров, снимок, обрезанный
    /// потолком, продукт, не найденный в каталоге.
    ///
    /// Отдельно от отказа библиотеки, а не в том же поле: библиотека рассылает
    /// состояние на каждый поставленный в очередь файл и гасит им своё
    /// сообщение — а поставила эти файлы как раз та работа, о которой
    /// извещение. В общем поле оно стиралось бы раньше, чем его успевали
    /// прочесть, причём не случайно, а всегда.
    ///
    /// Живёт до следующего действия: его и гасит (см. `module::on_ui_event`).
    /// Своего «закрыть» у него нет — извещение о том, что только что не
    /// вышло, переживать следующее нажатие не должно.
    pub notice: Option<String>,
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
    /// Что раскрыто на экране — одно на всех (см. [`Open`]).
    pub open: Open,
    /// Скорость закачки, байт в секунду: выводится из двух соседних состояний
    /// библиотеки (см. handlers::library). Своего поля у библиотеки под это
    /// нет — она рассылает то, что есть сейчас, а не то, как быстро оно росло.
    pub speed: f32,
    /// Прошлый замер: когда и сколько было скачано.
    pub measured: Option<(i64, u64)>,
    /// Что сейчас очерчено на шаре. Не в состоянии вкладки глобуса: контуры —
    /// свойство отмеченного в списках, а не экрана, и уезжают они к рисующему
    /// независимо от того, открыта ли вкладка (см. handlers::outline).
    ///
    /// Сводится сюда из пакетного выделения всех списков сразу — потому и не в
    /// состоянии вида: шар один, а списков, отмечающих на нём снимки, сколько
    /// угодно.
    pub outlined: Vec<globe::Outlined>,
    /// Что подсвечено на экране (см. [`Highlight`]).
    pub highlight: Option<Highlight>,
    /// Продукт каталога по ключу строки: что спрошено и что отвечено
    /// (см. [`Located`]). У строк поиска продукт под рукой, и сюда они не
    /// попадают.
    pub located: std::collections::HashMap<String, Located>,
    /// Наложения снимков на шар — то, что собирается или уже показано
    /// (см. handlers::overlay). Порядок — порядок слоёв снизу вверх, тот же,
    /// которым его понимает глобус; список «На просмотре» переворачивает его
    /// сам, потому что «сверху новые» — свойство экрана, а не набора.
    pub overlays: Vec<overlay::OverlayState>,
    /// Снимки, которые попросили очертить на шаре.
    ///
    /// Своё множество, а не производное от выбора в списках: контур, показ
    /// растром и выбор строки — три независимых вещи, и сведённые в одну они
    /// отвечали бы друг за друга. Одно на приложение, а не на вкладку: шар
    /// один, и «контуры этой вкладки» на вопрос «что на шаре» не отвечает.
    ///
    /// Это просьба, а не нарисованное: геометрию знает каталог, и до его
    /// ответа ключ здесь уже есть, а в [`State::outlined`] его ещё нет.
    pub outlines: std::collections::HashSet<String>,
    /// Снимки, для которых показ ждёт ответа каталога.
    ///
    /// Показ — не выбор: он кладёт слой во «На просмотре» и коробочку в списке
    /// не трогает (см. handlers::overlay). Поэтому и «а не расхотели ли» по
    /// выбору не спросить: ход к каталогу живёт секунды, убить его нечем, а
    /// «Снять с шара» за это время нажать успевают.
    pub showing: std::collections::HashSet<String>,
    /// Снимки, которые показать не вышло, и почему — по ключу снимка.
    ///
    /// Помнится затем, что отказ этот про сам снимок, а не про попытку:
    /// «изображения в продукте нет вовсе» и «файл читается только целиком»
    /// вторым нажатием не лечатся, а ждать их приходится десятки секунд.
    /// Сказанное строкой состояния живёт до следующего слова и пропадает
    /// вместе с ним; вернувшись в список через минуту, человек видит тот же
    /// зовущий значок и жмёт его снова.
    ///
    /// Не отменяет действие, а объясняет его: значок остаётся нажимаемым —
    /// сорваться попытка могла и по сети, — но горит предупреждением и носит
    /// причину подсказкой. Снимается причина новой попыткой: держать её после
    /// удавшегося показа значило бы врать.
    pub unshowable: std::collections::HashMap<String, String>,

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
    pub opens: Correlator<(String, crate::proto::globe::OverlayRole, u32, overlay::Part)>,
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
    /// data-provider/on_list_path_result. Контекст — кому этот листинг
    /// (см. [`Listing`]).
    pub listings: Correlator<Listing>,
    /// data-provider/on_search_result.
    pub searches: Correlator<ViewId>,
    /// globe/on_probe — «что под указателем». Не таблица, а последний вопрос:
    /// ответ на предыдущий уже не нужен, указатель с тех пор уехал.
    pub probe: veldsdk::Latest,
    /// data-provider/on_locate_result — продукт по ключу хранилища. Контекст —
    /// зачем его спросили (см. [`Locate`]): положить на шар или очертить.
    pub locates: Correlator<Locate>,
    /// fs/on_write раскладки: корреляция → регион, отданный службе. Освобождает
    /// его ответ — до него регион наш (см. handlers::persist).
    pub layout_writes: Correlator<u64>,
    /// Чтение сохранённой раскладки. Не таблица, а последний вопрос: спрашивают
    /// её один раз, на старте, и ответ на позапрошлый вопрос значил бы, что
    /// экран собирают дважды.
    pub layout_read: veldsdk::Latest,
}

impl State {
    pub fn new(config: crate::module::handlers::Config) -> anyhow::Result<Self> {
        // Стартовая вкладка — из конфига; умолчание и поведение при неизвестном
        // значении — Search (этот вид существует всегда).
        let start = match config.initial_view.as_deref() {
            None | Some("search") => crate::module::NewTab::Search,
            Some("browse") => crate::module::NewTab::Browse,
            Some("downloaded") => crate::module::NewTab::Downloaded,
            Some(other) => {
                veldsdk::log::warn!(target: "system", "unknown initial_view '{}', falling back to Search", other);
                crate::module::NewTab::Search
            }
        };

        let layout = Layout::new();
        Ok(Self {
            views: Vec::new(),
            focus: layout.first(),
            layout,
            next_id: 1,
            start,
            last_saved: 0,
            library: library::LibraryState::default(),
            error: None,
            notice: None,
            unshowable: std::collections::HashMap::new(),
            window_surface: None,
            window: (0, 0),
            scale: 1.0,
            open: Open::Nothing,
            speed: 0.0,
            measured: None,
            outlined: Vec::new(),
            showing: std::collections::HashSet::new(),
            outlines: std::collections::HashSet::new(),
            highlight: None,
            located: Default::default(),
            overlays: Vec::new(),
            previews: Correlator::new(),
            opens: Correlator::new(),
            imageries: Correlator::new(),
            preview_imagery: Correlator::new(),
            listings: Correlator::new(),
            searches: Correlator::new(),
            probe: veldsdk::Latest::default(),
            locates: Correlator::new(),
            layout_writes: Correlator::new(),
            layout_read: veldsdk::Latest::default(),
        })
    }

    /// Заводит панель с названной стороны и сразу задаёт ей долю — так экран
    /// собирают из сохранённого (см. handlers::persist). Живой путь к делению
    /// экрана другой: делит его перенос вкладки (см. [`Self::move_aside`]).
    pub fn split_pane(&mut self, pane: PaneId, side: Side, fraction: f32) -> Option<PaneId> {
        let fresh = self.layout.split(pane, side)?;
        if let Some(split) = self.layout.split_of(fresh) {
            self.layout.set_fraction(split, fraction);
            // Тот же предел, что у живой границы, и тем же способом: доля
            // приезжает из файла, а он бывает и правленым, и писанным другой
            // раскладкой окна. Открывшаяся в ноль панель не показывает ни
            // вкладок, ни границы, за которую ей вернуть место.
            self.divide(split, 0.0);
        }
        Some(fresh)
    }

    // -- Вкладки --

    /// Открывает вид в названной панели и делает его в ней активным, а саму
    /// панель — той, что под рукой. Открытие — это всегда новая вкладка:
    /// «переиспользовать похожую» решает вызывающий, у него для этого есть
    /// [`Self::find`].
    pub fn open_in(&mut self, pane: PaneId, kind: ViewKind) -> ViewId {
        let id = ViewId(self.next_id);
        self.next_id += 1;
        let at = self.after_last(pane);
        self.views.insert(at, View { id, kind, pane });
        self.layout.set_active(pane, Some(id));
        self.focus = pane;
        id
    }

    /// Место в списке сразу за последней вкладкой этой панели: туда встаёт и
    /// открытое, и перенесённое — конец полосы и есть то место, куда человек
    /// смотрит после такого действия. У панели без вкладок это конец списка.
    fn after_last(&self, pane: PaneId) -> usize {
        self.views
            .iter()
            .rposition(|view| view.pane == pane)
            .map_or(self.views.len(), |at| at + 1)
    }

    /// Убирает вид из набора и отдаёт его вызывающему: закрытие превью гасит
    /// показ у канвы, а публиковать состояние не вправе — исходящая связь
    /// модуля должна быть видна в схеме (см. handlers::nav).
    pub fn close(&mut self, id: ViewId) -> Option<View> {
        let at = self.views.iter().position(|view| view.id == id)?;
        let view = self.views.remove(at);
        if self.layout.active(view.pane) == Some(id) {
            self.layout.set_active(view.pane, self.neighbour(view.pane, at));
        }
        self.forget_if_empty(view.pane);
        Some(view)
    }

    /// Убирает опустевшую панель: место отдаётся соседней, к ней же переходит
    /// взгляд. Последняя панель остаётся пустой — отдавать её место некому, а
    /// «все вкладки закрыты» надо где-то показать.
    ///
    /// Публично не только ради закрытия вкладки: пустой панель выходит и из
    /// сохранённой раскладки, где синглтон назван дважды (см.
    /// `handlers::persist::restore`), а правило «пустых панелей не бывает» на
    /// то и правило, чтобы быть одним.
    pub fn forget_if_empty(&mut self, pane: PaneId) {
        if self.views_in(pane).next().is_some() {
            return;
        }
        if let Some(took) = self.layout.remove(pane)
            && self.focus == pane
        {
            self.focus = took;
        }
    }

    /// Кто занимает место закрытой вкладки: следующая в той же панели, иначе
    /// предыдущая.
    ///
    /// Считается по общему списку, а не по отфильтрованному: индексы в нём и
    /// есть порядок вкладок, а соседство — это соседство внутри своей панели.
    fn neighbour(&self, pane: PaneId, at: usize) -> Option<ViewId> {
        let right = self.views.iter().skip(at).find(|view| view.pane == pane);
        let left = self.views.iter().take(at).rev().find(|view| view.pane == pane);
        right.or(left).map(|view| view.id)
    }

    /// Делает вид активным в его панели, а её — той, что под рукой.
    pub fn focus(&mut self, id: ViewId) {
        let Some(view) = self.views.iter().find(|view| view.id == id) else { return };
        let pane = view.pane;
        self.layout.set_active(pane, Some(id));
        self.focus = pane;
    }

    /// Панель, чьи виджеты сейчас отвечают за события без адресата.
    pub fn focused(&self) -> PaneId {
        self.focus
    }

    /// Дерево панелей — то, по чему собирается экран.
    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    /// Свести всё в одну панель: вкладки собираются в ту, что под рукой, — на
    /// неё и смотрели, — а порядок их сохраняется, он свойство списка.
    pub fn collapse(&mut self) {
        let kept = self.layout.collapse(self.focus);
        for view in &mut self.views {
            view.pane = kept;
        }
        // Активной остаётся та, что была под рукой. Если под рукой была пустая
        // панель — первая же из оставшихся: вкладки есть, и «все вкладки
        // закрыты» вместо них было бы неправдой.
        let active = self.layout.active(kept).or_else(|| self.views.first().map(|view| view.id));
        self.layout.set_active(kept, active);
        self.focus = kept;
    }

    /// Перенести вкладку в названную панель — и туда же перевести взгляд.
    pub fn move_to(&mut self, id: ViewId, pane: PaneId) {
        let Some(at) = self.views.iter().position(|view| view.id == id) else { return };
        let from = self.views[at].pane;
        if !self.layout.contains(pane) {
            return;
        }
        // Переносить внутри своей же панели нечего, но показать — есть что:
        // зовут это и «плюс», которому ответили уже открытым синглтоном, и
        // бросок вкладки в свою полосу. Молчаливый выход отсюда значил бы, что
        // на просьбу «покажи мне поиск здесь» экран не меняется вовсе.
        if from == pane {
            self.layout.set_active(pane, Some(id));
            self.focus = pane;
            return;
        }

        let mut view = self.views.remove(at);
        // Кто останется показанным в прежней панели — считается до вставки:
        // вставка сдвигает индексы, по которым ищется сосед.
        let left_behind = self.neighbour(from, at);
        view.pane = pane;
        let to = self.after_last(pane);
        self.views.insert(to, view);

        if self.layout.active(from) == Some(id) {
            self.layout.set_active(from, left_behind);
        }
        self.layout.set_active(pane, Some(id));
        self.focus = pane;
        self.forget_if_empty(from);
    }

    /// Перенести вкладку туда, где человек видит названную сторону: в соседнюю
    /// панель, а если её там нет — в заведённую с этой стороны.
    ///
    /// Деление экрана поэтому не отдельное действие, а следствие переноса:
    /// делят его не сами по себе, а затем, чтобы что-то показать рядом, — и
    /// пустая панель, которую надо ещё чем-то наполнить, была бы лишним шагом
    /// ровно там, где человек уже сказал, чем.
    pub fn move_aside(&mut self, id: ViewId, side: Side) {
        if !self.can_move(id, side) {
            return;
        }
        let Some(from) = self.pane_of(id) else { return };
        let target = match self.layout.neighbour(from, side) {
            Some(pane) => pane,
            None => match self.layout.split(from, side) {
                Some(pane) => pane,
                None => return,
            },
        };
        self.move_to(id, target);
    }

    /// Положить вкладку рядом с названной панелью, поделив её место, — так
    /// кончается бросок на край (см. `handlers::nav::on_tab_drop`).
    ///
    /// От [`Self::move_aside`] отличается тем, что делит место названной
    /// панели, а не той, в которой вкладка лежала: у броска эти две разные, и
    /// спрашивают его о первой.
    pub fn drop_beside(&mut self, id: ViewId, pane: PaneId, side: Side) {
        // Единственная вкладка своей же панели, брошенная на её край, не
        // двигает ничего: заведённая панель тут же осталась бы одна вместо
        // прежней, а экран — прежним.
        if self.pane_of(id) == Some(pane) && self.views_in(pane).count() < 2 {
            return;
        }
        let Some(fresh) = self.split_pane(pane, side, 0.5) else { return };
        self.move_to(id, fresh);
    }

    /// Изменит ли такой перенос экран. Меню спрашивает об этом заранее: пункт,
    /// который ничего не сделает, обещает выбор и молчит в ответ.
    pub fn can_move(&self, id: ViewId, side: Side) -> bool {
        let Some(from) = self.pane_of(id) else { return false };
        match self.layout.neighbour(from, side) {
            // В соседнюю переносить есть смысл всегда: она другая.
            Some(_) => true,
            // Единственная вкладка панели уедет вместе с самой панелью: та
            // опустеет и уйдёт из дерева, а экран останется прежним.
            None => self.views_in(from).count() > 1,
        }
    }

    pub fn views(&self) -> &[View] {
        &self.views
    }

    /// Вкладки одной панели, в порядке их списка.
    pub fn views_in(&self, pane: PaneId) -> impl Iterator<Item = &View> {
        self.views.iter().filter(move |view| view.pane == pane)
    }

    /// Активная вкладка названной панели — то, что в ней показано.
    pub fn active_in(&self, pane: PaneId) -> Option<ViewId> {
        self.layout.active(pane)
    }

    /// Панель, в которой лежит вкладка. `None` — её уже закрыли.
    pub fn pane_of(&self, id: ViewId) -> Option<PaneId> {
        self.views.iter().find(|view| view.id == id).map(|view| view.pane)
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
    // Именно названного, а не активного: панелей на экране сколько угодно, и
    // «активный вид» на вопрос «чей это щелчок» не отвечает. Кто именно —
    // говорит само сообщение (см. `Msg::In`).

    /// Ширина окна в точках разметки.
    pub fn logical_width(&self) -> f32 {
        self.window.0 as f32 / self.scale.max(1.0)
    }

    /// Высота, доставшаяся дереву панелей: окно без строки состояния под ним.
    /// Ею меряется перетаскивание горизонтальной границы — доли считаются от
    /// места дерева, а не от окна.
    fn layout_height(&self) -> f32 {
        let chrome = crate::module::theme::BAR_HEIGHT + crate::module::theme::HAIRLINE;
        (self.window.1 as f32 / self.scale.max(1.0) - chrome).max(0.0)
    }

    /// Границу деления потянули: сдвиг в точках разметки становится сдвигом
    /// доли. Считается он от места, доставшегося самому делению, — деления
    /// бывают вложенными, и доля вложенного меряется не окном.
    ///
    /// Сами границы в этой арифметике не учитываются: каждая из них тоньше
    /// точности, с которой её тянут мышью.
    pub fn divide(&mut self, split: SplitId, delta: f32) {
        let Some(axis) = self.layout.axis_of(split) else { return };
        let (wide, tall) = self.layout.split_share(split);
        let room = match axis {
            Axis::Row => self.logical_width() * wide,
            Axis::Column => self.layout_height() * tall,
        };
        if room <= 0.0 {
            return;
        }
        self.layout.nudge(split, delta / room, MIN_PANE / room);
    }

    /// Ширина панели, в которой лежит вид, — то, чем меряется место под колонки
    /// списка. Считать его по окну нельзя: панель у́же, а колонки от этого не
    /// худеют, и тянущееся имя схлопнулось бы в ноль (см. `table::fit`).
    ///
    /// Доля берётся у дерева: делений может быть несколько, и вложенные доли
    /// перемножаются (см. `Layout::share`).
    pub fn pane_width(&self, id: ViewId) -> f32 {
        let share = self.pane_of(id).map_or(1.0, |pane| self.layout.share(pane).0);
        self.logical_width() * share
    }

    /// Настройки показа списка: их правят сообщения, одинаковые для всех
    /// списочных видов.
    pub fn listing_mut(&mut self, id: ViewId) -> Option<&mut listing::ListingState> {
        self.get_mut(id)?.listing_mut()
    }

    /// Закрыть раскрытое. Зовут это отовсюду, где действие уводит с того
    /// места, над которым меню стоит.
    pub fn close_menus(&mut self) {
        self.open = Open::Nothing;
    }

    /// Раскрыть названное — или закрыть, если оно уже раскрыто: повторное
    /// нажатие на чип закрывает его меню.
    pub fn toggle_open(&mut self, what: Open) {
        self.open = match self.open == what {
            true => Open::Nothing,
            false => what,
        };
    }

    /// Раскрытое меню списка названной вкладки. `None` — раскрыто не оно:
    /// меню другой вкладки, меню слоя, полоса вкладок или ничего.
    pub fn menu_in(&self, view: ViewId) -> Option<&listing::Menu> {
        match &self.open {
            Open::Listing(id, menu) if *id == view => Some(menu),
            _ => None,
        }
    }

    /// Раскрыто ли меню названного слоя «На просмотре».
    pub fn layer_menu(&self, key: &str) -> bool {
        matches!(&self.open, Open::Layer(open) if open == key)
    }

    /// Раскрыт ли список величин под кадром названной вкладки превью.
    pub fn variables_menu(&self, view: ViewId) -> bool {
        matches!(&self.open, Open::Variables(open) if *open == view)
    }

    /// Раскрыт ли список величин в строке названного слоя «На просмотре».
    pub fn layer_variables(&self, key: &str) -> bool {
        matches!(&self.open, Open::LayerVariables(open) if open == key)
    }

    /// Смена отбора в списке: страница сбрасывается, раскрытое гаснет. Общий
    /// хвост у всех правок отбора — набранное меняет состав списка, и прежняя
    /// страница относилась к другому списку.
    pub fn refine(&mut self, view: ViewId) {
        self.close_menus();
        if let Some(listing) = self.listing_mut(view) {
            listing.page = 0;
        }
    }


    /// Ключ снимка, обведённого на шаре лентой; пусто — подсвечено не это.
    pub fn picked_key(&self) -> &str {
        self.highlight.as_ref().filter(|it| it.on_globe).map_or("", |it| it.key.as_str())
    }

    /// Строка, к которой привёл переход в названной вкладке; пусто — переход
    /// был не сюда либо его не было.
    pub fn target_in(&self, view: ViewId) -> &str {
        self.highlight
            .as_ref()
            .filter(|it| it.view == Some(view))
            .map_or("", |it| it.key.as_str())
    }

    /// Погасить ленту на шаре, не трогая подсветку перехода: ставил её не он.
    /// `true` — было что гасить.
    pub fn deselect(&mut self) -> bool {
        let Some(highlight) = self.highlight.as_mut().filter(|it| it.on_globe) else {
            return false;
        };
        highlight.on_globe = false;
        // Подсветка без ленты и без вкладки не подсвечивает ничего.
        if highlight.view.is_none() {
            self.highlight = None;
        }
        true
    }

    /// Погасить подсветку перехода в названной вкладке: ушли с её страницы, и
    /// на другой она говорила бы неправду. Лента на шаре остаётся.
    pub fn drop_target_in(&mut self, view: ViewId) {
        let Some(highlight) = self.highlight.as_mut().filter(|it| it.view == Some(view)) else {
            return;
        };
        highlight.view = None;
        if !highlight.on_globe {
            self.highlight = None;
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

    /// Выбранный щелчком по шару снимок вместе с геометрией. `None` — не
    /// выбран ни один: щёлкнули мимо контуров либо убрали контур, и
    /// контура больше нет.
    pub fn picked(&self) -> Option<&globe::Outlined> {
        let key = self.picked_key();
        self.outlined.iter().find(|outlined| outlined.key == key)
    }

}

/// Поддаётся ли выбранное пакетному действию — по ответу на действие.
///
/// Считает его сам обработчик (`handlers::library::batch`), тем же перебором,
/// каким потом и действует: числа эти решают, показывать ли кнопку, и второй
/// ответ на вопрос «что будет сделано» однажды разошёлся бы с первым — кнопка
/// обещала бы действие и не совершала его.
pub struct Batch {
    /// Есть ли что удалять — хоть одна запись библиотеки.
    pub deletable: bool,
    /// Есть ли что ставить в закачку.
    pub fetchable: bool,
}

pub mod search;
pub mod browse;
pub mod globe;
pub mod layout;
pub mod library;
pub mod listing;
pub mod overlay;
pub mod preview;

pub use layout::{Axis, Layout, Node, PaneId, Side, SplitId};

pub use browse::BrowseState;
pub use globe::GlobeState;
pub use library::LibraryState;
pub use preview::PreviewState;
pub use search::SearchState;
