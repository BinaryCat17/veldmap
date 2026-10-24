//! components/list_screen.rs — a whole list screen.
//!
//! The three kinds differ in the heading, in having a path, and in where the
//! rows come from. Everything else — filtering, columns, buttons, pages — is
//! shared and lives here: they have nowhere else to diverge.

use veld_ui_service_wrap::{column, row};
use crate::proto::ui_service::{
    container, scrollable, text, Alignment, Element, FontWeight, Length, Padding,
    ScrollDirection, ScrollTo,
};
use crate::module::components::{arrange, controls, format, table, Row};
use crate::module::state::{Batch, ViewId};
use crate::module::state::listing::{ListingState, Menu};
use crate::module::{theme, Msg, ViewMsg};

/// What the screen is signed with and what it shows.
pub struct Screen<'a> {
    pub title: &'a str,
    /// Line under the heading: how many in total and where from.
    pub subtitle: String,
    /// Path of the current folder; empty — the view has no path. It is also the
    /// sign of "one folder is shown": all rows of such a view lie in it, and
    /// this changes not only the heading but also which levers make sense for
    /// the list.
    pub path: Option<&'a str>,
    /// What to show instead of the table when there are no rows: an empty
    /// search and an empty folder have different reasons.
    pub empty: &'a str,
    /// Levers that only this view has — above the shared filter bar. Shared by
    /// the three kinds is how they show rows; where the rows come from is
    /// different for each, and they ask about it in different ways.
    pub controls: Option<Element<Msg>>,
    pub rows: Vec<Row>,
    /// Key of the scene selected by a click on the globe; empty — none is
    /// selected. One for all lists: the choice happens on the globe, but it
    /// must be visible everywhere this scene stands as a row (see
    /// `table::Context::picked`).
    pub picked: &'a str,
    /// Whether there is anything for the batch buttons to do
    /// (`handlers::library::batch`). This is what decides whether to show them.
    pub batch: Batch,
    /// The expanded menu of this list (see `table::Context::menu`).
    pub menu: Option<&'a Menu>,
    /// The row the navigation to this tab led to; empty — the navigation was
    /// not here (see `state::Highlight`).
    pub target: &'a str,
}

/// How many subtitle characters fit in the heading line. By count, not by
/// width: the subtitle stands between the title and the buttons, and its width
/// is whatever is left of both — and the client must truncate it, because the
/// box would clip the buttons instead.
const SUBTITLE_CHARS: usize = 72;

/// The screen heading: the title, the caption under it, and the buttons on the
/// right.
///
/// A role, not an on-site assembly: the heading stands above every list, and
/// written once per screen it diverges in caption size, weight, and height.
pub fn heading(title: &str, subtitle: String, trailing: Vec<Element<Msg>>) -> Element<Msg> {
    let mut line = row![
        text::<Msg>(title.to_string())
            .size(theme::TEXT_TITLE)
            .color(theme::INK)
            .weight(FontWeight::WeightBold)
            .single_line(),
        // Shrinks even though a count arrives here: the line stands before the
        // spacer and any unexpectedly long text pushes the heading buttons
        // ("Clear marks") past the edge, and the box then clips them — making
        // them unclickable. The refusal reason never arrives here at all: it
        // belongs in the middle of an empty list, where it fits whole.
        text::<Msg>(format::ellipsize(&subtitle, SUBTITLE_CHARS))
            .size(theme::TEXT_BODY)
            .color(theme::INK_DIM)
            .single_line(),
        // The buttons are pushed to the right; without them the spacer changes
        // nothing.
        container(veld_ui_service_wrap::space::<Msg>(Length::Fill, Length::Fixed(0.0)))
            .width(Length::Fill),
    ];
    for element in trailing {
        line = line.push(element);
    }

    container(
        line.spacing(10.0)
            .width(Length::Fill)
            .height(Length::Fill)
            .align_items(Alignment::Center),
    )
    .width(Length::Fill)
    .height(Length::Fixed(theme::HEADING_HEIGHT))
    .padding(Padding { top: 0.0, bottom: 0.0, left: theme::GUTTER, right: theme::GUTTER })
    .into()
}

/// Where to scroll the list so the navigation target row ends up in view.
/// `None` — nothing to lead to, or the row is not on this page.
///
/// Counted in rows, not in renderer pixels: the row pitch is ours
/// (`theme::ROW_PITCH`), and only we can measure it. A couple of rows is left
/// above: a row pressed against the top edge reads as the start of the list,
/// not as the place one was led to.
///
/// The request number rides along with the offset: by offset alone the
/// renderer cannot tell "asked again" from "asking for the same" (see
/// `ListingState::aim`).
fn aim(arranged: &arrange::Arranged<'_>, listing: &ListingState, target: &str) -> Option<ScrollTo> {
    if target.is_empty() {
        return None;
    }
    let at = arranged.lines.iter().position(|line| match line {
        arrange::Line::Entry { row, .. } => row.named(target),
        arrange::Line::Group { .. } | arrange::Line::Waiting { .. } => false,
    })?;
    Some(ScrollTo {
        offset: at.saturating_sub(2) as f32 * theme::ROW_PITCH,
        request: listing.aim,
    })
}

/// The whole list screen.
///
/// `width` is the width of the PANE in logical points, not of the window: it
/// decides which columns fit and how many name characters fit into the name
/// column. The window is wider, and computing by it would collapse the name to
/// zero (see `State::pane_width` and [`table::fit`]).
pub fn view(
    view: ViewId,
    screen: Screen<'_>,
    listing: &ListingState,
    width: f32,
) -> Element<Msg> {
    let arranged = arrange::arrange(&screen.rows, listing);
    // What fits into the allotted width is decided by the table itself: half
    // the screen is twice narrower than the window, and the columns do not get
    // thinner because of that (see table::fit).
    //
    // Optional columns are needed only if there is something to expand and
    // something to mark — and they are counted by what is drawn, not by the
    // top tier: scenes in the catalogue lie under the day folder, that is,
    // they appear only in an expanded row. Counted by the top tier, the mark
    // column would be gone entirely, and there would be nothing to outline a
    // lazily expanded scene with, even though it is on screen (see
    // `Arranged::marks`).
    let optional = table::Optional {
        twisty: arranged.shown().any(Row::expandable),
        checkable: arranged.marks().next().is_some(),
    };
    let table::Fit { columns, name: name_width, compact } = table::fit(width, optional);

    // What is selected and what can be done with it. Nothing about the globe
    // here: outline and display are not selection, they have their own icons
    // in the row and their own list ("Under viewing").
    let mut trailing: Vec<Element<Msg>> = Vec::new();
    if !listing.selected.is_empty() {
        trailing.push(
            text::<Msg>(format!("{} selected", listing.selected.len()))
                .size(theme::TEXT_LABEL)
                .color(theme::ACCENT_TEXT)
                .single_line()
                .into(),
        );
        // With icons, not with labels: the pane is split in half precisely so
        // that the globe stands next to it, and three labels in a row do not
        // fit into the remaining half — the right edge clips them along with
        // the action. What an icon does is told by the tooltip, in the same
        // words.
        let mut act = |glyph: &'static str, tone: theme::IconTone, hint: &str, msg: ViewMsg| {
            trailing.push(table::icon_button(glyph, tone, hint, Msg::In(view, msg)));
        };
        // The button stands only when it has something to do: one can select
        // both what is not on disk and what is already fully downloaded.
        if screen.batch.fetchable {
            act(
                theme::glyph::DOWNLOAD,
                theme::IconTone::Rest,
                "Download selected",
                ViewMsg::CheckDownload,
            );
        }
        if screen.batch.deletable {
            act(
                theme::glyph::TRASH,
                theme::IconTone::Danger,
                "Delete selected",
                ViewMsg::CheckDelete,
            );
        }
        act(
            theme::glyph::BOX,
            theme::IconTone::Rest,
            "Clear selection",
            ViewMsg::CheckClear,
        );
    }
    let heading = heading(screen.title, screen.subtitle, trailing);

    // An empty list and a list emptied by the filter are different things:
    // "nothing downloaded" under the chosen filter would be untrue.
    let nothing = if screen.rows.is_empty() {
        screen.empty
    } else {
        "Nothing matched the filter"
    };

    let body: Element<Msg> = if arranged.lines.is_empty() {
        theme::empty(nothing).into()
    } else {
        scrollable(table::body(
            view,
            &arranged.lines,
            table::Context {
                listing,
                menu: screen.menu,
                columns: &columns,
                name_width,
                compact,
                shared: format::shared(
                    &arranged.shown().map(|row| row.title.as_str()).collect::<Vec<&str>>(),
                    format::mono_fit(name_width, theme::TEXT_MONO),
                ),
                here: screen.path.unwrap_or_default(),
                picked: screen.picked,
                target: screen.target,
                now: format::now(),
            },
        ))
            .direction(ScrollDirection::ScrollVertical)
            .scrollbar(theme::scrollbar())
            // The name is own to every tab: the tooltip is addressed by it,
            // and there can be two lists on screen.
            .name(format!("rows:{}", view))
            .scroll_to(aim(&arranged, listing, screen.target))
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    };

    let mut screen_rows: Vec<Element<Msg>> = vec![heading.into()];
    if let Some(path) = screen.path {
        screen_rows.push(controls::path(view, path));
    }
    if let Some(own) = screen.controls {
        screen_rows.push(own);
    }
    // Grouping by folders makes sense where rows are gathered from everywhere.
    // Having a path means exactly that: a view with a path shows the contents
    // of one folder, all its rows lie in it, and there would be exactly one
    // heading above them. No second sign is needed for this — this one is
    // already there.
    screen_rows.push(controls::toolbar(
        view, listing, screen.menu, &arranged.counts, screen.path.is_none(), width));
    screen_rows.push(table::header(view, &columns, arranged.all_marked(listing), compact));
    screen_rows.push(body);
    if arranged.pages > 1 {
        screen_rows.push(theme::hairline(theme::LINE_SOFT));
        screen_rows.push(controls::pager(view, &arranged, width));
    }

    column(screen_rows).width(Length::Fill).height(Length::Fill).into()
}
