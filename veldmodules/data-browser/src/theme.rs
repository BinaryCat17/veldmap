//! theme.rs — как выглядит приложение: палитра и роли.
//!
//! Роль, а не оформление: разметка просит «вкладку», «чип», «строку списка»
//! или «панель меню», а как они выглядят, знает только этот файл. Цвет,
//! написанный где-то ещё в разметке, — ошибка: набор их конечен и живёт здесь.
//!
//! Рендереры чужих областей (globe.wgsl) — не разметка: как выглядит их
//! содержимое, решает рисующий, и цвета в контракте нет намеренно. Свои
//! константы они держат согласованными с этой палитрой — сменив её, загляните
//! и туда.

use crate::proto::ui_service::{
    Border, Background, Color, Padding, ProgressBarStyle, Scrollbar, Shadow,
    SliderStyle, TextInputStyle, WidgetStyle, Wrapping, Container, Element, Length, container,
    icon, space, text,
};

// --- Глифы ---
//
// Словарь иконок (шрифт Icons) — один на приложение: вкладка, меню и таблица
// подписывают одну сущность одним знаком, и сменившийся глиф не должен
// разводить их между собой.
//
// Шрифт — Nerd Fonts Symbols, и большинство знаков здесь из блока Font Awesome
// (U+F000–F2FF). Те, которых в ней нет вовсе — спутник, перечёркнутый глаз,
// слои, — взяты из Material Design Icons: то же семейство, другой блок, оттого
// и коды в пять знаков.
pub mod glyph {
    pub const PLUS: &str = "\u{f067}";
    pub const CLOSE: &str = "\u{f00d}";
    pub const FOLDER: &str = "\u{f07b}";
    pub const SEARCH: &str = "\u{f002}";
    pub const DOWNLOAD: &str = "\u{f019}";
    pub const IMAGE: &str = "\u{f1c5}";
    pub const GLOBE: &str = "\u{f0ac}";
    pub const FILE: &str = "\u{f016}";
    pub const ENTER: &str = "\u{f061}";
    pub const PAUSE: &str = "\u{f04c}";
    pub const PLAY: &str = "\u{f04b}";
    pub const EYE: &str = "\u{f06e}";
    pub const EYE_OFF: &str = "\u{f0209}";
    pub const TRASH: &str = "\u{f1f8}";
    pub const LAYERS: &str = "\u{f0328}";
    /// Контур снимка на шаре: многоугольник с вершинами.
    pub const OUTLINE: &str = "\u{f0559}";
    /// Навести камеру: перекрестье прицела. Не фотоаппарат, хотя речь о
    /// камере: фотоаппаратом в этом списке читался бы снимок, а их тут полсотни
    /// в столбце.
    pub const FOCUS: &str = "\u{f05b}";
    /// Разделение экрана: две колонки рядом.
    pub const SPLIT: &str = "\u{f0db}";
    pub const MORE: &str = "\u{f141}";
    pub const CARET: &str = "\u{f0d7}";
    /// Тот же треугольник в закрытом положении. Пара к CARET, а не общая
    /// стрелка: раскрытие и переход вглубь — разные действия, и знаки у них
    /// разные (переход показан стрелкой ENTER).
    pub const CARET_RIGHT: &str = "\u{f0da}";
    /// Коробочка пакетного выделения: пустая и с галочкой.
    pub const BOX: &str = "\u{f096}";
    pub const BOX_TICKED: &str = "\u{f046}";
    /// Снимок — логическая единица, а не файл и не папка: его собирает декодер
    /// (см. `RowKind::Product`).
    pub const SATELLITE: &str = "\u{f0471}";
    pub const UP: &str = "\u{f062}";
    pub const DOWN: &str = "\u{f063}";
    pub const LEFT: &str = "\u{f053}";
    pub const RIGHT: &str = "\u{f054}";
    pub const TICK: &str = "\u{f00c}";
}

// --- Палитра ---
//
// Три группы: бумага (фон и поверхности), чернила (текст) и акценты
// (состояние). Внутри группы цвета идут от тихого к громкому.

pub const PAGE: Color = Color::rgb8(0xFB, 0xF8, 0xF1);
/// Хром: полоса вкладок, шапка таблицы, подвал — всё, что не содержимое.
pub const CHROME: Color = Color::rgb8(0xF1, 0xEA, 0xDA);
/// Подложка группирующих полос: шапка таблицы, строка пагинации.
pub const SHELF: Color = Color::rgb8(0xF5, 0xEF, 0xE0);
/// Поверхность управляемого элемента: кнопка, поле, панель меню. Дальше ролей
/// эти цвета не выходят — снаружи их незачем знать, там просят роль.
const SURFACE: Color = Color::rgb8(0xFF, 0xFD, 0xF8);
/// Наведение — общее на кнопки, пункты меню и чипы.
const HOVER: Color = Color::rgb8(0xF1, 0xEA, 0xDA);
/// Наведение на строку списка: слабее кнопочного, строк много.
const ROW_HOVER: Color = Color::rgb8(0xF7, 0xF2, 0xE5);
/// Погашенная строка: скрытый слой, снимок одним контуром. Не [`SHELF`] — тем
/// в этом же списке нарисованы шапка и черта между группами, и залитая им
/// строка читалась бы заголовком раздела. Этот гасит, а тот группирует.
const ROW_DIM: Color = Color::rgb8(0xF4, 0xF1, 0xEA);
const ROW_DIM_HOVER: Color = Color::rgb8(0xEE, 0xEA, 0xE1);

pub const LINE: Color = Color::rgb8(0xDC, 0xD3, 0xBF);
/// Разделитель внутри содержимого — тише рамки.
pub const LINE_SOFT: Color = Color::rgb8(0xE3, 0xDC, 0xCB);
/// Черта под строкой списка: её видно только рядом с соседней строкой.
pub const LINE_ROW: Color = Color::rgb8(0xEF, 0xE8, 0xD8);
/// Рамка нажимаемого: заметнее, чем разделитель.
pub const LINE_STRONG: Color = Color::rgb8(0xC9, 0xBF, 0xA6);

pub const INK: Color = Color::rgb8(0x2E, 0x2A, 0x22);
pub const INK_SOFT: Color = Color::rgb8(0x3D, 0x38, 0x2C);
pub const INK_MUTED: Color = Color::rgb8(0x5C, 0x56, 0x48);
pub const INK_DIM: Color = Color::rgb8(0x6B, 0x64, 0x55);
/// Служебное: единицы, колонка формата, счётчики.
pub const INK_FAINT: Color = Color::rgb8(0x94, 0x8B, 0x78);

pub const ACCENT: Color = Color::rgb8(0x4A, 0x7A, 0x3F);
/// Подпись состояния «готово»: тот же зелёный, но читаемый как текст.
pub const ACCENT_TEXT: Color = Color::rgb8(0x3D, 0x65, 0x34);
/// Чем закрашивается место, куда упадёт принесённая вкладка. Прозрачным, а не
/// плотным: под ним видно то, поверх чего она ляжет, — иначе обещание закрывало
/// бы собой то, о чём оно.
pub const DROP_HINT: Color = ACCENT.with_alpha(0.22);
/// Полоса работы, у которой нет доли: тот же акцент вполсилы. Заливается ею
/// вся дорожка — сказать «идёт» иначе нечем, — и вполсилы здесь единственное,
/// что отличает её от честно набранной единицы.
pub const ACCENT_HALF: Color = ACCENT.with_alpha(0.35);
/// Заливка выбранного: активная страница пагинации, выбранный пункт меню,
/// подсвеченная строка.
const ACCENT_WASH: Color = Color::rgb8(0xDC, 0xE7, 0xD2);
const ACCENT_WASH_HOVER: Color = Color::rgb8(0xD2, 0xE0, 0xC6);
/// Заливка отмеченного — та же зелень вполсилы. Слабее выбранного намеренно:
/// выбрано на экране одно, а отмечено бывает полсписка, и равная заливка
/// стёрла бы разницу между «это главное сейчас» и «этих я набрал».
const ACCENT_TINT: Color = Color::rgb8(0xEE, 0xF3, 0xE7);
/// Подложка предупреждающего значка и её край — та же охра, что у `WARN`,
/// разбавленная до бумаги. Своя пара, а не разбавленный акцент: значок этот
/// стои́т в ряду зелёных, и полутон зелени читался бы как «почти сделано».
const WARN_WASH: Color = Color::rgb8(0xF6, 0xE8, 0xD2);
const WARN_LINE: Color = Color::rgb8(0xE0, 0xC2, 0x92);
const ACCENT_TINT_HOVER: Color = Color::rgb8(0xE5, 0xED, 0xDA);
const ACCENT_LINE: Color = Color::rgb8(0xC3, 0xD6, 0xB4);

pub const WARN: Color = Color::rgb8(0xC4, 0x79, 0x1F);
pub const WARN_TEXT: Color = Color::rgb8(0x8A, 0x62, 0x15);
pub const DANGER: Color = Color::rgb8(0xB4, 0x50, 0x3C);
/// Подпись отказа: тот же кирпич, но читаемый как текст — как `ACCENT_TEXT`
/// для готового и `WARN_TEXT` для прерванного. Кружок и подпись рядом, и
/// набранная цветом кружка подпись выцветает на светлом фоне.
pub const DANGER_TEXT: Color = Color::rgb8(0x8C, 0x3E, 0x2F);
/// Дорожка полосы загрузки — под любым её цветом.
const TRACK: Color = Color::rgb8(0xE7, 0xDF, 0xCB);

// --- Размеры ---
//
// Метрики, от которых зависит не одно место: их согласованность и есть сетка.

/// Скругление всего мелкого: кнопки, чипы, поля.
const RADIUS: f32 = 7.0;
/// Скругление квадратной кнопки-иконки и пункта меню — чуть меньше, они мельче.
const RADIUS_SMALL: f32 = 6.0;
/// Высота строки списка. От неё считается, сколько строк влезает на страницу.
pub const ROW_HEIGHT: f32 = 30.0;
/// Толщина волосяной линии — ею разделено всё, что лежит полосами
/// (см. [`hairline`]).
pub const HAIRLINE: f32 = 1.0;
/// Шаг строки списка на экране: сама строка и черта под ней. По нему считает
/// прокрутку тот, кто ведёт к строке, — и без общего имени он разошёлся бы с
/// разметкой молча.
pub const ROW_PITCH: f32 = ROW_HEIGHT + HAIRLINE;
/// Высота управляющего элемента в тулбаре: поле фильтра и чипы.
pub const CONTROL_HEIGHT: f32 = 31.0;
/// Сторона квадратной кнопки в строке списка.
pub const ROW_BUTTON: f32 = 23.0;
/// Отступ содержимого от краёв экрана.
pub const GUTTER: f32 = 16.0;
/// Высота однострочной полосы хрома: строка состояния, подпись под областью.
/// Одна на всех — полосы одного роста читаются как один язык.
pub const BAR_HEIGHT: f32 = 30.0;
/// Высота заголовка экрана: название списка и подпись под ним. Тоже одна на
/// всех — заголовки разной высоты читаются как разные экраны.
pub const HEADING_HEIGHT: f32 = 52.0;

// --- Размеры текста ---

pub const TEXT_TITLE: f32 = 18.0;
pub const TEXT_BODY: f32 = 13.5;
pub const TEXT_LABEL: f32 = 12.5;
pub const TEXT_SMALL: f32 = 12.0;
/// Шапка таблицы: прописными и мельче остального.
pub const TEXT_HEADER: f32 = 11.0;
/// Моноширинное в строке списка — имя файла, размер.
pub const TEXT_MONO: f32 = 12.5;
/// Колонка формата: тише и мельче, это метка, а не значение.
pub const TEXT_TAG: f32 = 10.0;

// --- Сборка стиля ---

/// Вид виджета в одном состоянии.
fn face(background: Color, text: Color, border: Border) -> WidgetStyle {
    WidgetStyle {
        background: Some(Background::Color(background)),
        text_color: Some(text),
        border,
        shadow: None,
    }
}

fn outline(color: Color, radius: f32) -> Border {
    Border { color, width: 1.0, radius }
}

/// Общее начало всех нажимаемых ролей: коробка с видом в покое и под курсором.
///
/// Содержимое стоит по центру. Коробка кнопки почти всегда крупнее своей
/// подписи — от фиксированного размера или от чужой высоты в ряду, — и глиф,
/// прижатый к её углу, читается как ошибка вёрстки. Тому, чьё содержимое и так
/// тянется на всю коробку (строка списка, пункт меню), центрирование ничего не
/// меняет, так что случая назвать это иначе не возникает.
///
/// `pressed` и `disabled` не называются нигде: нажатие видно по тому, что оно
/// делает, а о недоступности говорит содержимое (бледная стрелка на краю
/// пагинации). Незваное вырождается к покою — см. `Interaction` в types.proto.
fn clickable<M>(content: impl Into<Element<M>>, rest: WidgetStyle, hovered: WidgetStyle) -> Container<M> {
    container(content).style(rest).hovered(hovered).center_x().center_y()
}

// --- Роли ---

/// Вкладка. Активная — цвета страницы, то есть продолжение показанного ниже;
/// остальные сливаются с хромом.
pub fn tab<M>(content: impl Into<Element<M>>, active: bool) -> Container<M> {
    let border = Border::with_radius(RADIUS);
    let (rest, hovered) = if active {
        (face(PAGE, INK, border), face(PAGE, INK, border))
    } else {
        (face(Color::TRANSPARENT, INK_DIM, border), face(HOVER, INK, border))
    };
    clickable(content, rest, hovered).padding(Padding { top: 4.0, bottom: 4.0, left: 10.0, right: 8.0 })
}

/// Кнопка-иконка на хроме: «+» в полосе вкладок, крестик вкладки.
pub fn chrome_icon<M>(content: impl Into<Element<M>>) -> Container<M> {
    clickable(
        content,
        face(Color::TRANSPARENT, INK_DIM, Border::with_radius(RADIUS_SMALL)),
        face(TRACK, INK, Border::with_radius(RADIUS_SMALL)),
    )
    .padding(4.0)
}

/// Кнопка-поверхность: чип фильтра, «вверх», страница пагинации, кнопка строки.
/// `raised` — открытый чип или выбранная страница: он держится нажатым.
pub fn surface_button<M>(content: impl Into<Element<M>>, raised: bool) -> Container<M> {
    let border = outline(if raised { LINE_STRONG } else { LINE }, RADIUS);
    let background = if raised { HOVER } else { SURFACE };
    clickable(content, face(background, INK_MUTED, border), face(HOVER, INK, border))
}

/// Полоса хрома: строка состояния, подпись под областью. Роль, а не сборка на
/// месте: полос таких две, и написанные порознь они разъезжаются зазором и
/// отступом — что и случилось, пока их было две копии.
pub fn chrome_bar<M>(content: impl Into<Element<M>>) -> Container<M> {
    container(content)
        .background(CHROME)
        .width(Length::Fill)
        .height(Length::Fixed(BAR_HEIGHT))
        .padding(Padding { top: 0.0, bottom: 0.0, left: GUTTER, right: GUTTER })
}

/// Распорка, отжимающая соседей к краям. Пишется одной ролью: `Fill` внутри
/// `Shrink` схлопывается в ноль (см. README про нехватку места), и обёртка,
/// которая этого не даёт, нужна каждому такому месту.
pub fn spacer<M>() -> Container<M> {
    container(space::<M>(Length::Fill, Length::Fixed(0.0))).width(Length::Fill)
}

/// Место, где показывать нечего, и короткая фраза почему. Одна роль на все
/// пустые состояния: они говорят об одном и том же и обязаны выглядеть
/// одинаково — иначе пустой список и пустая половина читаются как разные беды.
///
/// Сюда же уходят и отказы: причина — та же короткая фраза вместо содержимого.
/// А приезжает она откуда угодно, и пробелов в ней может не оказаться вовсе —
/// путь, ссылка, ключ. Поэтому перенос по буквам: умолчание рвёт строку только
/// по пробелам, и такая причина вышла бы за отведённое место (см. `Wrapping` в
/// types.proto).
pub fn empty<M>(what: &str) -> Container<M> {
    container(
        text::<M>(what.to_string())
            .size(TEXT_BODY)
            .color(INK_DIM)
            .wrapping(Wrapping::WrapWordOrGlyph),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .center_x()
    .center_y()
    .padding(Padding::new(20.0))
}

/// Значок строки списка. Размер здесь и назначается: строка одна и та же в
/// таблице и в списке слоёв, и разные размеры читались бы как разные вещи.
pub fn row_glyph<M>(glyph: &str, color: Color) -> Element<M> {
    icon::<M>(glyph).size(13.0).color(color).into()
}

/// Каким лицом стоит квадратная кнопка-значок.
///
/// Перечислением, а не набором флагов: лица говорят о разном. «Держится
/// нажатой» — про сам рычаг (раскрытое меню, включённый режим), «горит» — про
/// мир: то, о чём кнопка, уже сделано и это видно на шаре, «опасное» — про
/// последствие. Сложив первые два в один булев, мы получили бы значок, который
/// у лежащего на шаре снимка выглядит как раскрытое меню.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum IconTone {
    Rest,
    /// Держится нажатой: раскрытое меню, включённый режим.
    Raised,
    /// Необратимое — удаление. Тем же кирпичом, что и пункт меню «Удалить»
    /// (см. [`menu_item`]): одно и то же действие, названное разными цветами,
    /// читалось бы как два разных. В покое приглушённым ([`DANGER_TEXT`]) — по
    /// той же причине, по какой приглушена всякая подпись этим цветом, — и
    /// полным под курсором.
    Danger,
    /// Горит: снимок лежит на шаре и виден.
    Lit,
    /// Горит вполсилы: сделано, но увидеть этого сейчас нельзя — слой
    /// собирается или скрыт, контур ещё едет из каталога или не приедет вовсе.
    /// Та же зелень, что у [`IconTone::Lit`], слабее залитая: состояние одного
    /// рода, и говорить о нём другим цветом значило бы завести второй
    /// словарь.
    Half,
    /// Пробовали, и не вышло: значок нажимаем — попытка могла сорваться по
    /// сети, — но горит предупреждением, а причина стои́т подсказкой.
    ///
    /// Своим цветом, а не выключенным видом: выключенное говорит «нечего и
    /// пытаться», и сказанное так о том, что вполне может выйти со второго
    /// раза, отняло бы у человека действие. И не покоем: значок в покое зовёт
    /// нажать, ничего не обещая, — а здесь есть что сказать вперёд нажатия.
    Warn,
    /// Выключен: нажать нечего, а место за значком остаётся. Тем же знаком и
    /// той же коробкой, но цветом линии, а не чернил, — по той же причине, по
    /// какой отличается непроставляемая коробочка отметки (см. [`row_check`]):
    /// значок в полную силу обещает нажатие всюду, где стои́т.
    Idle,
}

/// Квадратная кнопка со значком в строке списка: главное действие, «на шар»,
/// меню. Роль, а не сборка на месте, — её собирают и таблица, и список слоёв, а
/// разойдясь, две одинаковые с виду кнопки читаются как две разные вещи.
///
/// Размер здесь и назначается: он свойство роли, а не того, кто её ставит.
pub fn row_button_icon<M>(glyph: &str, tone: IconTone) -> Container<M> {
    let lit = |fill: Color, ink: Color| {
        let border = outline(ACCENT_LINE, RADIUS);
        (face(fill, ink, border), face(ACCENT_WASH, ACCENT_TEXT, border))
    };
    let (rest, hovered) = match tone {
        IconTone::Danger => {
            let border = outline(LINE, RADIUS);
            (face(SURFACE, DANGER_TEXT, border), face(HOVER, DANGER, border))
        }
        IconTone::Rest | IconTone::Raised => {
            let raised = tone == IconTone::Raised;
            let border = outline(if raised { LINE_STRONG } else { LINE }, RADIUS);
            let background = if raised { HOVER } else { SURFACE };
            (face(background, INK_SOFT, border), face(HOVER, INK, border))
        }
        IconTone::Lit => lit(ACCENT_WASH, ACCENT_TEXT),
        IconTone::Half => lit(ACCENT_TINT, ACCENT_TEXT),
        IconTone::Warn => {
            let border = outline(WARN_LINE, RADIUS);
            (face(WARN_WASH, WARN_TEXT, border), face(WARN_WASH, WARN, border))
        }
        // Под курсором тем же: кнопка, которая отзывается на наведение и не
        // отзывается на нажатие, обещает больше, чем делает.
        IconTone::Idle => {
            let face = face(SURFACE, LINE_STRONG, outline(LINE_SOFT, RADIUS));
            (face.clone(), face)
        }
    };
    let ink = rest.text_color.unwrap_or(INK_SOFT);
    clickable(icon::<M>(glyph).size(11.0).color(ink), rest, hovered)
        .width(Length::Fixed(ROW_BUTTON))
        .height(Length::Fixed(ROW_BUTTON))
}

/// Коробочка пакетного выделения: в строке списка и в его шапке. Роль, а не
/// сборка на месте — стоит она в двух местах, и разъехавшись, читалась бы как
/// две разные вещи.
///
/// Занимает свою колонку целиком: та узкая (см. `table::CHECK`), и полей у неё
/// нет — иначе от коробочки остались бы несколько точек, по которым не попасть.
/// Коробочка отметки. `None` — отмечать нечем: рисуется тем же знаком, но
/// цветом линии, а не чернил. Разница нужна затем, что нажимается она не
/// везде, а коробочка в полную силу обещает нажатие всюду, где стои́т.
pub fn row_check<M>(marked: Option<bool>) -> Container<M> {
    let (glyph, color) = match marked {
        Some(true) => (glyph::BOX_TICKED, ACCENT),
        Some(false) => (glyph::BOX, INK_FAINT),
        None => (glyph::BOX, LINE_STRONG),
    };
    chrome_icon(icon::<M>(glyph).size(12.0).color(color))
        // Поля у́же обычных: колонка узкая, и четыре точки с каждой стороны
        // срезали бы сам знак.
        .padding(2.0)
        .width(Length::Fill)
        .height(Length::Fixed(ROW_BUTTON))
}

/// Кнопка с подписью в полосе хрома: под областью глобуса, в заголовке списка
/// слоёв. Тоже роль: полосы одного роста читаются как один язык, и кнопки в
/// них — тоже.
pub fn bar_button<M>(label: &str) -> Container<M> {
    surface_button(
        text::<M>(label.to_string()).size(TEXT_SMALL).single_line(),
        false,
    )
    .padding(Padding { top: 3.0, bottom: 3.0, left: 8.0, right: 8.0 })
}

/// Выбранная страница пагинации — единственная кнопка с акцентной заливкой.
pub fn page_button<M>(content: impl Into<Element<M>>, current: bool) -> Container<M> {
    let step = Padding { top: 0.0, bottom: 0.0, left: 7.0, right: 7.0 };
    if !current {
        return surface_button(content, false).padding(step);
    }
    let border = outline(ACCENT_LINE, RADIUS_SMALL);
    clickable(content, face(ACCENT_WASH, INK, border), face(ACCENT_WASH, INK, border)).padding(step)
}

/// Чем строка выделена среди соседей.
///
/// Перечислением, а не набором флагов, и вот почему: состояние коробки рендерер
/// выбирает целиком, а не подмешивает к покою (см. `Interaction` в
/// ui-service/types.proto), поэтому «отмечена и под курсором» протоколом не
/// выражается — свести их в один цвет обязан тот, кто цвет и называет. Раз
/// сводить всё равно здесь, то здесь же названо и старшинство: подсветка одна
/// на весь экран и старше отметки, которых в списке бывает полсотни.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum RowTint {
    #[default]
    Plain,
    /// На шаре её нет или её оттуда не видно: скрытый слой, снимок одним
    /// контуром. Не «выключена» — с ней всё то же самое делают, просто сейчас
    /// её не видно.
    Dim,
    /// Выбрана коробочкой — набрана для пакетного действия. Про шар заливка
    /// не говорит ничего: контур и показ видны значками строки.
    Marked,
    /// Главное сейчас: выбранный на шаре снимок или строка, к которой привёл
    /// переход (см. `state::Highlight`).
    Picked,
}

/// Пара «покой — под курсором» для заливки строки.
///
/// Наведение считается от покоя, а не берётся общим: общий ROW_HOVER светлее
/// акцентной заливки, и подсвеченная строка под курсором теряла бы подсветку —
/// снаружи это выглядит как «выделение слетает от наведения».
fn row_faces(tint: RowTint) -> (Color, Color) {
    match tint {
        RowTint::Plain => (Color::TRANSPARENT, ROW_HOVER),
        RowTint::Dim => (ROW_DIM, ROW_DIM_HOVER),
        RowTint::Marked => (ACCENT_TINT, ACCENT_TINT_HOVER),
        RowTint::Picked => (ACCENT_WASH, ACCENT_WASH_HOVER),
    }
}

/// Строка списка. Своего фона у обычной нет — она лежит на странице; видно её
/// только под курсором и по черте снизу.
pub fn row_button<M>(content: impl Into<Element<M>>, tint: RowTint) -> Container<M> {
    let (rest, hovered) = row_faces(tint);
    clickable(
        content,
        face(rest, INK_SOFT, Border::default()),
        face(hovered, INK_SOFT, Border::default()),
    )
    .padding(Padding { top: 0.0, bottom: 0.0, left: GUTTER, right: GUTTER })
}

/// Строка списка слоёв — та же заливка, но не кнопка: внутри строки лежит
/// ползунок прозрачности, и нажимаемая обёртка вокруг него отобрала бы у него
/// протаскивание.
pub fn layer_row<M>(content: impl Into<Element<M>>, tint: RowTint) -> Container<M> {
    container(content).background(row_faces(tint).0)
}

/// Пункт выпадающего меню. `danger` — удаление: цветом подписано только оно —
/// здесь и у пакетной кнопки (см. [`IconTone::Danger`]), — потому что только
/// оно необратимо.
pub fn menu_item<M>(content: impl Into<Element<M>>, selected: bool, danger: bool) -> Container<M> {
    let text = if danger { DANGER } else { INK };
    let background = if selected { HOVER } else { Color::TRANSPARENT };
    clickable(
        content,
        face(background, text, Border::with_radius(RADIUS_SMALL)),
        face(HOVER, text, Border::with_radius(RADIUS_SMALL)),
    )
    .padding(Padding { top: 6.0, bottom: 6.0, left: 9.0, right: 9.0 })
}

/// Сегмент пути. Нажимаемый, но не кнопка на вид: рамка и фон у каждого
/// сегмента превратили бы строку пути в ряд кнопок, а это одна строка.
pub fn crumb<M>(content: impl Into<Element<M>>) -> Container<M> {
    clickable(
        content,
        face(Color::TRANSPARENT, ACCENT, Border::with_radius(4.0)),
        face(HOVER, ACCENT, Border::with_radius(4.0)),
    )
    .padding(Padding { top: 2.0, bottom: 2.0, left: 3.0, right: 3.0 })
}

/// Коробка управляющего элемента: поле фильтра, строка пути. Та же
/// поверхность, что у кнопок рядом, — но нажимать её нечего, поэтому не кнопка.
pub fn control_box() -> WidgetStyle {
    face(SURFACE, INK_DIM, outline(LINE, RADIUS))
}

/// Всплывающая панель: меню, выпадающие списки и подсказки значков. Тень
/// отделяет её от того, над чем она висит, — без неё панель читается как часть
/// страницы. Подсказка здесь не по недосмотру: висит она так же и отделяться
/// обязана тем же, иначе два всплывающих над одним списком читались бы как
/// вещи разного рода.
pub fn panel() -> WidgetStyle {
    WidgetStyle {
        shadow: Some(Shadow { color: Color::rgb8(0x50, 0x42, 0x28).with_alpha(0.18), offset: (0.0, 8.0), blur: 24.0 }),
        ..face(SURFACE, INK, outline(LINE, 8.0))
    }
}

/// Поле ввода внутри коробки: рамку и фон даёт она, поэтому у самого поля их
/// нет — иначе рамка была бы нарисована дважды, одна в другой.
pub fn field() -> TextInputStyle {
    let bare = face(Color::TRANSPARENT, INK, Border::default());
    TextInputStyle {
        focused: bare.clone(),
        hovered: bare.clone(),
        active: bare,
        placeholder: INK_DIM,
        selection: ACCENT_WASH,
    }
}

pub fn scrollbar() -> Scrollbar {
    Scrollbar {
        width: 8.0,
        scroller_width: 6.0,
        margin: 2.0,
        rail: Some(Background::Color(Color::TRANSPARENT)),
        scroller: Some(Background::Color(LINE)),
        radius: 3.0,
    }
}

/// Полоса загрузки: цвет говорит то же, что подпись рядом, — идёт или прервано.
pub fn progress(color: Color) -> ProgressBarStyle {
    ProgressBarStyle {
        track: Some(Background::Color(TRACK)),
        bar: Some(Background::Color(color)),
        border: Border::with_radius(3.0),
    }
}

/// Ползунок: дорожка того же вида, что у полосы прогресса, — они стоят в одном
/// списке друг под другом, — и круглая ручка на ней. Отличается он не видом
/// дорожки, а тем, что за ручку тянут.
pub fn slider() -> SliderStyle {
    SliderStyle {
        filled: Some(Background::Color(ACCENT)),
        rest: Some(Background::Color(TRACK)),
        rail_width: 5.0,
        rail_border: Border::with_radius(3.0),
        handle: Some(Background::Color(SURFACE)),
        handle_radius: 6.0,
        handle_border_width: 1.5,
        handle_border_color: ACCENT,
    }
}

// --- Готовые мелочи ---

/// Пустое место нулевого размера — им занимают то, о чём сказать нечего:
/// ячейку без значения в сетке таблицы, пометку у пункта меню без неё. Место
/// остаётся за ним, поэтому соседи не съезжают.
pub fn nothing<M>() -> Element<M> {
    space::<M>(Length::Fixed(0.0), Length::Fixed(0.0)).into()
}

/// Волосяная линия во всю ширину: ею разделены полосы хрома и строки списка.
/// Отдельным виджетом, а не рамкой: рамка в этом протоколе одна на все четыре
/// стороны, а нужна только одна.
pub fn hairline<M>(color: Color) -> Element<M> {
    container(space::<M>(Length::Fill, Length::Fixed(HAIRLINE)))
        .background(color)
        .width(Length::Fill)
        .height(Length::Fixed(HAIRLINE))
        .into()
}

/// Граница между панелями: та же волосяная линия, но за неё берутся мышью.
///
/// Полоса захвата шире линии: попасть курсором в один пиксель нельзя, а
/// граница — единственное, чем панели меряют место. Подсвечивается она под
/// курсором — курсор в этом протоколе не меняется, и сказать «здесь можно
/// тянуть» больше нечем.
pub fn divider<M: veld_ui_service_wrap::UiMessage>(
    axis: crate::proto::ui_service::DividerAxis,
    on_drag: impl FnOnce(f32) -> M,
    on_release: M,
) -> veld_ui_service_wrap::Divider<M> {
    const GRIP: f32 = 7.0;
    veld_ui_service_wrap::divider(axis, on_drag)
        .on_release(on_release)
        .thickness(GRIP)
        .line(HAIRLINE)
        .color(LINE, ACCENT)
}

/// Та же линия поперёк — разделитель внутри строки.
pub fn vline<M>(color: Color) -> Element<M> {
    container(space::<M>(Length::Fixed(HAIRLINE), Length::Fill))
        .background(color)
        .width(Length::Fixed(HAIRLINE))
        .height(Length::Fill)
        .into()
}

/// Кружок состояния: цветом сказано то же, что подписью, — но видно раньше,
/// чем прочитано.
pub fn dot<M>(color: Color) -> Container<M> {
    const SIZE: f32 = 7.0;
    container(space::<M>(Length::Fixed(SIZE), Length::Fixed(SIZE)))
        .style(WidgetStyle {
            background: Some(Background::Color(color)),
            border: Border::with_radius(SIZE / 2.0),
            ..Default::default()
        })
        .width(Length::Fixed(SIZE))
        .height(Length::Fixed(SIZE))
}
