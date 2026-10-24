//! components/table.rs — listing table: header, rows, row menu.
//!
//! One table for all three views. What can be done with a row is derived from
//! the row itself, not from the view: a folder is opened where it exists, a
//! downloaded item is viewed where it was downloaded — and the tab has nothing
//! to add. So both the columns and the buttons are the same in all three views.

use veld_ui_service_wrap::{column, row, Keyed, Row as RowBuilder};
use crate::proto::ui_service::{
    container, icon, mono, popover, progress_bar, text, tooltip,
    Alignment, Color, Container, Element, FontWeight, Length, Padding, TooltipPosition,
};
use crate::module::components::{arrange::Line, format, OnGlobe, OnOutline, Row, RowKind, RowStatus};
use crate::module::state::listing::{ListingState, Menu};
use crate::module::state::overlay::Pace;
use crate::module::state::ViewId;
use crate::module::{theme, Msg, ViewMsg};

/// Table column. An enumeration, not a list of widths: there are as many
/// columns as cells in a row, and tying one to the other by position in an
/// array means one day shifting a cell onto a neighboring column.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Column {
    /// Row selection: a set for batch actions (see handlers::outline).
    /// Its own column for the same reason as expansion — it is a separate
    /// action, and it must be pressed outside the row's main action.
    Check,
    /// Expansion triangle. Its own column, not a glyph inside the name:
    /// expansion is an action, and it must be pressed outside the row itself,
    /// otherwise it competes with the row's main action.
    Twist,
    Icon,
    Name,
    Format,
    Date,
    Size,
    Status,
    Progress,
    Actions,
}

/// All columns in display order. One list for the whole table: the grid of the
/// header, the group title, and the row must match, and three copies of it
/// would sooner or later drift apart.
const COLUMNS: [Column; 10] = [
    Column::Check,
    Column::Twist,
    Column::Icon,
    Column::Name,
    Column::Format,
    Column::Date,
    Column::Size,
    Column::Status,
    Column::Progress,
    Column::Actions,
];

/// The order in which columns give up space when it is scarce. Date and size
/// go first: they are reference facts about the record, and they do not go
/// away — the record is the same in an hour. Loading yields next, but before
/// status and format: it tells what is happening right now, while of its two
/// neighbors one names the product kind and the other where it lies, and both
/// distinguish rows from one another. The glyph goes last: the row kind is
/// visible from its name too, but while there is room, the glyph reads faster.
///
/// Giving up a column does not lose the work: its place is the strip under the
/// snapshot row, and it is drawn exactly where the column is absent (see
/// [`strip_shown`]).
///
/// Name and buttons are absent from this list: the name is the only stretching
/// one, and squeezing drives it to zero width (see `Wrapping` in types.proto),
/// while a row without a name and without actions is not a row.
const DROP_ORDER: [Column; 6] =
    [Column::Date, Column::Size, Column::Progress, Column::Status, Column::Format, Column::Icon];

/// The check and the expansion triangle are narrow columns before the glyph.
/// They have no padding at all (see [`padding_of`]), so the width here is
/// exactly the room that goes to the button.
///
/// Measured by the glyph, not by eye: a row with nothing to mark and nothing
/// to expand leaves both empty, and any extra in them reads as indentation —
/// as if a file were nested where there is no nesting.
const CHECK: f32 = 20.0;
const TWIST: f32 = 14.0;
const ICON: f32 = 34.0;
/// Sized for the product type, not the file extension: for radar it is long
/// (`IW_GRDH_1SDV`), and three letters like "png" cannot measure such a column.
const FORMAT: f32 = 80.0;
const DATE: f32 = 88.0;
const SIZE: f32 = 84.0;
const STATUS: f32 = 104.0;
/// Sized for the progress caption, not just a bar: the cell holds a phrase
/// saying exactly what is being loaded, and the column is measured by it —
/// "step 10/12 · 8.6/25 MB" against [`format::text_width`].
const PROGRESS: f32 = 148.0;
/// How many buttons fit in the actions column. A snapshot stored as a folder
/// has the most: download whole, view, put on the globe, and the menu.
const ACTION_BUTTONS: f32 = 4.0;

/// Width of the actions column. Computed from the button count, not tuned by
/// eye — otherwise an added button would silently slide under the cell's clip
/// (see `grid`).
const ACTIONS: f32 =
    theme::ROW_BUTTON * ACTION_BUTTONS + BUTTON_GAP * (ACTION_BUTTONS - 1.0) + CELL_PADDING * 2.0;

/// The same column when only one menu is left in it.
const ACTIONS_COMPACT: f32 = theme::ROW_BUTTON + CELL_PADDING * 2.0;

/// Below this name not even a recognizable piece remains, and the actions
/// column gives up its menu glyphs (see [`Fit::compact`]). Smaller than
/// [`NAME_MIN`]: that is the width for which neighboring columns go, while
/// this is the boundary past which a row stops being a row.
const NAME_FLOOR: f32 = 64.0;

/// How much room a column takes. The name has no width of its own — it takes
/// the remainder.
fn width_of(column: Column) -> f32 {
    match column {
        Column::Check => CHECK,
        Column::Twist => TWIST,
        Column::Icon => ICON,
        Column::Name => 0.0,
        Column::Format => FORMAT,
        Column::Date => DATE,
        Column::Size => SIZE,
        Column::Status => STATUS,
        Column::Progress => PROGRESS,
        Column::Actions => ACTIONS,
    }
}

/// The width for which columns give up space: while the name gets less, the
/// next neighbor goes. It is not a promise — sometimes there is nothing left
/// to give up, and then the name gets what remains (see [`fit`]).
const NAME_MIN: f32 = 150.0;

/// Gap between the row's buttons. Public because the same button row stands in
/// the layer list, and a second record of this gap would drift from the first.
pub const BUTTON_GAP: f32 = 6.0;

/// Cell content padding from its edges — on both sides.
const CELL_PADDING: f32 = 8.0;

/// Which optional columns this listing needs. The one who assembled the rows
/// knows this: sometimes there is nothing to expand or mark, and an empty
/// column would then shift the whole listing for something that never happens
/// in it.
#[derive(Clone, Copy, Default)]
pub struct Optional {
    /// There is something to expand.
    pub twisty: bool,
    /// There is something to select — that is, the listing has at least one
    /// choosable row (see `Row::choosable`).
    pub checkable: bool,
}

/// Which columns fit in the allotted width and how much goes to the name.
///
/// Half the screen is twice as narrow as the window, and the fixed columns do
/// not get thinner — in a narrow place their sum covers all the room, and the
/// stretching name collapses to zero. So the extra ones are not squeezed but
/// dropped: a listing without a date reads, a listing without a name does not.
///
/// The returned name width is what it actually gets, even if that is less than
/// [`NAME_MIN`]: the ellipsis is computed from it (`format::mono_fit`), and a
/// name sized with a margin would not fit its cell — instead of an ellipsis the
/// clip would cut it off mid-glyph.
pub fn fit(width: f32, optional: Optional) -> Fit {
    let mut columns: Vec<Column> = COLUMNS.to_vec();
    if !optional.checkable {
        columns.retain(|column| *column != Column::Check);
    }
    if !optional.twisty {
        columns.retain(|column| *column != Column::Twist);
    }
    // Screen gutters and the name column's own padding are always spent.
    let overhead = theme::GUTTER * 2.0 + CELL_PADDING * 2.0;
    let taken = |columns: &[Column], compact: bool| -> f32 {
        columns
            .iter()
            .map(|column| match (*column, compact) {
                (Column::Actions, true) => ACTIONS_COMPACT,
                (other, _) => width_of(other),
            })
            .sum::<f32>()
            + overhead
    };

    for dropped in DROP_ORDER {
        if width - taken(&columns, false) >= NAME_MIN {
            break;
        }
        columns.retain(|column| *column != dropped);
    }
    // There is nothing left to give up, and the name has not even a
    // recognizable piece left: then the row's glyphs give up. They do not go
    // nowhere — the same action stands as a menu item, and the only difference
    // is whether there is room for it.
    let compact = width - taken(&columns, false) < NAME_FLOOR;
    let name = (width - taken(&columns, compact)).max(0.0);
    Fit { columns, name, compact }
}

/// What came out of [`fit`]: which columns to show, how much went to the name,
/// and whether the actions column had to shrink to a single menu.
pub struct Fit {
    pub columns: Vec<Column>,
    pub name: f32,
    /// The row's glyphs did not fit and moved into its menu.
    pub compact: bool,
}

/// Grouping step.
const INDENT: f32 = 12.0;

/// Header height — its own: it holds captions, not data rows.
const HEADER_HEIGHT: f32 = 22.0;

/// Progress bar of the globe placement under the snapshot row. Thin: it
/// answers one question — "is it going" — while what exactly is going and how
/// much is left is said in words in the LOADING column and in the "Being
/// viewed" list.
const ONTO_GLOBE: f32 = 3.0;

/// Bar in the LOADING cell. Thicker than the one under the row: there a single
/// bar covers the whole row and stands out by its length, while here it sits
/// under the caption and is measured by it.
const LOAD_BAR: f32 = 4.0;

/// Extensions that have a preview — the row glyph is chosen by them too.
const IMAGE_FORMATS: [&str; 7] = ["png", "jpg", "jpeg", "tif", "tiff", "jp2", "webp"];

/// A grid row: one cell per shown column. The cell clips its content at its
/// own edges — otherwise a long name is drawn over the neighboring column (see
/// `Wrapping` in types.proto).
///
/// `indent` is the grouping step. It widens the first column instead of
/// shifting the glyph inside it: the icon column holds exactly the glyph, and
/// a step laid inside it hits its edge — the second tier gets clipped, the
/// third and further stand in one place. The widening is taken by the name
/// column (the only stretching one), so both the glyph and the caption move
/// right, while everything to the right stays aligned to its places.
///
/// The caller sets the height: the header has its own.
fn grid(
    cells: impl Fn(Column) -> Element<Msg>,
    columns: &[Column],
    indent: f32,
    compact: bool,
) -> RowBuilder<Msg> {
    // The grouping step widens the first shown column, whichever it is: the
    // check and expansion columns are not always present, and tying the step
    // to them would drop the group indent where there is nothing to mark or
    // expand.
    let first = columns.first().copied();
    row(columns.iter().map(|column| {
        let step = if Some(*column) == first { indent } else { 0.0 };
        let pad = padding_of(*column);
        let (width, left) = match (column, compact) {
            // The step goes to the name too when it turns out to be the first
            // shown column: the name has no width of its own, so the step goes
            // into the padding. Otherwise the grouping tiers would collapse
            // into one — while the name width would still be computed minus
            // the step, i.e. trimmed for a padding that does not exist (see
            // `entry_line`).
            (Column::Name, _) => (Length::Fill, pad + step),
            (Column::Actions, true) => (Length::Fixed(ACTIONS_COMPACT + step), pad + step),
            (other, _) => (Length::Fixed(width_of(*other) + step), pad + step),
        };
        container(cells(*column))
            .width(width)
            .height(Length::Fill)
            .align_y(Alignment::Center)
            .padding(Padding { top: 0.0, bottom: 0.0, left, right: pad })
            .into()
    }))
    .width(Length::Fill)
}

/// Cell padding on the sides. The button columns have none: the button takes
/// the whole cell, and eight points of padding on each side would leave only a
/// stub of the twenty-four — the glyph in such a cell is drawn cropped, and
/// there is nowhere to press it.
fn padding_of(column: Column) -> f32 {
    match column {
        Column::Check | Column::Twist => 0.0,
        _ => CELL_PADDING,
    }
}

/// Grid indent from the screen edges. For a row it is set by the row button
/// itself (`theme::row_button`) — it must be pressable on the margins too — so
/// here it is given only to those that are not buttons: the header and the
/// group title. A twice-applied indent narrows the columns, while the name clip
/// knows nothing about it and draws the name over the neighbor.
fn gutters<M>(content: impl Into<Element<M>>) -> Container<M> {
    container(content)
        .width(Length::Fill)
        .padding(Padding { top: 0.0, bottom: 0.0, left: theme::GUTTER, right: theme::GUTTER })
}

/// Everything a row needs to know beyond itself. One value instead of four
/// parameters on every function: the list of what "needs to be known" grows,
/// and the signatures stay readable.
#[derive(Clone, Copy)]
pub struct Context<'a> {
    pub listing: &'a ListingState,
    /// The open menu of this listing; `None` — something else is open or
    /// nothing. It lives in the screen state (`State::open`), not the view:
    /// at most one thing is open across the whole screen.
    pub menu: Option<&'a Menu>,
    /// Columns that fit in the allotted width — those are the ones drawn (see
    /// [`fit`]). One set for the whole table: the header, the group title, and
    /// the row must match on the grid.
    pub columns: &'a [Column],
    /// Width of the name column in layout points.
    pub name_width: f32,
    /// The row's glyphs did not fit: only its menu is left, and what stood as
    /// glyphs moved into it as items (see [`Fit::compact`]).
    pub compact: bool,
    /// How many name characters match across all rows of the page — at the
    /// head and the tail (see [`format::shared`]). One for the whole table:
    /// every row is cut by it, and computing it per row would cut neighbors
    /// differently.
    pub shared: (usize, usize),
    /// The folder currently shown; empty — the view does not know it (the
    /// download, the search). By it one can see where "show in catalogue"
    /// would be pointless.
    pub here: &'a str,
    /// Key of the snapshot whose outline is selected on the globe; empty —
    /// none is selected. It lives in the application state (`State::pick`),
    /// not the view: the selection happens on the globe, but it must be seen
    /// in the listing the snapshot comes from.
    pub picked: &'a str,
    /// The row a transition led to — it is highlighted on par with the one
    /// selected on the globe: both answer the same question "what is the main
    /// thing here now", and there is nothing to tell them apart by.
    pub target: &'a str,
    /// Reference point for time captions. One for the whole table: rows of one
    /// frame must be measured from one "now", otherwise neighboring dates
    /// cross midnight differently.
    pub now: i64,
}

/// The header. It stands outside the scroll, so it does not move away with the
/// rows.
///
/// `all` — whether everything visible under it is marked (see
/// `Arranged::all_marked`): the check column holds the same box as the rows,
/// and it acts on the very set it speaks of.
pub fn header(view: ViewId, columns: &[Column], all: bool, compact: bool) -> Element<Msg> {
    let label = |name: &str| {
        text::<Msg>(name.to_string())
            .size(theme::TEXT_HEADER)
            .color(theme::INK_DIM)
            .weight(FontWeight::WeightBold)
            .single_line()
            .into()
    };
    let head = grid(
        |column| match column {
            // "Shown", not "all": the box takes the page with everything
            // expanded on it, and the caption must say so — otherwise it
            // promises to select the whole result set but selects twenty rows.
            Column::Check => hinted(
                theme::row_check(Some(all)).on_press(Msg::In(view, ViewMsg::CheckShown(!all))),
                match all {
                    true => "Deselect the shown",
                    false => "Select the shown",
                },
            ),
            Column::Name => label("NAME"),
            Column::Format => label("FORMAT"),
            Column::Date => label("DATE"),
            Column::Size => label("SIZE"),
            Column::Status => label("STATUS"),
            Column::Progress => label("LOADING"),
            Column::Twist | Column::Icon | Column::Actions => theme::nothing(),
        },
        columns,
        0.0,
        compact,
    )
    .height(Length::Fixed(HEADER_HEIGHT));

    column![
        theme::hairline(theme::LINE_SOFT),
        gutters(head).background(theme::SHELF),
        theme::hairline(theme::LINE_SOFT),
    ]
    .width(Length::Fill)
    .into()
}

/// Table rows. Each is named by its own key: the list reorders and loses items
/// from the middle, and widget state must travel with the row, not with the
/// place.
pub fn body(view: ViewId, lines: &[Line<'_>], context: Context<'_>) -> Element<Msg> {
    column(lines.iter().map(|line| match line {
        Line::Group { title, meta, depth } => group_line(title, meta, *depth, context),
        Line::Entry { row, depth } => entry_line(view, row, *depth, context),
        Line::Waiting { depth } => waiting_line(*depth, context),
    }))
    .width(Length::Fill)
    .into()
}

/// An expanded folder whose listing is still running. On the same grid as a
/// row — it stands in the row's place and will give it up to the first record
/// to arrive; there is nothing to press in it.
fn waiting_line(depth: usize, context: Context<'_>) -> Element<Msg> {
    let indent = indent(depth);
    let cells = grid(
        |column| match column {
            Column::Name => text::<Msg>("loading…".to_string())
                .size(theme::TEXT_SMALL)
                .color(theme::INK_FAINT)
                .single_line()
                .into(),
            _ => theme::nothing(),
        },
        context.columns,
        indent,
        context.compact,
    )
    .height(Length::Fixed(theme::ROW_HEIGHT));

    column![
        gutters(cells),
        theme::hairline(theme::LINE_ROW),
    ]
    .width(Length::Fill)
    .key(format!("waiting:{}", depth))
    .into()
}

/// Group title: the same grid as a row, but there is nothing to press in it.
fn group_line(title: &str, meta: &str, depth: usize, context: Context<'_>) -> Element<Msg> {
    let indent = indent(depth);
    let cells = grid(
        |column| match column {
            Column::Icon => glyph(theme::glyph::FOLDER, theme::ACCENT),
            Column::Name => text::<Msg>(format::ellipsize(
                title,
                format::mono_fit(context.name_width - indent, theme::TEXT_LABEL),
            ))
            .size(theme::TEXT_LABEL)
            .color(theme::INK)
            .weight(FontWeight::WeightMedium)
            .single_line()
            .into(),
            // How many are inside is said where rows show their status: it is
            // about the group's contents, not about its size.
            Column::Status => text::<Msg>(meta.to_string())
                .size(theme::TEXT_SMALL)
                .color(theme::INK_DIM)
                .single_line()
                .into(),
            _ => theme::nothing(),
        },
        context.columns,
        indent,
        context.compact,
    )
    .height(Length::Fixed(theme::ROW_HEIGHT));

    column![
        gutters(cells).background(theme::SHELF),
        theme::hairline(theme::LINE_ROW),
    ]
    .width(Length::Fill)
    .key(format!("group:{}:{}", depth, title))
    .into()
}

fn entry_line(view: ViewId, row_data: &Row, depth: usize, context: Context<'_>) -> Element<Msg> {
    let indent = indent(depth);
    let cells = grid(
        |column| match column {
            Column::Check => check(view, row_data, context),
            Column::Twist => twist(view, row_data, context),
            // The color says what the record is, not how it is stored: a
            // snapshot as one object and a snapshot as a catalogue are the same
            // snapshot.
            Column::Icon => glyph(
                row_glyph(row_data),
                match row_data.kind {
                    RowKind::File => theme::INK_FAINT,
                    RowKind::Folder | RowKind::Product { .. } => theme::ACCENT,
                },
            ),
            // Name in monospace: the column is measured in characters, and in
            // a proportional font their widths differ. A folder uses regular
            // type: it is a caption, not a value.
            Column::Name => {
                let title = format::distinct(
                    &row_data.title,
                    format::mono_fit(context.name_width - indent, theme::TEXT_MONO),
                    context.shared,
                );
                match row_data.kind.is_folder() {
                    true => text::<Msg>(title)
                        .size(theme::TEXT_LABEL)
                        .color(theme::INK)
                        .weight(FontWeight::WeightMedium)
                        .single_line()
                        .into(),
                    false => mono::<Msg>(title).size(theme::TEXT_MONO).color(theme::INK_SOFT).into(),
                }
            }
            // Monospace and truncated in place: this is a code, not a word,
            // and it must not be cut silently — the cell clips at the boundary,
            // and the value would butt against the neighboring column without a
            // single sign that it is incomplete.
            Column::Format => mono::<Msg>(format::ellipsize(
                &row_data.format(),
                format::mono_fit(FORMAT - CELL_PADDING * 2.0, theme::TEXT_TAG),
            ))
            .size(theme::TEXT_TAG)
            .color(theme::INK_FAINT)
            .single_line()
            .into(),
            Column::Date => text::<Msg>(format::date(row_data.date, context.now))
                .size(theme::TEXT_SMALL)
                .color(theme::INK_DIM)
                .single_line()
                .into(),
            Column::Size => container(
                mono::<Msg>(match row_data.size > 0 {
                    true => format::bytes(row_data.size),
                    false => String::new(),
                })
                .size(theme::TEXT_SMALL)
                .color(theme::INK_MUTED),
            )
            .width(Length::Fill)
            .align_x(Alignment::End)
            .into(),
            Column::Status => {
                let numbers = !context.columns.contains(&Column::Progress);
                let (dot, label, label_color) = status_look(&row_data.status, numbers);
                let cell = row![
                    theme::dot(dot),
                    text::<Msg>(label).size(theme::TEXT_SMALL).color(label_color).single_line(),
                ]
                .spacing(6.0)
                .align_items(Alignment::Center);
                // The failure reason is longer than the column, and cut
                // mid-sentence it explains nothing. The tooltip shows it whole
                // — the only place in the row whose width is not set by a
                // column.
                match &row_data.status {
                    RowStatus::Paused { trouble, .. } | RowStatus::Partial { trouble, .. }
                        if !trouble.is_empty() => tooltip(
                        cell,
                        // Trim it: the reason is written not by the layout but
                        // by whoever refused, and nothing limits its length —
                        // while the tooltip goes on one line and will not fit
                        // wider than the window.
                        format::ellipsize(trouble, 90),
                        TooltipPosition::TooltipTop,
                    )
                    .style(theme::panel())
                    .text_size(theme::TEXT_SMALL)
                    .into(),
                    _ => cell.into(),
                }
            }
            Column::Progress => progress_cell(row_data),
            Column::Actions => actions(view, row_data, context),
        },
        context.columns,
        indent,
        context.compact,
    );

    let onto_globe = row_data.globe.pace();
    let strip = strip_shown(row_data, context.columns);
    let height = theme::ROW_HEIGHT - if strip { ONTO_GLOBE } else { 0.0 };
    let cells = cells.height(Length::Fixed(height));

    // The whole row is a button: pressing it does what its main button does,
    // and for the one whose main business is to expand, that is what it does.
    // The separate button meanwhile stays where it is: it shows what exactly
    // will happen.
    let key = row_data.key();
    let press = primary(view, row_data).map(|action| action.message).or_else(|| {
        row_data
            .expandable()
            .then(|| Msg::In(view, ViewMsg::Expand(key.to_string())))
    });
    let line = match press {
        Some(message) => theme::row_button(cells, tint(row_data, context)).on_press(message),
        None => theme::row_button(cells, tint(row_data, context)),
    };

    let mut lines: Vec<Element<Msg>> =
        vec![line.width(Length::Fill).height(Length::Fixed(height)).into()];
    // There is no share yet, but the work is going — the bar fills completely,
    // but at half strength: an empty three-point track here says nothing, and
    // it is exactly the longest thing that happens to a snapshot that starts
    // this way — reading the raster over the network, tens of seconds.
    let bar = match onto_globe {
        Pace::Share(share) => Some((theme::ACCENT, share)),
        Pace::Unknown => Some((theme::ACCENT_HALF, 1.0)),
        // The fetching is over, but the room stays with it: empty, so the row
        // does not jump. That the snapshot is on the globe is told by the lit
        // glyph.
        Pace::Idle => None,
    };
    if strip {
        lines.push(match bar {
            Some((color, share)) => {
                // The bar has its own caption, not only the glyph at the
                // right edge of the row: it is the bar one looks at, and
                // without a caption it is a share of unknown what.
                tooltip(
                    progress_bar::<Msg>(0.0..=1.0, share)
                        .style(theme::progress(color))
                        .width(Length::Fill)
                        .height(Length::Fixed(ONTO_GLOBE)),
                    match row_data.globe.said() {
                        Some(said) => format!("Building the pyramid — {}", said),
                        None => "Placing on the globe".to_string(),
                    },
                    TooltipPosition::TooltipTop,
                )
                .style(theme::panel())
                .text_size(theme::TEXT_SMALL)
                .padding(6.0)
                .into()
            }
            None => veld_ui_service_wrap::space::<Msg>(Length::Fill, Length::Fixed(ONTO_GLOBE)).into(),
        });
    }
    lines.push(theme::hairline(theme::LINE_ROW));

    column(lines).width(Length::Fill).key(row_data.key().to_string()).into()
}

/// Whether the globe placement progress bar is drawn under the row.
///
/// A fallback path, not a second voice: where the LOADING column is shown, it
/// is the one that speaks of the work — with words, numbers, and a bar at once
/// — and a second bar under the same row would repeat it. Where the column is
/// absent (a narrow panel — the very one with the globe beside it), the bar is
/// the only thing visible without hovering the cursor.
///
/// The bar belongs to the snapshot, not to everyone who knows about it: a file
/// inside the snapshot answers for the globe too (see `rows::from_key`), but it
/// was the snapshot's glyph that was pressed, and it must be visible there.
///
/// The room under the bar is taken while the snapshot is on the globe — not
/// while work is going on it. Otherwise the row jumps by three points exactly
/// at the moment the fetching ends, that is, on every layer and before one's
/// eyes. The height is taken, not added: the row step is computed in one place
/// (`theme::ROW_PITCH`), the list scrolls to a row by it too, and it cannot
/// grow.
fn strip_shown(row: &Row, columns: &[Column]) -> bool {
    row.globe.any() && row.is_snapshot() && !columns.contains(&Column::Progress)
}

/// How the row stands out among its neighbors. The precedence is named in the
/// role itself (see [`theme::RowTint`]): the highlight is one for the whole
/// screen and outranks the marks, of which there may be fifty in a listing.
///
/// The selection is read here by the same measure as in the box (`check`) — by
/// the same key and the same set: two expressions of this question would one
/// day drift apart, and a filled row would stand with an empty box.
fn tint(row: &Row, context: Context<'_>) -> theme::RowTint {
    let picked = (!context.picked.is_empty() && row.snapshot_key() == context.picked)
        || row.named(context.target);
    if picked {
        return theme::RowTint::Picked;
    }
    let marked = context.listing.selected.contains_key(row.choice_key());
    match marked {
        true => theme::RowTint::Marked,
        false => theme::RowTint::Plain,
    }
}

/// How far the grouping tier is shifted. Computed from the depth here and only
/// here: two know the shift — the grid (the first column is wider by it) and
/// the name clip (the name column is narrower by the same), and they must not
/// drift apart.
fn indent(depth: usize) -> f32 {
    depth as f32 * INDENT
}

/// Contents of the first column. The grouping step is not part of it: the grid
/// sets it through the column width, otherwise the glyph butts against its edge
/// (see `grid`).
fn glyph(glyph: &str, color: Color) -> Element<Msg> {
    theme::row_glyph::<Msg>(glyph, color)
}

/// Row glyph — by kind, not by extension. A snapshot is marked by the
/// satellite even when it lies as a folder: a folder here is a way of storage,
/// not what the record is. The extension is asked last, from the one there is
/// nothing more to say about.
fn row_glyph(row: &Row) -> &'static str {
    match row.kind {
        RowKind::Product { .. } => theme::glyph::SATELLITE,
        RowKind::Folder => theme::glyph::FOLDER,
        RowKind::File if IMAGE_FORMATS.contains(&row.format().to_lowercase().as_str()) => {
            theme::glyph::IMAGE
        }
        RowKind::File => theme::glyph::FILE,
    }
}

/// Circle, caption, and its color — three ways to say the same state.
///
/// `numbers` — whether to speak in numbers. Numbers belong to the LOADING
/// column, and while it is shown, a word stands here: two neighboring cells
/// with the same fraction do not tell twice as much. The column is gone — the
/// numbers return here, because otherwise in a narrow panel there would be no
/// sign of the download progress left: the bar under the row answers only for
/// snapshots on the globe.
fn status_look(status: &RowStatus, numbers: bool) -> (Color, String, Color) {
    match status {
        RowStatus::Complete => (theme::ACCENT, "on disk".to_string(), theme::ACCENT_TEXT),
        RowStatus::Remote => (theme::LINE, "in storage".to_string(), theme::INK_DIM),
        RowStatus::Downloading { done, total } => (
            theme::ACCENT,
            match (numbers, *total > 0) {
                (true, true) => format::progress(*done, *total),
                _ => "downloading".to_string(),
            },
            theme::ACCENT_TEXT,
        ),
        // A failed download speaks in its own voice. "Interrupted" below is
        // everything else: both stopped by a human and the one whose reason did
        // not survive a restart; staying silent about a reason where there is
        // one means offering to press "Resume" blindly.
        RowStatus::Paused { trouble, .. } if !trouble.is_empty() => (
            theme::DANGER,
            trouble.clone(),
            theme::DANGER_TEXT,
        ),
        RowStatus::Paused { done, total, .. } => (
            theme::WARN,
            match (numbers, *total > 0) {
                (true, true) => format::progress(*done, *total),
                _ => "interrupted".to_string(),
            },
            theme::WARN_TEXT,
        ),
        // The refusal outweighs the count: how much is downloaded is visible
        // from the size too, while "3 on disk" in green is a calm caption
        // hiding what stands behind it.
        RowStatus::Partial { trouble, .. } if !trouble.is_empty() => (
            theme::DANGER,
            trouble.clone(),
            theme::DANGER_TEXT,
        ),
        RowStatus::Partial { done, .. } => (theme::ACCENT, format!("{} on disk", done), theme::ACCENT_TEXT),
    }
}

/// What is being done with the row right now — everything that shows it.
///
/// One cell for two kinds of work, because the viewer has one question: "is
/// anything going and how much is left". Two kinds in two columns would make
/// them look for the answer in two places, one of which is always empty.
struct Load {
    /// Caption parts from senior to junior: when shrinking, the cell drops
    /// them from the tail (see [`fit_label`]).
    parts: Vec<String>,
    /// Share, 0..1; `None` — work is going, but it has no share.
    share: Option<f32>,
    color: Color,
    /// Tooltip: the same work, named fully and in words. It has room for
    /// everything the cell did not fit.
    hint: String,
}

/// What the row is busy with. `None` — nothing: the column has nothing to say
/// in this row.
///
/// The precedence is single and it is not about convenience: downloading to
/// disk changes what lies on the machine, while the network view is derived
/// and repeatable — and about it, besides this cell, the lit globe glyph and
/// the "Being viewed" list speak.
///
/// The pyramid belongs to the snapshot, not to everyone who knows about it: a
/// file inside the snapshot answers for the globe too (see `rows::from_key`),
/// but it was the snapshot's glyph that was pressed, and it must be visible
/// there.
fn load_of(row: &Row) -> Option<Load> {
    match &row.status {
        RowStatus::Downloading { done, total } => Some(Load {
            parts: match *total > 0 {
                true => vec![format::progress(*done, *total)],
                false => vec!["downloading".to_string()],
            },
            share: (*total > 0).then(|| *done as f32 / *total as f32),
            color: theme::ACCENT,
            hint: match *total > 0 {
                true => format!(
                    "Downloading to disk — {}, {} left",
                    format::progress(*done, *total),
                    format::bytes(total.saturating_sub(*done)),
                ),
                false => "Downloading to disk".to_string(),
            },
        }),
        // A standing download also answers "how much is left": it will be
        // continued from this place. The reason is told by the neighboring
        // cell, here only the room.
        RowStatus::Paused { done, total, .. } if *total > 0 => Some(Load {
            parts: vec![format::progress(*done, *total)],
            share: Some(*done as f32 / *total as f32),
            color: theme::WARN,
            hint: format!(
                "Download is standing — {}, {} left",
                format::progress(*done, *total),
                format::bytes(total.saturating_sub(*done)),
            ),
        }),
        _ => None,
    }
    .or_else(|| {
        if !row.is_snapshot() {
            return None;
        }
        let parts = row.globe.parts();
        if parts.is_empty() {
            return None;
        }
        Some(Load {
            hint: format!("Building the pyramid — {}", parts.join(" · ")),
            parts,
            // There is no share yet, but the work is going — the bar fills
            // completely, but at half strength: an empty track here says
            // nothing, and it is exactly the longest thing that happens to a
            // snapshot that starts this way — reading the raster over the
            // network, tens of seconds.
            share: match row.globe.pace() {
                Pace::Share(share) => Some(share),
                Pace::Unknown | Pace::Idle => None,
            },
            color: match row.globe.pace() {
                Pace::Share(_) => theme::ACCENT,
                Pace::Unknown | Pace::Idle => theme::ACCENT_HALF,
            },
        })
    })
}

/// A caption shrunk to fit the width: parts are dropped from the tail until it
/// fits.
///
/// From the tail because the order of the parts is their precedence: the step
/// names what the bar measures, the bytes are the only thing moving in a long
/// read, and the tile count is smaller than both. Not even one fits — we give
/// it as is: cutting it with an ellipsis would show "ste…", from which nothing
/// follows, while the cell will clip the excess itself (see `grid`).
fn fit_label(parts: &[String], width: f32) -> String {
    for take in (1..=parts.len()).rev() {
        let said = parts[..take].join(" · ");
        if format::text_width(&said, theme::TEXT_TAG) <= width {
            return said;
        }
    }
    parts.first().cloned().unwrap_or_default()
}

fn progress_cell(row: &Row) -> Element<Msg> {
    let Some(load) = load_of(row) else { return theme::nothing() };
    let said = fit_label(&load.parts, PROGRESS - CELL_PADDING * 2.0);
    let (fill, color) = match load.share {
        Some(share) => (share, load.color),
        None => (1.0, theme::ACCENT_HALF),
    };
    tooltip(
        column![
            text::<Msg>(said).size(theme::TEXT_TAG).color(theme::INK_DIM).single_line(),
            progress_bar::<Msg>(0.0..=1.0, fill)
                .style(theme::progress(color))
                .width(Length::Fill)
                .height(Length::Fixed(LOAD_BAR)),
        ]
        .spacing(3.0)
        .width(Length::Fill),
        load.hint,
        TooltipPosition::TooltipTop,
    )
    .style(theme::panel())
    .text_size(theme::TEXT_SMALL)
    .padding(6.0)
    .into()
}

/// The row's main action: what will happen when pressing it.
struct Primary {
    glyph: &'static str,
    /// Tooltip caption — it also says what exactly will happen.
    hint: &'static str,
    message: Msg,
    /// The action leads inside — into another folder or another listing. It
    /// does not stand as a separate glyph: pressing the row does exactly the
    /// same, and an arrow beside it would repeat it (see `quick`).
    transition: bool,
}

impl Primary {
    fn new(glyph: &'static str, hint: &'static str, message: Msg) -> Self {
        Self { glyph, hint, message, transition: false }
    }

    fn transition(glyph: &'static str, hint: &'static str, message: Msg) -> Self {
        Self { glyph, hint, message, transition: true }
    }
}

/// The row's main button. `None` — there is nothing to do with the row (for
/// example, the provider key is unknown).
fn primary(view: ViewId, row: &Row) -> Option<Primary> {
    // A row folded from records is not a record itself: its files are what to
    // download and open, while its key is the snapshot's path in storage, and
    // sending it to the download would mean asking to download a folder as one
    // object. Its own action is to expand (see `twist`).
    if row.folded {
        return None;
    }
    // An action without a key has no one to address — better not to offer it at
    // all than to send a knowingly empty request. Hence the `?` on each: what
    // to address this action with is known earlier than the action is built.
    let remote = || (!row.identifier.is_empty()).then(|| row.identifier.clone());
    let local = || (!row.name.is_empty()).then(|| row.name.clone());

    Some(match &row.status {
        // The folder is asked first, whatever it is busy with: its key is a
        // common prefix, not an object, and sending it to the download would
        // mean asking to download the folder as one piece. It has its own pause
        // and resume as menu items — there they are named for the snapshot, not
        // the file.
        _ if row.kind.is_folder() => Primary::transition(
            theme::glyph::ENTER,
            "Enter",
            Msg::In(view, ViewMsg::Enter(remote()?)),
        ),
        RowStatus::Downloading { .. } => {
            Primary::new(theme::glyph::PAUSE, "Pause", Msg::Download(remote()?, row.product.clone()))
        }
        RowStatus::Paused { .. } => Primary::new(
            theme::glyph::PLAY,
            "Resume",
            Msg::Download(remote()?, row.product.clone()),
        ),
        RowStatus::Remote => Primary::new(
            theme::glyph::DOWNLOAD,
            "Download",
            Msg::Download(remote()?, row.product.clone()),
        ),
        RowStatus::Complete => {
            Primary::new(theme::glyph::EYE, "Open", Msg::In(view, ViewMsg::Preview(local()?)))
        }
        // About a folder that is partly downloaded the main button cannot say
        // anything: "download the rest" the library cannot do, and "open" there
        // is nothing to open.
        RowStatus::Partial { .. } => return None,
    })
}

/// Buttons on the right: quick glyphs and the menu of everything else.
fn actions(view: ViewId, entry: &Row, context: Context<'_>) -> Element<Msg> {
    // In tight space the glyphs give up their place to the name: a row without
    // a name is not a row. They do not go nowhere — the same action becomes a
    // menu item, and the glyph and the item are one and the same here, the
    // only difference being whether there is room.
    let shown = match context.compact {
        true => Vec::new(),
        false => quick(view, entry),
    };
    let mut buttons: Vec<Element<Msg>> = shown
        .into_iter()
        .map(|Quick { glyph, hint, message, tone, .. }| match message {
            Some(message) => icon_button(glyph, tone, &hint, message),
            None => hinted(theme::row_button_icon(glyph, tone), &hint),
        })
        .collect();

    let mut items = menu_items(view, entry, context.here);
    if context.compact {
        // As items — ahead of the rest: these are the row's main actions, they
        // stood as glyphs. A disabled glyph does not become an item: a glyph at
        // least keeps its place in the row and is explained by the tooltip,
        // while a menu item that does nothing does not do even that.
        let moved: Vec<super::menu::Item> = quick(view, entry)
            .into_iter()
            .filter_map(|Quick { glyph, label, message, .. }| {
                Some(super::menu::Item::new(label, message?).glyph(glyph))
            })
            .collect();
        items.splice(0..0, moved);
    }
    if !items.is_empty() {
        let menu = Menu::Row(entry.key().to_string());
        let open = context.menu == Some(&menu);
        let raised = match open {
            true => theme::IconTone::Raised,
            false => theme::IconTone::Rest,
        };
        let anchor = theme::row_button_icon(theme::glyph::MORE, raised)
            .on_press(Msg::In(view, ViewMsg::OpenMenu(Some(menu))));

        buttons.push(
            popover(anchor, open, || super::menu::panel(items))
                .align_x(Alignment::End)
                .gap(4.0)
                .on_dismiss(Msg::In(view, ViewMsg::OpenMenu(None)))
                .into(),
        );
    }

    container(row(buttons).spacing(BUTTON_GAP).align_items(Alignment::Center))
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(Alignment::End)
        .align_y(Alignment::Center)
        .into()
}

/// A quick glyph of the row: how it is drawn, what the tooltip will say, what
/// it will send, and with what face it stands.
struct Quick {
    glyph: &'static str,
    hint: String,
    /// What it is called as a menu item, where it moves in tight space.
    /// Separate from the tooltip: the tooltip also tells the state ("asking the
    /// catalogue…"), while a menu item must name the action — a state in the
    /// item list does not read and offers no one knows what. A constructor
    /// argument, not a default: a forgotten caption would give an unnamed item,
    /// and the compiler would stay silent.
    label: &'static str,
    /// What the press will send. `None` — there is nothing to send: the glyph
    /// stands disabled and does not move into the tight-space menu. That is how
    /// the aiming glyph stands for a snapshot that is not on the globe at all:
    /// it cannot vanish — the neighboring glyphs would slide into its place on
    /// every press — but it has no right to promise a press.
    message: Option<Msg>,
    /// With what face it stands. The default is rest: the outline and globe
    /// glyphs have state, while aiming has only "there is somewhere to aim" and
    /// "there is nowhere".
    tone: theme::IconTone,
}

impl Quick {
    fn new(glyph: &'static str, label: &'static str, hint: String, message: Msg) -> Self {
        Self { glyph, hint, label, message: Some(message), tone: theme::IconTone::Rest }
    }

    /// Glyph without a press: there is nothing to do, but its place stays.
    fn idle(glyph: &'static str, label: &'static str, hint: String) -> Self {
        Self { glyph, hint, label, message: None, tone: theme::IconTone::Idle }
    }

    fn tone(self, tone: theme::IconTone) -> Self {
        Self { tone, ..self }
    }
}

/// The row's quick glyphs — only those three about the globe: outline,
/// raster, and aiming.
///
/// The first two are toggles, and their mechanics is one: press — the snapshot
/// is on the globe, press again — it is taken off. There cannot be two rules
/// here: their question is one ("is this snapshot on the globe"), the
/// difference is only in the detail of how it lies there. Neither moves the
/// camera — that is a third intention, and it stands as the third glyph:
/// "show here" and "take me there" merged into one press would take the answer
/// away from each other.
///
/// Everything else — download, pause, open, view — stands as menu items (see
/// [`menu_items`]). The row is narrow, the panel is shared in half, and a glyph
/// beyond these would take room from the name; while each of the three tells
/// its state with color, which a menu item cannot do.
///
/// There is no transition deeper among them: pressing the row itself leads
/// inside, and an arrow beside it would say the same a second time.
///
/// All of them only for a snapshot: neither an outline nor a raster can belong
/// to a path folder or to a file inside the snapshot, and a button with nothing
/// to do lies about what the row can.
fn quick(view: ViewId, row: &Row) -> Vec<Quick> {
    let mut quick = Vec::new();
    let key = row.snapshot_key().to_string();
    if !row.is_snapshot() || key.is_empty() {
        return quick;
    }

    // The outline is the row's own state, not the selection and not the show
    // (see handlers::outline). It burns green when drawn; at half strength —
    // while the geometry is on its way and when there turned out to be none.
    //
    // The caption names what will happen on the press, while the state is
    // explained after the dash: for the unrequested one a press re-asks, it
    // does not lift the request (see `outline::toggle_outline`).
    let (tone, hint) = match row.outlined {
        OnOutline::Off => (theme::IconTone::Rest, "Outline on the globe"),
        OnOutline::Asking => (theme::IconTone::Half, "Remove the outline — asking the catalogue…"),
        OnOutline::Blank => {
            (theme::IconTone::Half, "Remove the outline — the snapshot has no geometry")
        }
        OnOutline::Failed => (theme::IconTone::Half, "Ask for the outline again — it did not come"),
        OnOutline::Drawn => (theme::IconTone::Lit, "Remove the outline from the globe"),
    };
    quick.push(
        Quick::new(
            theme::glyph::OUTLINE,
            "Outline on the globe",
            hint.to_string(),
            Msg::OutlineToggle(key.clone()),
        )
        .tone(tone),
    );

    // An empty reason is not "it will surely show", but "it makes sense to
    // offer" (see `Row::unviewable`): the glyph over level-0 raw data or over
    // an archive promises what does not happen. A named reason dims the glyph
    // instead of removing it: the place stays, and the tooltip says exactly
    // what is impossible — the same way as the aiming below. In tight space
    // this is not seen: a disabled glyph does not become a menu item (see
    // `actions`).
    if row.unviewable.is_empty() {
        // The glyph burns when the snapshot lies on the globe as a raster. The
        // caption names what will happen on the press: for a lying one it is
        // removal, not a second overlay. A hidden one is taken off on par with
        // a visible one — it remains a layer, and its own eye in the "Being
        // viewed" list hides it.
        let (tone, hint) = match row.globe {
            OnGlobe::Off => (theme::IconTone::Rest, "Show on the globe"),
            OnGlobe::Asked => (theme::IconTone::Half, "Cancel the show — asking the catalogue…"),
            OnGlobe::Assembling => (theme::IconTone::Half, "Cancel the show — placing on the globe…"),
            OnGlobe::Laid { hidden: true, .. } => {
                (theme::IconTone::Half, "Take off the globe — it lies hidden now")
            }
            OnGlobe::Laid { hidden: false, .. } => (theme::IconTone::Lit, "Take off the globe"),
        };
        // The fetching progress is not named here: the glyph answers "what will
        // happen if pressed", while "what is happening now" stands in the same
        // row as the LOADING cell or, where there is none, as the tooltip of
        // the bar under the row.
        //
        // But a past failure is named, and it outranks everything else: waiting
        // for it took tens of seconds, and silently inviting another press
        // would mean offering to wait them again. The glyph stays pressable
        // meanwhile — the attempt could have failed over the network too (see
        // `Row::unshowable`).
        let (tone, hint) = match (row.unshowable.is_empty(), row.globe) {
            (false, OnGlobe::Off) => (
                theme::IconTone::Warn,
                format!("Showing failed — {}. Press to try again", row.unshowable),
            ),
            _ => (tone, hint.to_string()),
        };
        quick.push(
            Quick::new(
                theme::glyph::GLOBE,
                "Show on the globe",
                hint,
                Msg::In(view, ViewMsg::GlobeToggle(key.clone())),
            )
            .tone(tone),
        );
    } else {
        quick.push(Quick::idle(
            theme::glyph::GLOBE,
            "Show on the globe",
            format!("Cannot put on the globe — {}", row.unviewable),
        ));
    }

    // Aiming. Its own press, not an appendage to the show: otherwise one could
    // not put a second snapshot beside the first without flying away to it.
    // Aiming is possible only at what has a place on the globe — a drawn
    // outline or an established layer. One show request is not enough: until
    // the catalogue answers, not even where the snapshot is is known, and the
    // press would lead the eye to a globe where nothing has changed.
    let aimable = matches!(row.outlined, OnOutline::Drawn) || row.globe.laid();
    quick.push(match aimable {
        true => Quick::new(
            theme::glyph::FOCUS,
            "Aim the camera",
            "Aim the camera and highlight".to_string(),
            Msg::OutlineFocus(key),
        ),
        false => Quick::idle(
            theme::glyph::FOCUS,
            "Aim the camera",
            "Nothing to aim at: the snapshot is not on the globe".to_string(),
        ),
    });
    quick
}

/// The expansion triangle. For a row with nothing to show it is empty: its
/// place stays, so the neighbors do not shift when the listing has both.
///
/// The folder's contents may not have been requested yet at this point — the
/// expansion is what requests them (see handlers::browse::request_children),
/// so the triangle stands on every folder, not only a filled one.
fn twist(view: ViewId, row: &Row, context: Context<'_>) -> Element<Msg> {
    if !row.expandable() {
        return theme::nothing();
    }
    let open = context.listing.expanded.contains(row.key());
    theme::chrome_icon(
        icon::<Msg>(if open { theme::glyph::CARET } else { theme::glyph::CARET_RIGHT })
            .size(10.0)
            .color(theme::INK_DIM),
    )
    // The padding is narrower than usual — for the same reason as the check
    // box.
    .padding(1.0)
    .width(Length::Fill)
    .height(Length::Fixed(theme::ROW_BUTTON))
    .on_press(Msg::In(view, ViewMsg::Expand(row.key().to_string())))
    .into()
}

/// The selection box.
///
/// Selection is a set of rows for batch actions: the selected are deleted and
/// downloaded (see the listing header). It does not touch the globe at all —
/// neither the outline nor the show: those have their own glyphs and their own
/// states (see handlers::outline).
///
/// Only a path folder lacks it: one enters a folder, while what is selected
/// lies in the catalogue or on disk. A snapshot stored as a catalogue (.SAFE,
/// .SEN3) is also a "folder" in storage, but it is exactly the one being
/// selected.
///
/// It is disabled on a row without a key, and such a row has nowhere to come
/// from in the listing (see [`Row::key`]) — this box is a guard one: it must be
/// drawn anyway, otherwise the column breaks, and a lever that is visible and
/// does nothing lies exactly as its absence does. A disabled one tells the
/// truth and explains it with a tooltip.
///
/// With a tooltip, and this is not decoration: the selection acts not here but
/// with the buttons in the listing header, and without a caption the box
/// offers to "mark" for no one knows what.
fn check(view: ViewId, row: &Row, context: Context<'_>) -> Element<Msg> {
    if matches!(row.kind, RowKind::Folder) {
        return theme::nothing();
    }
    if !row.choosable() {
        return hinted(
            theme::row_check(None),
            "Nothing to select: the record has no key to address it by",
        );
    }
    let key = row.choice_key();
    let marked = context.listing.selected.contains_key(key);
    hinted(
        theme::row_check(Some(marked)).on_press(Msg::In(view, ViewMsg::Check(key.to_string()))),
        match marked {
            false => "Select",
            true => "Deselect",
        },
    )
}

/// A glyph-button: how it is drawn, with what face it stands, what the tooltip
/// will say, and what it will send. Rows of such buttons stand on the right in
/// a table row, in the layer list, and in the listing header — that is why they
/// are assembled here, not each place on its own: otherwise glyphs that look
/// the same drift apart in face and tooltip.
pub fn icon_button(
    glyph: &'static str,
    tone: theme::IconTone,
    hint: &str,
    message: Msg,
) -> Element<Msg> {
    hinted(theme::row_button_icon(glyph, tone).on_press(message), hint)
}

/// A button with a tooltip: the tooltip is the same for all row glyphs, and
/// written at every call site it would drift apart in padding and font size.
pub fn hinted(button: crate::proto::ui_service::Container<Msg>, hint: &str) -> Element<Msg> {
    tooltip(button, hint, TooltipPosition::TooltipTop)
        .style(theme::panel())
        .text_size(theme::TEXT_SMALL)
        .padding(6.0)
        .into()
}

/// Everything done with a row rarely. The order is common: first the
/// transitions, then the irreversible.
fn menu_items(view: ViewId, row: &Row, here: &str) -> Vec<super::menu::Item> {
    use super::menu::Item;
    let mut items = Vec::new();

    // The row's main action — download, pause, open. It does not stand as a
    // glyph: only those two that put the snapshot on the globe remain in the
    // row (see [`quick`]). The transition deeper does not go as an item for the
    // same reason it did not go as a glyph — pressing the row itself leads
    // inside.
    if let Some(action) = primary(view, row).filter(|action| !action.transition) {
        // With a glyph: this item is the main one, and its sign is the same one
        // the action is named by everywhere — otherwise the same thing would
        // read as different.
        items.push(Item::new(action.hint, action.message).glyph(action.glyph));
    }
    let key = row.snapshot_key().to_string();
    // A snapshot stored as a folder is downloaded whole: its files are laid out
    // in tiers, and downloading them one by one, walking the catalogue by hand,
    // is work the application can do itself (see
    // handlers::library::on_download_snapshot). There is no point in offering
    // it to a completed one: it is already all on disk.
    if row.kind.is_product()
        && row.kind.is_folder()
        && !key.is_empty()
        && !matches!(row.status, RowStatus::Complete)
    {
        // The weight is in the caption: one press queues the whole snapshot,
        // and that is gigabytes, and learning it from the download counter is
        // too late. The catalogue does not know the folder's size (in S3 there
        // is none behind a prefix), and then it is simply said that the
        // snapshot is downloaded whole.
        let label = match row.size > 0 {
            true => format!("Download the whole snapshot — {}", format::bytes(row.size)),
            false => "Download the whole snapshot".to_string(),
        };
        items.push(Item::new(label, Msg::DownloadSnapshot(key.clone())).glyph(theme::glyph::DOWNLOAD));
    }
    // View the snapshot — its own action, however the snapshot is stored: as a
    // folder of tiers or as a single file. The condition is the same as for the
    // globe glyph, and this is no coincidence — their question is one ("can
    // this be shown"), and two answers to it would drift apart silently.
    // Except when the main action is already "Open": it is the same eye and the
    // same meaning, and two such items in a row would read as two different
    // actions.
    let opens = matches!(row.status, RowStatus::Complete) && !row.kind.is_folder();
    if row.is_snapshot() && row.unviewable.is_empty() && !key.is_empty() && !opens {
        items.push(
            Item::new("View the snapshot", Msg::In(view, ViewMsg::PreviewProduct(key.clone())))
                .glyph(theme::glyph::EYE),
        );
    }
    // A file inside the snapshot puts the snapshot on the globe with itself: it
    // is laid as the layer's detailed raster, not the provider's choice (see
    // handlers::overlay::on_file_pressed). Whether it is a raster is decided by
    // the provider from the name and by the tiler from the contents;
    // `unviewable` has nothing to do with it here: for a file inside the
    // snapshot it says "the snapshot, not the file, is put on the globe" —
    // exactly what this item does differently.
    if !row.product.is_empty() && !row.is_snapshot() && !row.kind.is_folder() {
        items.push(
            Item::new("On the globe with this file", Msg::GlobeFile(row.product.clone(), row.identifier.clone()))
                .glyph(theme::glyph::GLOBE),
        );
    }
    // Everything with a provider key can be shown on the globe: for a found one
    // the product with its outline is already at hand, while a catalogue row or
    // a downloaded one has it restored by the provider from the key (see
    // handlers::overlay). For a snapshot this item stands as a glyph in the row
    // itself and is not repeated here.
    //
    // The snapshot is put on the globe, not the file: an individual file has
    // neither an outline nor a raster. Hence both the key and the caption: it
    // names what will happen on the press, and the row knows it for the same
    // reason the snapshot's glyph does — it answers for the globe with the
    // snapshot it belongs to (see `rows::from_key`).
    //
    // A path folder does not get this item: there is no snapshot behind it, and
    // the catalogue answers "no such product" to its name — offering this means
    // promising a refusal.
    if !row.identifier.is_empty() && !row.is_snapshot() && !matches!(row.kind, RowKind::Folder) {
        let label = match row.globe.any() {
            true => "Take the snapshot off the globe",
            false => "Show the snapshot on the globe",
        };
        items.push(Item::new(label, Msg::In(view, ViewMsg::GlobeToggle(row.product_key().to_string()))));
    }
    // There is no point in showing a folder that is already open: the item
    // would lead where the user already stands.
    if !row.folder().is_empty() && !crate::module::components::row::bare(here).ends_with(row.folder())
    {
        items.push(Item::new(
            "Show in the catalogue",
            Msg::In(view, ViewMsg::InCatalog(row.snapshot_key().to_string())),
        ));
    }
    if !row.kind.is_folder() && !row.identifier.is_empty() && matches!(row.status, RowStatus::Remote) {
        items.push(Item::new(
            "View without downloading",
            Msg::In(view, ViewMsg::PreviewRemote(row.identifier.clone())),
        ));
    }
    // A re-download wipes the existing file before it starts — otherwise a
    // second, half-downloaded one would lie beside it (see
    // data-library::download). Hence the mark. A folded row has nothing to
    // download: its key is the snapshot's path in storage, and sending it to
    // the download would mean asking to download a folder as one object. The
    // whole snapshot is downloaded by its own glyph (see `quick`).
    if !row.identifier.is_empty() && !row.folded && matches!(row.status, RowStatus::Complete) {
        items.push(
            Item::new("Download again", Msg::Download(row.identifier.clone(), row.product.clone()))
                .danger(),
        );
    }
    // Only for what is on disk: there is nothing to show in the file manager
    // for what lies in storage.
    if !row.name.is_empty() && !matches!(row.status, RowStatus::Remote) {
        items.push(Item::new(
            "Show in the file manager",
            Msg::Reveal(row.name.clone()),
        ));
    }
    // Abandoning the started and deleting the downloaded is one action: both
    // leave an empty place behind. Only the caption can differ, because what
    // will disappear is named differently.
    if !row.name.is_empty() && !matches!(row.status, RowStatus::Remote) {
        let label = match row.status {
            RowStatus::Downloading { .. } | RowStatus::Paused { .. } => "Cancel the download",
            _ => "Delete from disk",
        };
        items.push(Item::new(label, Msg::Delete(row.name.clone())).danger());
    }
    // Pause for the whole snapshot — for the same reason it is downloaded whole:
    // it has getting on for three dozen files, and pressing pause on each would
    // mean asking the user to do for us what we have composed ourselves. It is
    // offered only while there is something to stop.
    //
    // Whether a download runs over the snapshot, a folded row sees from its
    // files, while a catalogue row sees from itself: no files have arrived
    // under it, and it knows exactly what the library said about its prefix
    // (see `rows::from_key`).
    let downloading = match row.folded {
        true => row.children.iter().any(|file| matches!(file.status, RowStatus::Downloading { .. })),
        false => row.kind.is_folder() && matches!(row.status, RowStatus::Downloading { .. }),
    };
    if downloading && !row.product.is_empty() {
        items.push(Item::new("Pause the snapshot", Msg::PauseSnapshot(row.product.clone())));
    }
    // A folded row has no name of its own — it is not a record, but a snapshot.
    // It is thrown away whole: expanding it to delete files one by one would
    // mean asking the user to do for us what we have composed ourselves.
    if row.folded && !row.product.is_empty() {
        items.push(
            Item::new("Delete the snapshot from disk", Msg::DeleteSnapshot(row.product.clone())).danger(),
        );
    }

    items
}
