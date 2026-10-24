//! components/controls.rs — list controls: selection, path, and pages.
//!
//! Everything around the table that changes not the data but the display. The
//! listing is the same for all three views, so the controls are shared too —
//! with the rule that a control appears where it changes something: the path
//! exists only in the catalogue, and grouping is meaningless exactly where the
//! path exists.

use veld_ui_service_wrap::{column, row, Keyed};
use crate::proto::ui_service::{
    container, icon, mono, popover, space, text, text_input, Alignment, Element,
    FontWeight, Length, Padding,
};
use crate::module::components::{arrange::Arranged, format, menu};
use crate::module::state::listing::{Choice, ListingState, Menu};
use crate::module::state::ViewId;
use crate::module::{theme, Msg, ViewMsg};

/// Padding inside the control box — on both sides of the content.
const PAD: f32 = 11.0;
/// Gap between the bar controls and between the parts inside a control.
const GAP: f32 = 7.0;
/// Gap between the icon and the text inside a field — the icon is smaller than
/// the caption and, pressed by the same gap, reads as glued on.
const ICON_GAP: f32 = 8.0;
/// Size of the chip caret. Also its width with slack: the glyph is narrower
/// than its size.
const CARET: f32 = 8.0;
/// Size of the magnifier in the field — and likewise its width with slack.
const LENS: f32 = 11.0;
/// Width of the date field: a date is eight characters and needs no stretching.
const DATE_WIDTH: f32 = 150.0;

/// A bar control together with the space it needs for its caption to read.
///
/// Only the one who built the caption knows the width, while the bar is laid
/// out by [`bar`] — so the control carries the width along. Otherwise the bar
/// would have to guess from the number of controls, while chips of different
/// length ("Mission: all" and "Grouping: by folders") differ twofold.
pub struct Control {
    element: Element<Msg>,
    width: f32,
}

/// Selection bar: the filter field and the chips.
///
/// `groupable` — whether there is anything to group by folders. A control that
/// changes nothing is worse than a missing one: it promises a choice and
/// answers with silence. The view knows this — it alone knows where its rows
/// come from (see `list_screen`).
///
/// `width` — the width of the half the list sits in: it decides whether the
/// bar fits on one line or unfolds (see [`bar`]).
pub fn toolbar(
    view: ViewId,
    listing: &ListingState,
    opened: Option<&Menu>,
    counts: &[usize],
    groupable: bool,
    width: f32,
) -> Element<Msg> {
    let open = opened;
    let field = search_field("Filter by name", &listing.query, move |query| {
        Msg::In(view, ViewMsg::Query(query))
    }, None);

    let mut controls = vec![
        chip(view, "State:", listing.filter, Menu::Filter, open, counts, move |choice| {
            Msg::In(view, ViewMsg::Filter(choice))
        }),
    ];
    if groupable {
        controls.push(chip(view, "Grouping:", listing.grouping, Menu::Grouping, open, &[], move |choice| {
            Msg::In(view, ViewMsg::Group(choice))
        }));
    }
    controls.push(chip(view, "Sorting:", listing.sorting, Menu::Sorting, open, &[], move |choice| {
        Msg::In(view, ViewMsg::Sort(choice))
    }));

    bar(width, field, controls)
}

/// How the bar fits the allotted width.
#[derive(PartialEq, Eq, Debug)]
enum Shape {
    /// On one line: the field stretches, the controls stand behind it.
    Line,
    /// The field took the whole line, the controls fell under it in rows — as
    /// many per row as fit.
    Stack(Vec<usize>),
}

/// How to lay the bar out in the allotted width.
///
/// The threshold is measured by the field, not the controls: the field is the
/// only stretching part of the bar, and it is the one that shrinks in a tight
/// spot — down to a hint cut mid-word. The controls stay whole, because they
/// carry their own width. So what must be measured is what is lost first: once
/// the field stops fitting, the bar unfolds.
fn shape(width: f32, field: f32, controls: &[f32]) -> Shape {
    let free = width - theme::GUTTER * 2.0;
    if field + controls.iter().map(|control| GAP + control).sum::<f32>() <= free {
        return Shape::Line;
    }

    // Greedy, not even: chips differ in length, and a row of three narrow ones
    // reads the same as a row of two wide ones. A row is never empty — a
    // control wider than the whole bar still has to be shown somewhere, and it
    // gets its own row.
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

/// The control bar above the table: the field and its chips. A role, not an
/// on-the-spot assembly — there are two such bars (list selection and catalogue
/// query), and written separately they would drift apart in gap, bottom
/// padding, and unfolding threshold.
///
/// The field goes as a separate argument, not first in the list: it is the
/// only stretching part, and in a narrow half it is the one that gets the whole
/// line.
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

/// Field with a magnifier: the input in one box with the icon. The magnifier
/// sits inside it, not beside — otherwise it reads as a separate button. One
/// assembly for both fields of the application: the list filter and the
/// catalogue query differ only in the caption, the input message, and whether
/// the field waits for Enter (`on_submit`).
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

    // The field is the only stretching part of the bar, and it asks for exactly
    // as much as its hint needs: a hint cut mid-word hints at nothing, and the
    // bar unfolds earlier (see [`bar`]).
    Control {
        element: element.into(),
        width: PAD * 2.0 + LENS + ICON_GAP + format::text_width(placeholder, theme::TEXT_BODY),
    }
}

/// Chip with a value and a dropdown list. Only those given counters show them:
/// sorting and grouping have nothing to count.
///
/// `opened` — the menu opened in this view: there are many, only one is open,
/// and this is all the chip needs to know.
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

    // The width follows what the chip shows right now, not the longest of the
    // menu captions: the selected value is its content. The bar reflows when
    // the value changes, and honestly so — the chip really did get wider.
    let width = PAD * 2.0
        + GAP * 2.0
        + CARET
        + format::text_width(caption, theme::TEXT_LABEL)
        + format::text_width(current.label(), theme::TEXT_LABEL);

    // Items are built together with the panel, i.e. only for the opened chip:
    // there are as many of them as the enum has values, and on a closed chip
    // they are never visible for a second.
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

/// Path of the current folder: "up" and the segments to return by. Shown where
/// a path exists — that is, in the network catalogue.
pub fn path(view: ViewId, current: &str) -> Element<Msg> {
    // The root is named as a folder, not as empty space: it is an equal step of
    // the path, and returning to it is an ordinary transition, not a special
    // case.
    let mut crumbs: Vec<Element<Msg>> = vec![
        theme::crumb(
            mono::<Msg>("root")
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

/// Page button: numbers stretch to their caption, arrows do not.
const STEP_HEIGHT: f32 = 24.0;
const STEP_WIDTH: f32 = 26.0;
/// Gap between the footer parts.
const STEP_GAP: f32 = 8.0;
/// Page button padding on both sides of the number (`theme::page_button`).
const STEP_PAD: f32 = 14.0;

/// How many page numbers fit in the footer of the allotted width.
///
/// Three things in it are fixed — the range label and both arrows; the rest is
/// given to the numbers, and they are measured by the widest one: numbers of
/// different length stand in a row, and one measured by the short would not fit
/// when three-digit numbers come.
fn room(width: f32, pages: usize, range: &str) -> usize {
    let fixed = theme::GUTTER * 2.0
        + format::text_width(range, theme::TEXT_LABEL)
        + (STEP_WIDTH + STEP_GAP) * 2.0;
    let slot =
        format::text_width(&pages.to_string(), theme::TEXT_LABEL) + STEP_PAD + STEP_GAP;
    (((width - fixed) / slot) as i32).max(1) as usize
}

/// Page numbers in the footer: a window around the current one, with the edges
/// always named. `None` — a gap, in place of which stands an ellipsis.
///
/// A window, not the whole list: a catalogue may have around fifty numbers,
/// they do not fit in a row, and the box cuts by itself — and what is cut off
/// is the tail of the row, that is the "next" arrow. A list with no way to
/// leave its first page looks shorter than it is. The edges are named because
/// otherwise the last page is only reachable one by one.
fn numbers(page: usize, pages: usize, room: usize) -> Vec<Option<usize>> {
    // With fewer than five slots the window cannot show itself: first, gap,
    // current, gap, last — that is its smallest form. Tight either way, but
    // with the window reaching the edges at least one can get there.
    let room = room.max(5);
    if pages <= room {
        return (0..pages).map(Some).collect();
    }
    // The edges and both gaps are occupied; the moving window gets the rest.
    let span = room - 4;
    let first = page.saturating_sub(span / 2).clamp(1, pages - 1 - span);
    let last = first + span - 1;

    let mut slots = vec![Some(0)];
    if first > 1 {
        slots.push(None);
    }
    slots.extend((first..=last).map(Some));
    if last < pages - 2 {
        slots.push(None);
    }
    slots.push(Some(pages - 1));
    slots
}

/// Footer with pages. A long catalogue is cut into pages, and scrolling stays
/// within the page — so the place the user stood at is not lost on navigation.
pub fn pager(view: ViewId, arranged: &Arranged<'_>, width: f32) -> Element<Msg> {
    let step = |glyph: &str, to: usize, enabled: bool| {
        let step = theme::surface_button(
            icon::<Msg>(glyph).size(9.0).color(if enabled { theme::INK_SOFT } else { theme::LINE_STRONG }),
            false,
        )
        .width(Length::Fixed(STEP_WIDTH))
        .height(Length::Fixed(STEP_HEIGHT));
        if enabled { step.on_press(Msg::In(view, ViewMsg::Page(to))) } else { step }
    };

    let range = arranged.range();
    let pages = numbers(arranged.page, arranged.pages, room(width, arranged.pages, &range))
        .into_iter()
        .map(|slot| match slot {
            Some(index) => theme::page_button(
                text::<Msg>((index + 1).to_string())
                    .size(theme::TEXT_LABEL)
                    .color(theme::INK_MUTED)
                    .single_line(),
                index == arranged.page,
            )
            .height(Length::Fixed(STEP_HEIGHT))
            .on_press(Msg::In(view, ViewMsg::Page(index)))
            .into(),
            None => text::<Msg>("…")
                .size(theme::TEXT_LABEL)
                .color(theme::INK_DIM)
                .single_line()
                .into(),
        });

    let bar = row![
        text::<Msg>(range).size(theme::TEXT_LABEL).color(theme::INK_DIM).single_line(),
        // Spacer: label on the left, pages on the right.
        space::<Msg>(Length::Fill, Length::Fixed(0.0)),
    ]
    .push(step(theme::glyph::LEFT, arranged.page.saturating_sub(1), arranged.page > 0))
    .extend(pages)
    .push(step(theme::glyph::RIGHT, arranged.page + 1, arranged.page + 1 < arranged.pages))
    .spacing(STEP_GAP)
    .width(Length::Fill)
    .align_items(Alignment::Center)
    .padding(Padding { top: 7.0, bottom: 7.0, left: theme::GUTTER, right: theme::GUTTER });

    container(bar).width(Length::Fill).background(theme::SHELF).into()
}

/// Date field in the filter bar: an edge caption and the typed text beside it.
///
/// The width is fixed, not stretching: a date is eight characters, and
/// stretching half the bar under it would take space from the query field,
/// which is exactly the one that stretches. Text beyond the space is cut by the
/// box — the box is the display boundary (see `Container` in
/// ui-service/types.proto).
pub fn date_field(
    caption: &str,
    value: &str,
    on_input: impl Fn(String) -> Msg + 'static,
    on_submit: Msg,
) -> Control {
    let input = text_input::<Msg>("YYYY-MM-DD", value)
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
