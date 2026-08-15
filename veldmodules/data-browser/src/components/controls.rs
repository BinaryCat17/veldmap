//! components/controls.rs — управление списком: отбор, путь и страницы.
//!
//! Всё, что стоит вокруг таблицы и меняет не данные, а показ. Список у трёх
//! видов один и тот же, поэтому и рычаги к нему одни — с оговоркой, что рычаг
//! показывается там, где он что-то меняет: путь есть только у каталога,
//! группировка бессмысленна ровно там, где путь есть.

use veld_ui_service_wrap::{column, row, Keyed};
use crate::proto::ui_service::{
    container, icon, mono, popover, space, text, text_input, Alignment, Element,
    FontWeight, Length, Padding,
};
use crate::module::components::{arrange::Arranged, format, menu};
use crate::module::state::listing::{Choice, ListingState, Menu};
use crate::module::state::ViewId;
use crate::module::{theme, Msg, ViewMsg};

/// Поля внутри коробки рычага — по обе стороны от содержимого.
const PAD: f32 = 11.0;
/// Зазор между рычагами полосы и между частями внутри рычага.
const GAP: f32 = 7.0;
/// Зазор между значком и текстом внутри поля — значок мельче подписи, и
/// прижатый тем же зазором читается как приклеенный.
const ICON_GAP: f32 = 8.0;
/// Кегль треугольника чипа. Он же его ширина с запасом: знак у́же своего кегля.
const CARET: f32 = 8.0;
/// Кегль лупы в поле — и так же её ширина с запасом.
const LENS: f32 = 11.0;
/// Ширина поля даты: дата — восемь знаков, и тянуться ей незачем.
const DATE_WIDTH: f32 = 150.0;

/// Рычаг полосы вместе с местом, которое ему нужно, чтобы подпись читалась.
///
/// Ширину знает только тот, кто собрал подпись, а раскладывает полосу [`bar`] —
/// поэтому рычаг приносит её с собой. Иначе полосе пришлось бы гадать по числу
/// рычагов, а чипы разной длины («Миссия: все» и «Группировка: по папкам»)
/// отличаются вдвое.
pub struct Control {
    element: Element<Msg>,
    width: f32,
}

/// Полоса отбора: поле фильтра и чипы.
///
/// `groupable` — есть ли что складывать по папкам. Рычаг, который ничего не
/// меняет, хуже отсутствующего: он обещает выбор и молчит в ответ. Знает это
/// вид — он один и знает, откуда собраны его строки (см. `list_screen`).
///
/// `width` — ширина половины, в которой стоит список: от неё зависит, ляжет
/// полоса в строку или развернётся (см. [`bar`]).
pub fn toolbar(
    view: ViewId,
    listing: &ListingState,
    opened: Option<&Menu>,
    counts: &[usize],
    groupable: bool,
    width: f32,
) -> Element<Msg> {
    let open = opened;
    let field = search_field("Фильтр по имени", &listing.query, move |query| {
        Msg::In(view, ViewMsg::Query(query))
    }, None);

    let mut controls = vec![
        chip(view, "Состояние:", listing.filter, Menu::Filter, open, counts, move |choice| {
            Msg::In(view, ViewMsg::Filter(choice))
        }),
    ];
    if groupable {
        controls.push(chip(view, "Группировка:", listing.grouping, Menu::Grouping, open, &[], move |choice| {
            Msg::In(view, ViewMsg::Group(choice))
        }));
    }
    controls.push(chip(view, "Сортировка:", listing.sorting, Menu::Sorting, open, &[], move |choice| {
        Msg::In(view, ViewMsg::Sort(choice))
    }));

    bar(width, field, controls)
}

/// Как полоса легла в отведённую ширину.
#[derive(PartialEq, Eq, Debug)]
enum Shape {
    /// Одной строкой: поле тянется, рычаги стоят за ним.
    Line,
    /// Поле забрало строку целиком, рычаги легли под ним рядами — по столько,
    /// сколько в ряд помещается.
    Stack(Vec<usize>),
}

/// Как разложить полосу в отведённой ширине.
///
/// Порог считается по полю, а не по рычагам: тянется в полосе одно оно, и
/// сжимается в узком месте тоже оно — до подсказки, обрезанной на середине
/// слова. Рычаги при этом стоят целыми, потому что ширина у них своя. Значит,
/// и мерить надо то, что теряется первым: перестало помещаться поле — полоса
/// разворачивается.
fn shape(width: f32, field: f32, controls: &[f32]) -> Shape {
    let free = width - theme::GUTTER * 2.0;
    if field + controls.iter().map(|control| GAP + control).sum::<f32>() <= free {
        return Shape::Line;
    }

    // Жадно, а не поровну: чипы разной длины, и ряд из трёх узких читается так
    // же, как ряд из двух широких. Ряд не бывает пустым — рычаг шире всей
    // полосы всё равно надо где-то показать, и это его собственный ряд.
    let mut rows: Vec<usize> = Vec::new();
    let mut taken = 0.0;
    for control in controls {
        match rows.last_mut() {
            Some(count) if taken + GAP + control <= free => {
                *count += 1;
                taken += GAP + control;
            }
            _ => {
                rows.push(1);
                taken = *control;
            }
        }
    }
    Shape::Stack(rows)
}

/// Полоса рычагов над таблицей: поле и чипы при нём. Роль, а не сборка на
/// месте — таких полос две (отбор списка и запрос к каталогу), и написанные
/// порознь они разъезжаются зазором, отступом снизу и порогом разворота.
///
/// Поле идёт отдельным доводом, а не первым в списке: оно единственное
/// тянущееся, и в узкой половине именно ему достаётся строка целиком.
pub fn bar(width: f32, field: Control, controls: Vec<Control>) -> Element<Msg> {
    let padding = Padding { top: 0.0, bottom: 10.0, left: theme::GUTTER, right: theme::GUTTER };
    let line = |controls: Vec<Element<Msg>>| {
        row(controls).spacing(GAP).width(Length::Fill).align_items(Alignment::Center)
    };

    let widths: Vec<f32> = controls.iter().map(|control| control.width).collect();
    let rows = match shape(width, field.width, &widths) {
        Shape::Line => {
            let mut all = vec![field.element];
            all.extend(controls.into_iter().map(|control| control.element));
            return line(all).padding(padding).into();
        }
        Shape::Stack(rows) => rows,
    };

    let mut rest = controls.into_iter();
    let mut lines: Vec<Element<Msg>> = vec![line(vec![field.element]).into()];
    for count in rows {
        let taken = rest.by_ref().take(count).map(|control| control.element).collect();
        lines.push(line(taken).into());
    }

    column(lines).spacing(GAP).width(Length::Fill).padding(padding).into()
}

/// Поле с лупой: ввод в одной коробке со значком. Лупа внутри неё, а не
/// рядом, — иначе она читается как отдельная кнопка. Одна сборка на оба поля
/// приложения: фильтр списка и запрос к каталогу различаются только подписью,
/// сообщением ввода и тем, ждёт ли поле Enter (`on_submit`).
pub fn search_field(
    placeholder: &str,
    value: &str,
    on_input: impl Fn(String) -> Msg + 'static,
    on_submit: Option<Msg>,
) -> Control {
    let mut input = text_input::<Msg>(placeholder, value)
        .style(theme::field())
        .font_family(veld_ui_service_wrap::style::FONT_UI)
        .size(theme::TEXT_BODY)
        .padding(Padding::ZERO)
        .width(Length::Fill)
        .on_input(on_input);
    if let Some(message) = on_submit {
        input = input.on_submit(message);
    }

    let element = container(
        row![
            icon::<Msg>(theme::glyph::SEARCH).size(LENS).color(theme::INK_FAINT),
            input,
        ]
        .spacing(ICON_GAP)
        .width(Length::Fill)
        .align_items(Alignment::Center),
    )
    .style(theme::control_box())
    .width(Length::Fill)
    .height(Length::Fixed(theme::CONTROL_HEIGHT))
    .align_y(Alignment::Center)
    .padding(Padding { top: 0.0, bottom: 0.0, left: PAD, right: PAD });

    // Поле — единственное тянущееся в полосе, и просит оно ровно столько,
    // сколько нужно его подсказке: подсказка, обрезанная на середине слова,
    // ничего не подсказывает, и полоса разворачивается раньше (см. [`bar`]).
    Control {
        element: element.into(),
        width: PAD * 2.0 + LENS + ICON_GAP + format::text_width(placeholder, theme::TEXT_BODY),
    }
}

/// Чип со значением и выпадающим списком. Счётчики показывает только тот, кому
/// их дали: у порядка и группировки считать нечего.
///
/// `opened` — меню, раскрытое в этом виде: их много, а раскрыто одно, и знать
/// чипу нужно только это.
pub fn chip<C: Choice>(
    view: ViewId,
    caption: &str,
    current: C,
    menu: Menu,
    opened: Option<&Menu>,
    counts: &[usize],
    make: impl Fn(C) -> Msg,
) -> Control {
    let open = opened == Some(&menu);
    let anchor = theme::surface_button(
        row![
            text::<Msg>(caption.to_string()).size(theme::TEXT_LABEL).color(theme::INK_DIM).single_line(),
            text::<Msg>(current.label().to_string())
                .size(theme::TEXT_LABEL)
                .color(theme::INK)
                .weight(FontWeight::WeightMedium)
                .single_line(),
            icon::<Msg>(theme::glyph::CARET).size(CARET).color(theme::INK_FAINT),
        ]
        .spacing(GAP)
        .align_items(Alignment::Center),
        open,
    )
    .height(Length::Fixed(theme::CONTROL_HEIGHT))
    .padding(Padding { top: 0.0, bottom: 0.0, left: PAD, right: PAD })
    .on_press(Msg::In(view, ViewMsg::OpenMenu(Some(menu))));

    // Ширина — по тому, что на чипе стои́т сейчас, а не по самой длинной из
    // подписей меню: выбранное значение и есть его содержимое. Полоса от смены
    // значения перекладывается, и это честно — чип действительно стал шире.
    let width = PAD * 2.0
        + GAP * 2.0
        + CARET
        + format::text_width(caption, theme::TEXT_LABEL)
        + format::text_width(current.label(), theme::TEXT_LABEL);

    // Пункты собираются вместе с панелью, то есть только у раскрытого чипа:
    // их столько же, сколько значений у перечисления, и на закрытом они не
    // видны ни секунды.
    let items = || {
        C::ALL
            .iter()
            .enumerate()
            .map(|(index, value)| {
                let item = menu::Item::new(value.title(), make(*value)).selected(*value == current);
                match counts.get(index) {
                    Some(count) => item.count(*count),
                    None => item,
                }
            })
            .collect()
    };

    let element = popover(anchor, open, || menu::panel(items()))
        .align_x(Alignment::End)
        .gap(3.0)
        .on_dismiss(Msg::In(view, ViewMsg::OpenMenu(None)));

    Control { element: element.into(), width }
}

/// Путь текущей папки: «вверх» и сегменты, по которым можно вернуться.
/// Показывается там, где путь есть, — то есть в сетевом каталоге.
pub fn path(view: ViewId, current: &str) -> Element<Msg> {
    // Корень назван как папка, а не пустым местом: он такой же шаг пути, и
    // вернуться в него — обычный переход, а не особый случай.
    let mut crumbs: Vec<Element<Msg>> = vec![
        theme::crumb(
            mono::<Msg>("корень")
                .size(theme::TEXT_SMALL)
                .color(if current.is_empty() { theme::INK } else { theme::ACCENT }),
        )
        .on_press(Msg::In(view, ViewMsg::Enter(String::new())))
        .key("root")
        .into(),
    ];
    let mut prefix = String::new();
    let segments: Vec<&str> = current.split('/').filter(|part| !part.is_empty()).collect();

    for (index, segment) in segments.iter().enumerate() {
        crumbs.push(mono::<Msg>("/").size(theme::TEXT_SMALL).color(theme::INK_FAINT).into());
        prefix.push_str(segment);
        prefix.push('/');
        let last = index + 1 == segments.len();
        crumbs.push(
            theme::crumb(
                mono::<Msg>((*segment).to_string())
                    .size(theme::TEXT_SMALL)
                    .color(if last { theme::INK } else { theme::ACCENT }),
            )
            .on_press(Msg::In(view, ViewMsg::Enter(prefix.clone())))
            .key(prefix.clone()),
        );
    }

    row![
        theme::surface_button(icon::<Msg>(theme::glyph::UP).size(11.0).color(theme::INK_MUTED), false)
            .width(Length::Fixed(theme::CONTROL_HEIGHT))
            .height(Length::Fixed(theme::CONTROL_HEIGHT))
            .on_press(Msg::In(view, ViewMsg::Up)),
        container(row(crumbs).spacing(4.0).align_items(Alignment::Center))
            .style(theme::control_box())
            .width(Length::Fill)
            .height(Length::Fixed(theme::CONTROL_HEIGHT))
            .align_y(Alignment::Center)
            .padding(Padding { top: 0.0, bottom: 0.0, left: 11.0, right: 11.0 }),
    ]
    .spacing(7.0)
    .width(Length::Fill)
    .align_items(Alignment::Center)
    .padding(Padding { top: 0.0, bottom: 9.0, left: theme::GUTTER, right: theme::GUTTER })
    .into()
}

/// Кнопка страницы: номера растягиваются по своей подписи, стрелки — нет.
const STEP_HEIGHT: f32 = 24.0;
const STEP_WIDTH: f32 = 26.0;

/// Подвал со страницами. Длинный каталог режется на страницы, и прокрутка
/// остаётся внутри страницы — поэтому место, на котором стоял пользователь,
/// не теряется при переходе.
pub fn pager(view: ViewId, arranged: &Arranged<'_>) -> Element<Msg> {
    let step = |glyph: &str, to: usize, enabled: bool| {
        let step = theme::surface_button(
            icon::<Msg>(glyph).size(9.0).color(if enabled { theme::INK_SOFT } else { theme::LINE_STRONG }),
            false,
        )
        .width(Length::Fixed(STEP_WIDTH))
        .height(Length::Fixed(STEP_HEIGHT));
        if enabled { step.on_press(Msg::In(view, ViewMsg::Page(to))) } else { step }
    };

    let pages = (0..arranged.pages).map(|index| {
        theme::page_button(
            text::<Msg>((index + 1).to_string())
                .size(theme::TEXT_LABEL)
                .color(theme::INK_MUTED)
                .single_line(),
            index == arranged.page,
        )
        .height(Length::Fixed(STEP_HEIGHT))
        .on_press(Msg::In(view, ViewMsg::Page(index)))
        .into()
    });

    let bar = row![
        text::<Msg>(arranged.range()).size(theme::TEXT_LABEL).color(theme::INK_DIM).single_line(),
        // Распорка: подпись слева, страницы справа.
        space::<Msg>(Length::Fill, Length::Fixed(0.0)),
    ]
    .push(step(theme::glyph::LEFT, arranged.page.saturating_sub(1), arranged.page > 0))
    .extend(pages)
    .push(step(theme::glyph::RIGHT, arranged.page + 1, arranged.page + 1 < arranged.pages))
    .spacing(8.0)
    .width(Length::Fill)
    .align_items(Alignment::Center)
    .padding(Padding { top: 7.0, bottom: 7.0, left: theme::GUTTER, right: theme::GUTTER });

    container(bar).width(Length::Fill).background(theme::SHELF).into()
}

/// Поле даты в полосе фильтров: подпись края и набранное рядом.
///
/// Ширина фиксированная, а не тянущаяся: дата — восемь знаков, и растягивать
/// под неё половину полосы значило бы отнять место у поля запроса, которое как
/// раз тянется. Набранное сверх места срезает коробка — она и есть граница
/// показа (см. `Container` в ui-service/types.proto).
pub fn date_field(
    caption: &str,
    value: &str,
    on_input: impl Fn(String) -> Msg + 'static,
    on_submit: Msg,
) -> Control {
    let input = text_input::<Msg>("ГГГГ-ММ-ДД", value)
        .style(theme::field())
        .font_family(veld_ui_service_wrap::style::FONT_MONO)
        .size(theme::TEXT_LABEL)
        .padding(Padding::ZERO)
        .width(Length::Fill)
        .on_input(on_input)
        .on_submit(on_submit);

    let element = container(
        row![
            text::<Msg>(caption.to_string())
                .size(theme::TEXT_LABEL)
                .color(theme::INK_DIM)
                .single_line(),
            input,
        ]
        .spacing(ICON_GAP)
        .width(Length::Fill)
        .align_items(Alignment::Center),
    )
    .style(theme::control_box())
    .width(Length::Fixed(DATE_WIDTH))
    .height(Length::Fixed(theme::CONTROL_HEIGHT))
    .align_y(Alignment::Center)
    .padding(Padding { top: 0.0, bottom: 0.0, left: PAD, right: PAD });

    Control { element: element.into(), width: DATE_WIDTH }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Поле запроса, три чипа — обычная полоса каталога.
    const FIELD: f32 = 280.0;
    const CHIPS: [f32; 3] = [140.0, 150.0, 160.0];

    /// Пока поле помещается вместе с рычагами, полоса остаётся строкой: она и
    /// читается лучше, и не отнимает у списка высоту.
    #[test]
    fn широкая_половина_держит_полосу_строкой() {
        assert_eq!(shape(900.0, FIELD, &CHIPS), Shape::Line);
    }

    /// Места не хватило — строку забирает поле, а рычаги ложатся под ним
    /// столько в ряд, сколько влезло. Ни один при этом не теряется.
    #[test]
    fn узкая_половина_разворачивает_полосу() {
        assert_eq!(shape(700.0, FIELD, &CHIPS), Shape::Stack(vec![3]));
        assert_eq!(shape(400.0, FIELD, &CHIPS), Shape::Stack(vec![2, 1]));

        for width in [120.0, 400.0, 700.0, 780.0] {
            let Shape::Stack(rows) = shape(width, FIELD, &CHIPS) else { continue };
            assert_eq!(rows.iter().sum::<usize>(), CHIPS.len(), "ширина {}", width);
            assert!(rows.iter().all(|count| *count > 0), "пустой ряд при ширине {}", width);
        }
    }

    /// Рычаг шире всей полосы всё равно надо где-то показать — и он встаёт в
    /// свой ряд один, а не выпадает из неё.
    #[test]
    fn рычаг_шире_полосы_встаёт_один() {
        assert_eq!(shape(150.0, FIELD, &CHIPS), Shape::Stack(vec![1, 1, 1]));
    }
}
