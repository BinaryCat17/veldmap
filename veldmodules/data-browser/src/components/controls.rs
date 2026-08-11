//! components/controls.rs — управление списком: отбор, путь и страницы.
//!
//! Всё, что стоит вокруг таблицы и меняет не данные, а показ. Одинаково у трёх
//! видов: список один и тот же, значит и рычаги к нему те же.

use veld_ui_service_wrap::{row, Keyed};
use crate::proto::ui_service::{
    button, container, icon, mono, popover, space, text, text_input, Alignment, Element,
    FontWeight, Length, Padding,
};
use crate::module::components::{arrange::Arranged, menu};
use crate::module::state::listing::{Choice, ListingState, Menu};
use crate::module::{theme, Msg};

const GLYPH_SEARCH: &str = "\u{f002}";
const GLYPH_CARET: &str = "\u{f0d7}";
const GLYPH_UP: &str = "\u{f062}";
const GLYPH_LEFT: &str = "\u{f053}";
const GLYPH_RIGHT: &str = "\u{f054}";

/// Полоса отбора: поле фильтра и три чипа.
pub fn toolbar(listing: &ListingState, counts: &[usize]) -> Element<Msg> {
    row![
        field(listing),
        chip("Состояние:", listing.filter, Menu::Filter, listing, counts, Msg::Filter),
        chip("Группировка:", listing.grouping, Menu::Grouping, listing, &[], Msg::Group),
        chip("Сортировка:", listing.sorting, Menu::Sorting, listing, &[], Msg::Sort),
    ]
    .spacing(7.0)
    .width(Length::Fill)
    .align_items(Alignment::Center)
    .padding(Padding { top: 0.0, bottom: 10.0, left: theme::GUTTER, right: theme::GUTTER })
    .into()
}

/// Поле фильтра: лупа и ввод в одной коробке. Лупа внутри неё, а не рядом, —
/// иначе она читается как отдельная кнопка.
fn field(listing: &ListingState) -> Element<Msg> {
    container(
        row![
            icon::<Msg>(GLYPH_SEARCH).size(11.0).color(theme::INK_FAINT),
            text_input::<Msg>("Фильтр по имени", &listing.query)
                .style(theme::field())
                .font_family(veld_ui_service_wrap::style::FONT_UI)
                .size(theme::TEXT_BODY)
                .padding(Padding::ZERO)
                .width(Length::Fill)
                .on_input(Msg::Query),
        ]
        .spacing(8.0)
        .width(Length::Fill)
        .align_items(Alignment::Center),
    )
    .style(theme::control_box())
    .width(Length::Fill)
    .height(Length::Fixed(theme::CONTROL_HEIGHT))
    .align_y(Alignment::Center)
    .padding(Padding { top: 0.0, bottom: 0.0, left: 11.0, right: 11.0 })
    .into()
}

/// Чип со значением и выпадающим списком. Счётчики показывает только тот, кому
/// их дали: у порядка и группировки считать нечего.
fn chip<C: Choice>(
    caption: &str,
    current: C,
    menu: Menu,
    listing: &ListingState,
    counts: &[usize],
    make: fn(C) -> Msg,
) -> Element<Msg> {
    let open = listing.menu == menu;
    let anchor = theme::surface_button(
        button(
            row![
                text::<Msg>(caption.to_string()).size(theme::TEXT_LABEL).color(theme::INK_DIM).single_line(),
                text::<Msg>(current.label().to_string())
                    .size(theme::TEXT_LABEL)
                    .color(theme::INK)
                    .weight(FontWeight::WeightMedium)
                    .single_line(),
                icon::<Msg>(GLYPH_CARET).size(8.0).color(theme::INK_FAINT),
            ]
            .spacing(7.0)
            .align_items(Alignment::Center),
        ),
        open,
    )
    .height(Length::Fixed(theme::CONTROL_HEIGHT))
    .padding(Padding { top: 0.0, bottom: 0.0, left: 11.0, right: 11.0 })
    .on_press(Msg::OpenMenu(menu));

    let items = C::ALL
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let item = menu::Item::new(value.title(), make(*value)).selected(*value == current);
            match counts.get(index) {
                Some(count) => item.count(*count),
                None => item,
            }
        })
        .collect();

    popover(anchor, menu::panel(items))
        .open(open)
        .align_x(Alignment::End)
        .gap(3.0)
        .on_dismiss(Msg::OpenMenu(Menu::Closed))
        .into()
}

/// Путь текущей папки: «вверх» и сегменты, по которым можно вернуться.
/// Показывается там, где путь есть, — то есть в сетевом каталоге.
pub fn path(current: &str) -> Element<Msg> {
    // Корень назван как папка, а не пустым местом: он такой же шаг пути, и
    // вернуться в него — обычный переход, а не особый случай.
    let mut crumbs: Vec<Element<Msg>> = vec![
        theme::crumb(button(
            mono::<Msg>("корень")
                .size(theme::TEXT_SMALL)
                .color(if current.is_empty() { theme::INK } else { theme::ACCENT }),
        ))
        .on_press(Msg::Enter(String::new()))
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
            theme::crumb(button(
                mono::<Msg>((*segment).to_string())
                    .size(theme::TEXT_SMALL)
                    .color(if last { theme::INK } else { theme::ACCENT }),
            ))
            .on_press(Msg::Enter(prefix.clone()))
            .key(prefix.clone()),
        );
    }

    row![
        theme::surface_button(button(icon::<Msg>(GLYPH_UP).size(11.0).color(theme::INK_MUTED)), false)
            .width(Length::Fixed(theme::CONTROL_HEIGHT))
            .height(Length::Fixed(theme::CONTROL_HEIGHT))
            .on_press(Msg::Up),
        container(row(crumbs).spacing(4.0).align_items(Alignment::Center))
            .style(theme::control_box())
            .width(Length::Fill)
            .height(Length::Fixed(theme::CONTROL_HEIGHT))
            .align_y(Alignment::Center)
            .padding(Padding { top: 0.0, bottom: 0.0, left: 11.0, right: 11.0 })
            .clip(),
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
pub fn pager(arranged: &Arranged<'_>) -> Element<Msg> {
    let step = |glyph: &str, to: usize, enabled: bool| {
        let button = theme::surface_button(
            button(icon::<Msg>(glyph).size(9.0).color(if enabled { theme::INK_SOFT } else { theme::LINE_STRONG })),
            false,
        )
        .width(Length::Fixed(STEP_WIDTH))
        .height(Length::Fixed(STEP_HEIGHT));
        if enabled { button.on_press(Msg::Page(to)) } else { button }
    };

    let pages = (0..arranged.pages).map(|index| {
        theme::page_button(
            button(
                text::<Msg>((index + 1).to_string())
                    .size(theme::TEXT_LABEL)
                    .color(theme::INK_MUTED)
                    .single_line(),
            ),
            index == arranged.page,
        )
        .height(Length::Fixed(STEP_HEIGHT))
        .on_press(Msg::Page(index))
        .into()
    });

    let bar = row![
        text::<Msg>(arranged.range()).size(theme::TEXT_LABEL).color(theme::INK_DIM).single_line(),
        // Распорка: подпись слева, страницы справа.
        space::<Msg>(Length::Fill, Length::Fixed(0.0)),
    ]
    .push(step(GLYPH_LEFT, arranged.page.saturating_sub(1), arranged.page > 0))
    .extend(pages)
    .push(step(GLYPH_RIGHT, arranged.page + 1, arranged.page + 1 < arranged.pages))
    .spacing(8.0)
    .width(Length::Fill)
    .align_items(Alignment::Center)
    .padding(Padding { top: 7.0, bottom: 7.0, left: theme::GUTTER, right: theme::GUTTER });

    container(bar).width(Length::Fill).background(theme::SHELF).into()
}
