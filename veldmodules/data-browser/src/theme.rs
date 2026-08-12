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
    TextInputStyle, WidgetStyle, Container, Element, Length, container, space,
};

// --- Глифы ---
//
// Словарь иконок (шрифт Icons) — один на приложение: вкладка, меню и таблица
// подписывают одну сущность одним знаком, и сменившийся глиф не должен
// разводить их между собой.
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
    pub const MORE: &str = "\u{f141}";
    pub const CARET: &str = "\u{f0d7}";
    pub const UP: &str = "\u{f062}";
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
/// Заливка выбранного: активная страница пагинации, выбранный пункт меню.
const ACCENT_WASH: Color = Color::rgb8(0xDC, 0xE7, 0xD2);
const ACCENT_LINE: Color = Color::rgb8(0xC3, 0xD6, 0xB4);

pub const WARN: Color = Color::rgb8(0xC4, 0x79, 0x1F);
pub const WARN_TEXT: Color = Color::rgb8(0x8A, 0x62, 0x15);
pub const DANGER: Color = Color::rgb8(0xB4, 0x50, 0x3C);
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
/// Высота управляющего элемента в тулбаре: поле фильтра и чипы.
pub const CONTROL_HEIGHT: f32 = 31.0;
/// Сторона квадратной кнопки в строке списка.
pub const ROW_BUTTON: f32 = 23.0;
/// Отступ содержимого от краёв экрана.
pub const GUTTER: f32 = 16.0;
/// Высота однострочной полосы хрома: строка состояния, подпись под областью.
/// Одна на всех — полосы одного роста читаются как один язык.
pub const BAR_HEIGHT: f32 = 30.0;

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
        face(Color::rgb8(0xE7, 0xDF, 0xCB), INK, Border::with_radius(RADIUS_SMALL)),
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

/// Выбранная страница пагинации — единственная кнопка с акцентной заливкой.
pub fn page_button<M>(content: impl Into<Element<M>>, current: bool) -> Container<M> {
    let step = Padding { top: 0.0, bottom: 0.0, left: 7.0, right: 7.0 };
    if !current {
        return surface_button(content, false).padding(step);
    }
    let border = outline(ACCENT_LINE, RADIUS_SMALL);
    clickable(content, face(ACCENT_WASH, INK, border), face(ACCENT_WASH, INK, border)).padding(step)
}

/// Строка списка. Своего фона у неё нет — она лежит на странице; видно её
/// только под курсором и по черте снизу.
pub fn row_button<M>(content: impl Into<Element<M>>) -> Container<M> {
    clickable(
        content,
        face(Color::TRANSPARENT, INK_SOFT, Border::default()),
        face(ROW_HOVER, INK_SOFT, Border::default()),
    )
    .padding(Padding { top: 0.0, bottom: 0.0, left: GUTTER, right: GUTTER })
}

/// Пункт выпадающего меню. `danger` — удаление: единственное, что подписано
/// цветом, потому что единственное необратимое.
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

/// Всплывающая панель: меню и выпадающие списки. Тень отделяет её от того, над
/// чем она висит, — без неё панель читается как часть страницы.
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
    container(space::<M>(Length::Fill, Length::Fixed(1.0)))
        .background(color)
        .width(Length::Fill)
        .height(Length::Fixed(1.0))
        .into()
}

/// Та же линия поперёк — разделитель внутри строки.
pub fn vline<M>(color: Color) -> Element<M> {
    container(space::<M>(Length::Fixed(1.0), Length::Fill))
        .background(color)
        .width(Length::Fixed(1.0))
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
