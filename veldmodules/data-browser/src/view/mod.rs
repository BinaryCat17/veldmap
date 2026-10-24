//! View rendering for data-browser.
//!
//! This is the window frame: a tree of panes, each with its own tab strip and
//! the body of its active tab, and below them one shared status bar. The tab
//! contents are assembled by the sibling modules.
//!
//! The status bar is one per window, not per pane: it reports what the
//! application does as a whole — whether downloads are running and how much is
//! taken on disk — and has nothing to do with what this pane is showing.

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


/// Height of the tab strip. Fixed, not derived from the content: this is
/// constant-size chrome, and no tab may stretch it with itself. The status bar
/// is an ordinary chrome bar (theme::BAR_HEIGHT).
const TAB_STRIP_HEIGHT: f32 = 38.0;

/// Gap between the items in a chrome bar. One for both bars: they read as one
/// language, and differing gaps would show.
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

/// A tree node: a pane or a split in two (see state::layout).
///
/// The renderer matches widget state — list scroll, field caret — by the place
/// in the layout tree, and a split changes that place: a pane that was the root
/// becomes the child of a row. Its list scroll therefore does not survive the
/// move, and this is the price of arbitrary splits: the shape of the layout
/// dictates the shape of the layout tree, and the two must coincide.
fn node(state: &State, node: &Node) -> Element<Msg> {
    let split = match node {
        Node::Leaf(leaf) => return pane(state, leaf.id),
        Node::Split(split) => split,
    };

    let (head, tail) = portions(split.fraction);
    let first = container(self::node(state, &split.first));
    let second = container(self::node(state, &split.second));
    // The border is not a line between the children but a widget: it is dragged
    // with the mouse, and the children measure their place by it (see
    // `Msg::Divide`).
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

/// The first child's share, in shares of the space: the layout has no other way
/// to say "this part is twice as wide". Thousandths, because the shares are
/// integers, and a thousandth of the screen is finer than the eye can tell.
///
/// Neither part reaches zero: a zero-size pane stays in the tree with all its
/// tabs but becomes unreachable — there would be nothing left to give its place
/// back.
fn portions(fraction: f32) -> (u16, u16) {
    let head = ((fraction * 1000.0).round() as i32).clamp(1, 999) as u16;
    (head, 1000 - head)
}

/// One pane: its own tab strip and the body of its active tab.
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

    // The pane body is a drop target: into the middle a tab is placed, at an
    // edge it divides the place (see `Msg::TabDrop`). The tab strip accepts too,
    // but it has no edges: there is nothing to divide a strip by.
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

/// An empty tab: a place where what to show is not decided yet. It asks that
/// directly — with a list of what there is, not an empty place with a cross in
/// the corner.
fn empty_tab(id: ViewId) -> Element<Msg> {
    chooser("Empty tab", "Choose what to show here.", |kind| {
        Msg::In(id, ViewMsg::Fill(kind))
    })
}

/// A pane with no tab left in it. So it is with the last one — other emptied
/// panes leave the tree (see `State::close`).
fn nothing_open(pane: PaneId) -> Element<Msg> {
    chooser("All tabs closed", "Choose where to start.", move |kind| {
        Msg::NewTab(pane, kind)
    })
}

/// A choice of the tab kind. What to do with it — fill an empty tab or start a
/// new one in an emptied pane — is decided by the caller: they ask about one
/// and the same, but answer with different things.
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

/// The tab strip of one pane. A tab is addressed by its `ViewId`, not by its
/// position: the position changes when a neighbor is closed.
///
/// Tabs do not shrink but move under horizontal scroll: shrinking drives the
/// label to zero width, and such text takes height instead of width (see
/// `Wrapping` in types.proto).
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

        // The cross and "more" are separate buttons inside the tab: a press on
        // them does not reach the tab itself, so it does not select it as well.
        //
        // The menu button is absent while there is nothing to offer: the only
        // tab on the whole screen has nowhere to move, and "close" is at hand
        // anyway with the cross — a menu of that one item would promise a
        // choice that does not exist.
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
        // The tab is taken with the mouse and carried: it returns in the drop
        // event under the name it calls itself.
        .draggable(view.id.to_string());

        // Underline of the active one: the frame in this protocol is one for
        // all sides, while only the bottom one is needed.
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

/// What the tab can do with the layout: where to move and whether the panes can
/// be collapsed back.
///
/// The directions offered are the ones that change the screen: moving where a
/// neighbor already is means placing the tab next to it; where there is none —
/// means dividing the screen with that move. An item that would change nothing
/// promises a choice and answers with silence, so it is not here at all (see
/// `State::can_move`).
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

/// The tab's own menu: where to put it. Closing as the last item — it is about
/// the tab as a whole, not about its place.
fn tab_options(mut items: Vec<menu::Item>, id: ViewId) -> Vec<menu::Item> {
    items.push(menu::Item::new("Close tab", Msg::TabClose(id)).glyph(theme::glyph::CLOSE));
    items
}

/// The mark of the move direction — the same arrow with which the direction is
/// shown everywhere else.
fn side_glyph(side: Side) -> &'static str {
    match side {
        Side::Left => theme::glyph::LEFT,
        Side::Right => theme::glyph::RIGHT,
        Side::Above => theme::glyph::UP,
        Side::Below => theme::glyph::DOWN,
    }
}

/// The glyph of the tab being offered — the same one it is signed with later
/// (see [`glyph`]).
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

/// The status bar: what is happening right now and how much is taken on disk.
/// There are no empty places in it — what there is nothing to say about is not
/// shown at all.
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
    // A notice of a just-failed action outranks the library failure: it is
    // about what the person did a second ago, and there is no room for both in
    // the bar.
    if let Some(said) = state.notice.as_ref().or(state.error.as_ref()) {
        // Ellipsized: both strings carry what the failing side said — a folder
        // path, a provider reason — and nothing limits its length. The bar goes
        // as one line, so an untouched reason does not wrap but rides past the
        // window edge. It is always in the log in full.
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

/// The view glyph — the same one its item is signed with in the open menu: the
/// tab and the item denote one and the same (see `ViewKind::opened_as`). Only
/// the scene preview has no mark of its own — it is not opened from the menu.
fn glyph(kind: &ViewKind) -> &'static str {
    kind.opened_as().map_or(theme::glyph::IMAGE, tab_glyph)
}
