pub mod browse;
pub mod downloaded;
pub mod globe;
pub mod preview;
pub mod search;
pub mod shown;

use veld_ui_service_wrap::{column, row, Keyed};
use crate::proto::ui_service::{
    container, icon, popover, progress_bar, space, text, Alignment, Color, DividerAxis, Element,
    FontWeight, Length, Padding, ScrollDirection, scrollable,
};
use crate::module::components::{format, menu};
use crate::module::state::listing::Choice;
use crate::module::state::{Axis, Node, Open, PaneId, Side, State, ViewId, ViewKind};
use crate::module::{theme, Msg, NewTab, ViewMsg};

const TAB_STRIP_HEIGHT: f32 = 38.0;

pub const BAR_SPACING: f32 = 10.0;

pub fn build_root(state: &State) -> Element<Msg> {
    container(
        column![
            container(node(state, state.layout().root()))
                .width(Length::Fill)
                .height(Length::Fill),
            theme::hairline(theme::LINE),
            status_bar(state),
        ]
        .width(Length::Fill)
        .height(Length::Fill),
    )
    .background(theme::PAGE)
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

fn node(state: &State, node: &Node) -> Element<Msg> {
    let split = match node {
        Node::Leaf(leaf) => return pane(state, leaf.id),
        Node::Split(split) => split,
    };

    let (head, tail) = portions(split.fraction);
    let first = container(self::node(state, &split.first));
    let second = container(self::node(state, &split.second));
    let id = split.id;
    let border = |axis| theme::divider(axis, move |delta| Msg::Divide(id, delta), Msg::Divided);
    match split.axis {
        Axis::Row => row![
            first.width(Length::FillPortion(head)).height(Length::Fill),
            border(DividerAxis::DividerVertical),
            second.width(Length::FillPortion(tail)).height(Length::Fill),
        ]
        .width(Length::Fill)
        .height(Length::Fill)
        .into(),
        Axis::Column => column![
            first.width(Length::Fill).height(Length::FillPortion(head)),
            border(DividerAxis::DividerHorizontal),
            second.width(Length::Fill).height(Length::FillPortion(tail)),
        ]
        .width(Length::Fill)
        .height(Length::Fill)
        .into(),
    }
}

fn portions(fraction: f32) -> (u16, u16) {
    let head = ((fraction * 1000.0).round() as i32).clamp(1, 999) as u16;
    (head, 1000 - head)
}

fn pane(state: &State, pane: PaneId) -> Element<Msg> {
    let shown = state.active_in(pane).and_then(|id| state.get(id).map(|kind| (id, kind)));
    let body: Element<Msg> = match shown {
        Some((id, ViewKind::Empty)) => empty_tab(id),
        Some((id, ViewKind::Browse(view))) => browse::view(state, id, view),
        Some((id, ViewKind::Search(view))) => search::view(state, id, view),
        Some((id, ViewKind::Downloaded(listing))) => downloaded::view(state, id, listing),
        Some((id, ViewKind::Preview(view))) => preview::view(state, id, view),
        Some((id, ViewKind::Globe(view))) => globe::view(state, id, view),
        Some((id, ViewKind::Shown)) => shown::view(state, id),
        None => nothing_open(pane),
    };

    column![
        tab_strip(state, pane),
        theme::hairline(theme::LINE),
        container(body)
            .width(Length::Fill)
            .height(Length::Fill)
            .on_drop(move |drop| Msg::TabDrop(pane, drop), true, theme::DROP_HINT),
    ]
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

fn empty_tab(id: ViewId) -> Element<Msg> {
    chooser("Empty tab", "Choose what to show here.", |kind| {
        Msg::In(id, ViewMsg::Fill(kind))
    })
}

fn nothing_open(pane: PaneId) -> Element<Msg> {
    chooser("All tabs closed", "Choose where to start.", move |kind| {
        Msg::NewTab(pane, kind)
    })
}

fn chooser(title: &str, hint: &str, message: impl Fn(NewTab) -> Msg) -> Element<Msg> {
    let choices = NewTab::KINDS.iter().map(|kind| {
        theme::surface_button(
            row![
                icon::<Msg>(tab_glyph(*kind)).size(12.0).color(theme::ACCENT),
                text::<Msg>(kind.title().to_string()).size(theme::TEXT_BODY).single_line(),
            ]
            .spacing(10.0)
            .align_items(Alignment::Center),
            false,
        )
        .width(Length::Fill)
        .padding(Padding { top: 9.0, bottom: 9.0, left: 12.0, right: 12.0 })
        .align_x(Alignment::Start)
        .on_press(message(*kind))
        .into()
    });

    container(
        column![
            text::<Msg>(title.to_string())
                .size(theme::TEXT_TITLE)
                .color(theme::INK)
                .weight(FontWeight::WeightBold)
                .single_line(),
            text::<Msg>(hint.to_string()).size(theme::TEXT_BODY).color(theme::INK_DIM),
            container(space::<Msg>(Length::Fixed(0.0), Length::Fixed(6.0))),
            column(choices).spacing(6.0).width(Length::Fill),
        ]
        .spacing(6.0)
        .width(Length::Fixed(340.0)),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .center_x()
    .center_y()
    .into()
}

fn tab_strip(state: &State, pane: PaneId) -> Element<Msg> {
    let active = state.active_in(pane);
    let tabs = state.views_in(pane).map(|view| {
        let current = Some(view.id) == active;
        let label = row![
            icon::<Msg>(glyph(&view.kind))
                .size(10.0)
                .color(if current { theme::ACCENT } else { theme::INK_FAINT }),
            text::<Msg>(view.kind.title()).size(theme::TEXT_BODY).single_line(),
        ]
        .spacing(7.0)
        .align_items(Alignment::Center);

        let options = state.open == Open::TabOptions(view.id);
        let arrangements = arrangements(state, view.id);
        let more: Element<Msg> = match arrangements.is_empty() {
            true => theme::nothing(),
            false => popover(
                theme::chrome_icon(
                    icon::<Msg>(theme::glyph::SPLIT).size(9.0).color(theme::INK_FAINT),
                )
                .on_press(Msg::TabOptions(if options { None } else { Some(view.id) })),
                options,
                || menu::panel(tab_options(arrangements, view.id)),
            )
            .gap(4.0)
            .on_dismiss(Msg::TabOptions(None))
            .into(),
        };

        let tab = theme::tab(
            row![
                label,
                more,
                theme::chrome_icon(icon::<Msg>(theme::glyph::CLOSE).size(9.0).color(theme::INK_FAINT))
                    .on_press(Msg::TabClose(view.id)),
            ]
            .spacing(4.0)
            .align_items(Alignment::Center),
            current,
        )
        .height(Length::Fill)
        .on_press(Msg::TabSelect(view.id))
        .draggable(view.id.to_string());

        column![
            container(tab).height(Length::Fill).width(Length::Shrink),
            theme::hairline(if current { theme::ACCENT } else { Color::TRANSPARENT }),
        ]
        .width(Length::Shrink)
        .key(view.id.to_string())
        .into()
    });

    let strip = row(tabs)
        .spacing(2.0)
        .width(Length::Fill)
        .height(Length::Fill)
        .padding(Padding { top: 5.0, right: 7.0, bottom: 0.0, left: 7.0 })
        .align_items(Alignment::End);

    let open = state.open == Open::TabMenu(pane);
    let opener = popover(
        theme::chrome_icon(icon::<Msg>(theme::glyph::PLUS).size(11.0).color(theme::INK_DIM))
            .width(Length::Fixed(TAB_STRIP_HEIGHT))
            .height(Length::Fill)
            .on_press(Msg::TabMenu(if open { None } else { Some(pane) })),
        open,
        || {
            menu::panel(
                NewTab::ALL
                    .iter()
                    .map(|kind| {
                        menu::Item::new(kind.title(), Msg::NewTab(pane, *kind))
                            .glyph(tab_glyph(*kind))
                    })
                    .collect(),
            )
        },
    )
    .gap(2.0)
    .on_dismiss(Msg::TabMenu(None));

    container(
        row![
            opener,
            theme::vline(theme::LINE_SOFT),
            scrollable(strip)
                .direction(ScrollDirection::ScrollHorizontal)
                .scrollbar(theme::scrollbar())
                .width(Length::Fill)
                .height(Length::Fill),
        ]
        .width(Length::Fill)
        .height(Length::Fill),
    )
    .on_drop(move |drop| Msg::TabDrop(pane, drop), false, theme::DROP_HINT)
    .background(theme::CHROME)
    .width(Length::Fill)
    .height(Length::Fixed(TAB_STRIP_HEIGHT))
    .into()
}

fn arrangements(state: &State, id: ViewId) -> Vec<menu::Item> {
    let mut items = Vec::new();
    for &side in Side::ALL {
        if state.can_move(id, side) {
            items.push(
                menu::Item::new(side.title(), Msg::TabMove(id, side)).glyph(side_glyph(side)),
            );
        }
    }
    if state.layout().count() > 1 {
        items.push(menu::Item::new("Collapse panes", Msg::TabCollapse).glyph(theme::glyph::SPLIT));
    }
    items
}

fn tab_options(mut items: Vec<menu::Item>, id: ViewId) -> Vec<menu::Item> {
    items.push(menu::Item::new("Close tab", Msg::TabClose(id)).glyph(theme::glyph::CLOSE));
    items
}

fn side_glyph(side: Side) -> &'static str {
    match side {
        Side::Left => theme::glyph::LEFT,
        Side::Right => theme::glyph::RIGHT,
        Side::Above => theme::glyph::UP,
        Side::Below => theme::glyph::DOWN,
    }
}

fn tab_glyph(kind: NewTab) -> &'static str {
    match kind {
        NewTab::Browse => theme::glyph::FOLDER,
        NewTab::Search => theme::glyph::SEARCH,
        NewTab::Downloaded => theme::glyph::DOWNLOAD,
        NewTab::Globe => theme::glyph::GLOBE,
        NewTab::Shown => theme::glyph::LAYERS,
        NewTab::Empty => theme::glyph::PLUS,
    }
}

fn status_bar(state: &State) -> Element<Msg> {
    let mut parts: Vec<Element<Msg>> = Vec::new();
    let label = |content: String, color: Color| {
        text::<Msg>(content).size(theme::TEXT_LABEL).color(color).single_line()
    };

    let (count, done, total) = state.library.downloading();
    if count > 0 {
        parts.push(
            label(
                format!("Downloading {} {}", count, format::plural(count, ["file", "files", "files"])),
                theme::ACCENT_TEXT,
            )
                .weight(FontWeight::WeightMedium)
                .into(),
        );
        if total > 0 {
            parts.push(
                container(
                    progress_bar::<Msg>(0.0..=1.0, done as f32 / total as f32)
                        .style(theme::progress(theme::ACCENT))
                        .width(Length::Fixed(88.0))
                        .height(Length::Fixed(5.0)),
                )
                .height(Length::Fill)
                .align_y(Alignment::Center)
                .into(),
            );
        }
        if state.speed > 0.0 {
            parts.push(label(format!("{}/s", format::bytes(state.speed as u64)), theme::INK_DIM).into());
        }
        parts.push(label("|".to_string(), theme::LINE).into());
    }

    parts.push(label(format!("on disk {}", format::bytes(state.library.stored())), theme::INK_DIM).into());
    parts.push(theme::spacer().into());
    if let Some(said) = state.notice.as_ref().or(state.error.as_ref()) {
        parts.push(label(format::ellipsize(said, 72), theme::DANGER).into());
    }

    theme::chrome_bar(
        row(parts)
            .spacing(BAR_SPACING)
            .width(Length::Fill)
            .height(Length::Fill)
            .align_items(Alignment::Center),
    )
    .into()
}

fn glyph(kind: &ViewKind) -> &'static str {
    kind.opened_as().map_or(theme::glyph::IMAGE, tab_glyph)
}
