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

/// Всё, что таблица знает про набор сообщений: как каждое объявляется в
/// разметке, кого адресует и как читается обратно.
///
/// Трейтом, а не тройкой свободных функций: `Msg` спрашивает у вложенного
/// набора ровно то же, чем отвечает сам (см. [`Msg::In`]).
trait Wired: Sized {
    /// Имя метода и объявленная нагрузка.
    fn declared(&self) -> (&'static str, String);
    /// Кого сообщение адресует; пусто — никого, и это законно.
    fn named(&self) -> String;
    /// `None` — метод не наш: это сообщение набору не принадлежит.
    fn parse(event: &UiEventResponse) -> Option<Self>;
}

/// Объявляет набор сообщений вместе с тем, как каждое ездит по шине.
///
/// Одним местом на четыре списка: сам вариант, имя метода, сборка нагрузки и её
/// разбор. Написанные порознь, они держатся вниманием, и держатся неодинаково:
/// сборка матчится по перечислению исчерпывающе, и забыть её нельзя, а разбор
/// смотрит на имя метода строкой и кончается «прочее — не наше», поэтому
/// забытый рукав разбора компилируется молча и превращает нажатие в
/// «непонятное сообщение разметки» в логе. Из таблицы порождаются оба, и
/// забыть разбор больше не из чего: одно сообщение — одна строка.
///
/// Строка называет вариант, его поля и имя метода. Перед полем стоит его род —
/// то, каким полем `UiEventResponse` оно ездит:
///
/// * `val` — нагрузка, названная самой разметкой;
/// * `sub` — нагрузка, которую подставляет рендерер: объявлять её нечем, а
///   читается она так же, как всякая другая;
/// * `key` — адресат.
///
/// Как ездит нагрузка каждого типа, сказано по разу на тип — в [`Value`] и
/// [`Addressee`], — а не по разу на сообщение.
///
/// Хвостом за таблицей объявляется вариант, у которого своего имени метода
/// нет: его называет вложенный набор, им же оно и опознаётся при разборе.
/// Строкой таблицы такое не выразить, поэтому оно стоит отдельно — и потому же
/// последним в перечислении.
macro_rules! messages {
    (
        $(#[$outer:meta])*
        $name:ident {
            $(
                $(#[$doc:meta])*
                $variant:ident $(( $($slot:ident $ty:ty),+ ))? = $method:literal;
            )*
        }
        $(
            $(#[$tail_doc:meta])*
            $tail:ident(key $tail_key:ty, nested $tail_msg:ty);
        )?
    ) => {
        $(#[$outer])*
        pub enum $name {
            $( $(#[$doc])* $variant $(( $($ty),+ ))?, )*
            $( $(#[$tail_doc])* $tail($tail_key, $tail_msg), )?
        }

        impl Wired for $name {
            fn declared(&self) -> (&'static str, String) {
                match self {
                    $(
                        $name::$variant $(( $( messages!(@declaring $slot $slot) ),+ ))? =>
                            ($method, messages!(@declare $($( $slot $slot )+)?)),
                    )*
                    $( $name::$tail(_, inner) => inner.declared(), )?
                }
            }

            fn named(&self) -> String {
                match self {
                    $(
                        $name::$variant $(( $( messages!(@naming $slot $slot) ),+ ))? =>
                            messages!(@name $($( $slot $slot )+)?),
                    )*
                    $( $name::$tail(view, _) => view.name(), )?
                }
            }

            fn parse(event: &UiEventResponse) -> Option<Self> {
                Some(match event.method.as_str() {
                    $(
                        $method =>
                            $name::$variant $(( $( messages!(@read $slot $ty, event) ),+ ))?,
                    )*
                    _ => return messages!(@rest event $(, $name $tail $tail_key, $tail_msg)?),
                })
            }
        }
    };

    // Поле называет себя только в той сборке, которая его и спрашивает: связанное
    // и неиспользованное — это предупреждение компилятора на каждую строку
    // таблицы.
    (@declaring val $bound:ident) => { $bound };
    (@declaring sub $bound:ident) => { _ };
    (@declaring key $bound:ident) => { _ };
    (@naming key $bound:ident) => { $bound };
    (@naming val $bound:ident) => { _ };
    (@naming sub $bound:ident) => { _ };

    // Нагрузку несёт не больше одного поля, адресата — тоже: остальные поля
    // обходятся, а не найдя своего — пусто.
    (@declare) => { String::new() };
    (@declare val $bound:ident $($rest:tt)*) => { $bound.declare() };
    (@declare sub $bound:ident $($rest:tt)*) => { messages!(@declare $($rest)*) };
    (@declare key $bound:ident $($rest:tt)*) => { messages!(@declare $($rest)*) };
    (@name) => { String::new() };
    (@name key $bound:ident $($rest:tt)*) => { $bound.name() };
    (@name val $bound:ident $($rest:tt)*) => { messages!(@name $($rest)*) };
    (@name sub $bound:ident $($rest:tt)*) => { messages!(@name $($rest)*) };

    (@read val $ty:ty, $event:ident) => { <$ty as Value>::read($event)? };
    (@read sub $ty:ty, $event:ident) => { <$ty as Value>::read($event)? };
    (@read key $ty:ty, $event:ident) => { <$ty as Addressee>::read($event)? };

    // Имя не из таблицы: либо его знает вложенный набор, либо оно не наше.
    (@rest $event:ident) => { None };
    (@rest $event:ident, $name:ident $tail:ident $tail_key:ty, $tail_msg:ty) => {
        Some($name::$tail(
            <$tail_key as Addressee>::read($event)?,
            <$tail_msg as Wired>::parse($event)?,
        ))
    };

    // Числа ездят строкой и разбираются обратно ею же: негодная строка — не
    // число, а значит и не сообщение.
    (@numbers $($ty:ty),+ $(,)?) => { $(
        impl Value for $ty {
            fn declare(&self) -> String {
                self.to_string()
            }

            fn read(event: &UiEventResponse) -> Option<Self> {
                event.value().parse().ok()
            }
        }
    )+ };

    // Нагрузка, которую изготавливает рендерер: объявить её в разметке нечем —
    // где какая зона и куда встал указатель, знает только он.
    (@substituted $($ty:ty => $carried:ident),+ $(,)?) => { $(
        impl Value for $ty {
            fn declare(&self) -> String {
                String::new()
            }

            fn read(event: &UiEventResponse) -> Option<Self> {
                event.$carried().cloned()
            }
        }
    )+ };

    // Наборы `Choice` ездят своим ключом — все одинаково.
    (@choices $($ty:ty),+ $(,)?) => { $(
        impl Value for $ty {
            fn declare(&self) -> String {
                self.key().to_string()
            }

            fn read(event: &UiEventResponse) -> Option<Self> {
                <$ty>::from_key(event.value())
            }
        }
    )+ };

    // Идентификаторы адресуют так же — и сами по себе, и необязательные.
    (@ids $($ty:ty),+ $(,)?) => { $(
        impl Addressee for $ty {
            fn name(&self) -> String {
                self.to_string()
            }

            /// Разбор идентификатора — здесь и только здесь: обратно он
            /// приезжает строкой, и место, где строка снова становится
            /// идентификатором, должно быть одно.
            fn read(event: &UiEventResponse) -> Option<Self> {
                event.key.parse().ok()
            }
        }

        impl Addressee for Option<$ty> {
            fn name(&self) -> String {
                self.map(|id| id.to_string()).unwrap_or_default()
            }

            /// Неразобранное имя — «никого»: пустым ключом едет и «закрой», и
            /// сообщение, которому адресат не нужен.
            fn read(event: &UiEventResponse) -> Option<Self> {
                Some(event.key.parse().ok())
            }
        }
    )+ };
}

messages! {
    Msg {
        // -- Вкладки --
        //
        // Вкладка адресуется идентификатором, а не позицией: позиция меняется,
        // когда закрывают соседа.
        TabSelect(key ViewId) = "tab_select";
        TabClose(key ViewId) = "tab_close";
        /// Меню «плюса» названной панели; `None` — закрыть раскрытое.
        TabMenu(key Option<PaneId>) = "tab_menu";
        /// Меню самой вкладки: перенести, закрыть. `None` — закрыть.
        TabOptions(key Option<ViewId>) = "tab_options";
        /// Завести вкладку в названной панели.
        NewTab(key PaneId, val NewTab) = "new_tab";
        /// Перенести вкладку туда, где человек видит эту сторону: в соседнюю
        /// панель либо в заведённую с этой стороны (см. `State::move_aside`).
        TabMove(key ViewId, val Side) = "tab_move";
        /// Свести все панели в одну.
        TabCollapse = "tab_collapse";
        /// Границу деления потянули: сдвиг в точках разметки. Что это в долях,
        /// знает только тот, кто их задавал (см. `State::divide`).
        Divide(key SplitId, sub f32) = "divide";
        /// Границу отпустили. Отдельным сообщением, а не концом потока сдвигов:
        /// «перетаскивание кончилось» — момент, а не отсутствие событий, и ждать
        /// его молчанием значило бы не дождаться вовсе.
        Divided = "divided";
        /// В панель бросили вкладку. Что принесли и в какой край панели попали,
        /// изготавливает рендерер: где какая зона, знает только он (см. `Drop` в
        /// ui-service/types.proto). Нагрузка едет как есть — разбирает её
        /// обработчик, ровно как у указателя над областью.
        TabDrop(key PaneId, sub DropEvent) = "tab_drop";

        // -- Записи --
        //
        // Действуют не на вид, а на библиотеку, поэтому вкладки-источника не несут:
        // скачанное одно на всё окно, из какой панели его ни попроси.
        //
        // Снимок у закачки едет ключом: нагрузку занял сам файл, а вопрос «к чему
        // его отнести» — это тот же вопрос «с кем», на который ключ и отвечает.
        /// Скачать, докачать или приостановить — по ключу провайдера. Второе поле —
        /// снимок, к которому файл относится (пусто — сам по себе): библиотека
        /// пишет его в свой снимок, а вывести это из ключа не может.
        Download(val String, key String) = "download";
        /// Выбросить запись: удалить скачанное или отказаться от начатого. Одно
        /// сообщение на оба, потому что оставляют они после себя одно и то же —
        /// ничего; разными их делает только подпись в меню.
        Delete(val String) = "delete";
        /// Выбросить снимок целиком — все его файлы. Ключом здесь снимок, а не
        /// запись: «снимок» — понятие показа (библиотека ведёт учёт файлам), и
        /// разворачивает его в записи тот, кто его и собрал (см.
        /// `handlers::library::on_delete_snapshot`).
        DeleteSnapshot(val String) = "delete_snapshot";
        /// Скачать снимок целиком — по ключу продукта. Список файлов приезжает
        /// рекурсивным листингом от провайдера: раскладку .SAFE знает только он, а
        /// закачка идёт по одному файлу (см. `handlers::library::on_download_snapshot`).
        DownloadSnapshot(val String) = "download_snapshot";
        /// Приостановить закачку снимка целиком. Ключом снимок — по той же
        /// причине, что и у выброса: разворачивает его в файлы тот, кто его собрал.
        PauseSnapshot(val String) = "pause_snapshot";
        /// Показать запись в файловом менеджере. Путь считает библиотека — раскладка
        /// хранения её, — а показывает платформа.
        Reveal(val String) = "reveal";

        // -- Шар --
        /// Снять с шара всё: и наложения, и контуры. Порознь их не снять ничем, а
        /// разделять их пользователю не по чему — он видит один шар.
        GlobeClear = "globe_clear";

        // -- На просмотре --
        //
        // Слой адресуется ключом продукта, и едет он тем же полем, что вкладка у
        // `In`: вопрос у них один — «с кем», — и отвечать на него двумя способами
        // было бы незачем.
        /// Прозрачность слоя, 0..1.
        OverlayOpacity(key String, sub f32) = "overlay_opacity";
        /// Скрыть слой или вернуть его на шар.
        OverlayHidden(key String, val bool) = "overlay_hidden";
        /// Убрать слой совсем: ресурсы отпускаются, вернуть его — это открыть
        /// снимок заново.
        OverlayRemove(key String) = "overlay_remove";
        /// Подвинуть слой в наборе: порядок набора — это порядок слоёв на шаре
        /// снизу вверх, и другого способа сказать «этот поверх того» нет.
        OverlayShift(key String, val Shift) = "overlay_shift";
        /// Скрыть или показать все слои разом.
        OverlayHideAll(val bool) = "overlay_hide_all";
        /// Меню слоя: порядок и переходы к снимку. `None` — закрыть раскрытое.
        OverlayMenu(key Option<String>) = "overlay_menu";
        /// Раскрыть список величин лежащего подробным файла слоя или закрыть
        /// раскрытый (`None`).
        OverlayVariables(key Option<String>) = "overlay_variables";
        /// Показать на шаре другую величину файла слоя — путь из списка.
        OverlayVariable(key String, val String) = "overlay_variable";

        // -- Контуры --
        //
        // Контур адресуется тем же ключом снимка, что и слой, и тем же полем: на
        // шаре они про один и тот же снимок, просто с разной подробностью.
        /// Очертить снимок на шаре или убрать контур — значок в строке списка.
        /// Ни выбора коробочкой, ни показа растром это не касается: три
        /// состояния строки независимы (см. handlers::outline).
        OutlineToggle(key String) = "outline_toggle";
        /// Убрать контур — строка списка «На просмотре», где он стои́т своей
        /// строкой и убрать его больше неоткуда.
        OutlineRemove(key String) = "outline_remove";
        /// Навести шар на контур и выбрать его.
        OutlineFocus(key String) = "outline_focus";
    }
    // Своего имени метода у этого варианта нет — его называет вложенное
    // сообщение, им же оно и опознаётся при разборе; строкой таблицы такое не
    // выразить.
    /// Сообщение, рождённое в теле названной вкладки.
    In(key ViewId, nested ViewMsg);
}

messages! {
    /// То, что говорит виджет внутри вкладки. Отдельный тип, а не два десятка
    /// вариантов с `ViewId` первым полем: адресат у них общий и приписывается один
    /// раз — в [`Msg::In`].
    ViewMsg {
        /// Чем наполнить пустую вкладку. Ей, а не панели: вкладка становится
        /// выбранным видом на месте, не заводя рядом вторую и не оставляя после
        /// себя пустую (см. `handlers::nav::fill`).
        Fill(val NewTab) = "fill";

        // -- Показ списка --
        /// Раскрыть меню или закрыть раскрытое (`None`).
        OpenMenu(val Option<Menu>) = "open_menu";
        Filter(val Filter) = "filter";
        Group(val Grouping) = "group";
        Sort(val Sorting) = "sort";
        /// Набранное в поле фильтра — его подставляет рендерер, а не разметка.
        Query(val String) = "query";
        Page(val usize) = "page";
        /// Раскрыть строку в её содержимое или свернуть обратно.
        Expand(val String) = "expand";
        /// Выбрать строку или снять выбор — набор для пакетных действий.
        /// Шара это не касается: контур и показ ставят свои значки.
        Check(val String) = "check";
        /// Выбрать или снять выбор разом со всего, что показано, — коробочка в
        /// шапке. Действует ровно на то, о чём говорит: на выбираемые строки
        /// этой страницы.
        CheckShown(val bool) = "check_shown";
        /// Снять весь выбор списка, включая сделанный в другой папке, — кнопка в
        /// заголовке. Отдельное сообщение, а не `CheckShown(false)`: выбор
        /// переживает переход по папкам, и «снять видимое» оставило бы набранным
        /// то, чего на экране уже нет.
        CheckClear = "check_clear";
        /// Скачать всё выбранное в этом списке.
        CheckDownload = "check_download";
        /// Удалить всё выбранное в этом списке — и с диска, и из очереди
        /// закачек.
        CheckDelete = "check_delete";

        // -- Поиск по каталогу --
        //
        // Отбор в списке (`Query`) и запрос к каталогу — разные вещи, и путать их
        // нельзя: первый сужает уже найденное, второй идёт по сети и меняет то,
        // что вообще нашлось.
        /// Набранное в поле запроса.
        SearchQuery(val String) = "search_query";
        /// Чем сузить запрос. Каждое из трёх отправляет его заново: выбор сделан
        /// одним нажатием, и спрашивать после него ещё и подтверждения не за что.
        SearchMission(val Mission) = "search_mission";
        SearchPeriod(val Period) = "search_period";
        /// Края своего интервала — набранное в полях дат (см. `Period::Custom`).
        SearchFrom(val String) = "search_from";
        SearchTo(val String) = "search_to";
        SearchCloud(val Cloud) = "search_cloud";
        /// Отправить запрос каталогу.
        RunSearch = "run_search";

        // -- Каталог --
        /// Перейти в папку по ключу провайдера; пустой ключ — перечитать текущую.
        Enter(val String) = "enter";
        Up = "up";
        /// Показать названную запись в каталоге: открыть папку, в которой она
        /// лежит, встать на её страницу и подсветить её.
        ///
        /// Отдельно от [`ViewMsg::Enter`], потому что вопрос другой: тот называет
        /// папку и ведёт в неё, этот называет запись и ведёт к ней. Каталог при
        /// этом переиспользуется — новую вкладку заводит только тот, кому её
        /// попросили завести (см. `handlers::nav::catalog`).
        InCatalog(val String) = "in_catalog";

        // -- Просмотр снимка --
        //
        // Открывают новую вкладку — в той же половине, где стоит строка: смотреть
        // рядом со списком и есть то, ради чего экран делят.
        /// Смотреть скачанное — по имени записи библиотеки.
        Preview(val String) = "preview";
        /// Смотреть ещё не скачанное — по ключу провайдера.
        PreviewRemote(val String) = "preview_remote";
        /// Показать снимок, лежащий папкой: растр внутри выбирает провайдер
        /// (см. `handlers::preview::on_view_product_pressed`).
        PreviewProduct(val String) = "preview_product";

        // -- Канва превью --
        //
        // Нагрузку области подставляет рендерер (как у глобуса); кнопки тулбара
        // едут своими сообщениями.
        /// Области под кадр досталось новое место — в пикселях её текстуры.
        PreviewResized(sub ViewportSize) = "preview_resized";
        /// Указатель над канвой, в тех же пикселях.
        PreviewPointer(sub PointerEvent) = "preview_pointer";
        /// Вписать снимок в канву.
        PreviewFit = "preview_fit";
        /// Шаг масштаба вокруг центра: знак — направление.
        PreviewZoom(val f32) = "preview_zoom";
        /// Раскрыть список величин файла под кадром или закрыть его.
        PreviewVariables(val bool) = "preview_variables";
        /// Показать другую величину файла — путь из списка под кадром.
        PreviewVariable(val String) = "preview_variable";

        // -- Глобус --
        //
        // Нагрузку у обоих подставляет рендерер, поэтому в разметке они объявлены
        // конструкторами, а не готовыми значениями (см. `viewport`).
        /// Области под шар досталось новое место — в пикселях её текстуры.
        GlobeResized(sub ViewportSize) = "globe_resized";
        /// Указатель над областью, в тех же пикселях.
        GlobePointer(sub PointerEvent) = "globe_pointer";
        /// Положить снимок на шар растром — или снять его оттуда. По ключу
        /// провайдера; нагрузка своя — приходит не от области, а из строки
        /// списка.
        ///
        /// Переключателем, и тем же именем, что у контура (`outline_toggle`):
        /// вопрос у них один — лежит ли снимок на шаре, — и разной механикой
        /// они читались бы как два разных рода действия. Камеры это не
        /// касается: наводит `outline_focus`.
        GlobeToggle(val String) = "globe_toggle";
    }
}

impl UiMessage for Msg {
    fn encode(&self) -> (String, String) {
        let (method, value) = self.declared();
        (method.to_string(), value)
    }

    /// Кого адресует сообщение: вкладку, панель или слой на шаре.
    ///
    /// Одинаковых виджетов на экране столько же, сколько вкладок и снимков, и
    /// различить их по имени метода нельзя — а нагрузка у половины из них
    /// занята рендерером (см. `Handler.key` в ui-service/types.proto).
    fn key(&self) -> String {
        self.named()
    }

    fn decode(event: &UiEventResponse) -> Option<Self> {
        Msg::parse(event)
    }
}

// -- Как ездят нагрузка и адресат --
//
// По разу на тип, а не на сообщение: строка ездит строкой одинаково у всех двух
// десятков сообщений, которые её несут, и правило это принадлежит типу.

/// Нагрузка: поле `Handler.value` туда и `UiEventResponse.payload` обратно.
///
/// `read` отвечает `None`, когда нагрузка не годится, и разбор всего сообщения
/// на этом обрывается. Проверять при этом вид нагрузки не нужно: у события
/// другого вида строка пуста, и разбор пустой строки числом или ключом набора
/// сам скажет, что читать нечего.
trait Value: Sized {
    /// Что объявить в разметке.
    fn declare(&self) -> String;
    fn read(event: &UiEventResponse) -> Option<Self>;
}

/// Адресат: поле `Handler.key`. Отдельно от нагрузки — см. заголовок файла.
trait Addressee: Sized {
    fn name(&self) -> String;
    fn read(event: &UiEventResponse) -> Option<Self>;
}

impl Value for String {
    fn declare(&self) -> String {
        self.clone()
    }

    fn read(event: &UiEventResponse) -> Option<Self> {
        Some(event.value().to_string())
    }
}

impl Value for bool {
    fn declare(&self) -> String {
        self.to_string()
    }

    /// Всё, что не «true», — «false»: третьего состояния у коробочки нет, и
    /// незнакомое слово значит то же, что снятая отметка.
    fn read(event: &UiEventResponse) -> Option<Self> {
        Some(event.value() == "true")
    }
}

messages!(@numbers usize, f32);
messages!(@choices NewTab, Filter, Grouping, Sorting, Mission, Period, Cloud, Side, Shift);
messages!(@substituted ViewportSize => size, PointerEvent => pointer, DropEvent => drop);

/// Меню — набор, у которого «ничего не раскрыто» законное значение: пустым
/// именем едет и «закрой», и незнакомое имя, а показывать по нему нечего
/// (см. [`Menu::from_key`]). Поэтому чтение здесь не отказывает.
impl Value for Option<Menu> {
    fn declare(&self) -> String {
        self.as_ref().map(Menu::key).unwrap_or_default()
    }

    fn read(event: &UiEventResponse) -> Option<Self> {
        Some(Menu::from_key(event.value()))
    }
}

impl Addressee for String {
    fn name(&self) -> String {
        self.clone()
    }

    fn read(event: &UiEventResponse) -> Option<Self> {
        Some(event.key.clone())
    }
}

impl Addressee for Option<String> {
    fn name(&self) -> String {
        self.clone().unwrap_or_default()
    }

    /// Пустое имя — «никого»: имени у слоя пустым не бывает, поэтому пустым
    /// ключом едет «закрыть раскрытое».
    fn read(event: &UiEventResponse) -> Option<Self> {
        Some(Some(event.key.clone()).filter(|key| !key.is_empty()))
    }
}

messages!(@ids ViewId, PaneId, SplitId);

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
    /// encode без изменений. Ровно тот класс расхождений сборки и разбора, ради
    /// которого они порождаются одной таблицей.
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
            Msg::OverlayVariables(Some("продукт".into())),
            Msg::OverlayVariables(None),
            Msg::OverlayVariable("продукт".into(), "/PRODUCT/qa_value".into()),
            Msg::OutlineToggle("продукт".into()),
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
            Msg::In(id(), ViewMsg::CheckDownload),
            Msg::In(id(), ViewMsg::CheckDelete),
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
            Msg::In(id(), ViewMsg::PreviewVariables(true)),
            Msg::In(id(), ViewMsg::PreviewVariables(false)),
            Msg::In(id(), ViewMsg::PreviewVariable("/PRODUCT/qa_value".into())),
            Msg::In(id(), ViewMsg::GlobeToggle("продукт".into())),
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
                Msg::decode(&event).unwrap_or_else(|| panic!("«{}» не разобралось", event.method));
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
            ("globe_resized", Wire::Size(size.clone())),
            ("preview_resized", Wire::Size(size.clone())),
            ("globe_pointer", Wire::Pointer(pointer.clone())),
            ("preview_pointer", Wire::Pointer(pointer.clone())),
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
            method: "overlay_opacity".to_string(),
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
            method: "divide".to_string(),
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
            method: "run_search".to_string(),
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
