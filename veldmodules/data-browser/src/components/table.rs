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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Column {
    Check,
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

const DROP_ORDER: [Column; 6] =
    [Column::Date, Column::Size, Column::Progress, Column::Status, Column::Format, Column::Icon];

const CHECK: f32 = 20.0;
const TWIST: f32 = 14.0;
const ICON: f32 = 34.0;
const FORMAT: f32 = 80.0;
const DATE: f32 = 88.0;
const SIZE: f32 = 84.0;
const STATUS: f32 = 104.0;
const PROGRESS: f32 = 148.0;
const ACTION_BUTTONS: f32 = 4.0;

const ACTIONS: f32 =
    theme::ROW_BUTTON * ACTION_BUTTONS + BUTTON_GAP * (ACTION_BUTTONS - 1.0) + CELL_PADDING * 2.0;

const ACTIONS_COMPACT: f32 = theme::ROW_BUTTON + CELL_PADDING * 2.0;

const NAME_FLOOR: f32 = 64.0;

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

const NAME_MIN: f32 = 150.0;

pub const BUTTON_GAP: f32 = 6.0;

const CELL_PADDING: f32 = 8.0;

#[derive(Clone, Copy, Default)]
pub struct Optional {
    pub twisty: bool,
    pub checkable: bool,
}

pub fn fit(width: f32, optional: Optional) -> Fit {
    let mut columns: Vec<Column> = COLUMNS.to_vec();
    if !optional.checkable {
        columns.retain(|column| *column != Column::Check);
    }
    if !optional.twisty {
        columns.retain(|column| *column != Column::Twist);
    }
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
    let compact = width - taken(&columns, false) < NAME_FLOOR;
    let name = (width - taken(&columns, compact)).max(0.0);
    Fit { columns, name, compact }
}

pub struct Fit {
    pub columns: Vec<Column>,
    pub name: f32,
    pub compact: bool,
}

const INDENT: f32 = 12.0;

const HEADER_HEIGHT: f32 = 22.0;

const ONTO_GLOBE: f32 = 3.0;

const LOAD_BAR: f32 = 4.0;

const IMAGE_FORMATS: [&str; 7] = ["png", "jpg", "jpeg", "tif", "tiff", "jp2", "webp"];

fn grid(
    cells: impl Fn(Column) -> Element<Msg>,
    columns: &[Column],
    indent: f32,
    compact: bool,
) -> RowBuilder<Msg> {
    let first = columns.first().copied();
    row(columns.iter().map(|column| {
        let step = if Some(*column) == first { indent } else { 0.0 };
        let pad = padding_of(*column);
        let (width, left) = match (column, compact) {
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

fn padding_of(column: Column) -> f32 {
    match column {
        Column::Check | Column::Twist => 0.0,
        _ => CELL_PADDING,
    }
}

fn gutters<M>(content: impl Into<Element<M>>) -> Container<M> {
    container(content)
        .width(Length::Fill)
        .padding(Padding { top: 0.0, bottom: 0.0, left: theme::GUTTER, right: theme::GUTTER })
}

#[derive(Clone, Copy)]
pub struct Context<'a> {
    pub listing: &'a ListingState,
    pub menu: Option<&'a Menu>,
    pub columns: &'a [Column],
    pub name_width: f32,
    pub compact: bool,
    pub shared: (usize, usize),
    pub here: &'a str,
    pub picked: &'a str,
    pub target: &'a str,
    pub now: i64,
}

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

pub fn body(view: ViewId, lines: &[Line<'_>], context: Context<'_>) -> Element<Msg> {
    column(lines.iter().map(|line| match line {
        Line::Group { title, meta, depth } => group_line(title, meta, *depth, context),
        Line::Entry { row, depth } => entry_line(view, row, *depth, context),
        Line::Waiting { depth } => waiting_line(*depth, context),
    }))
    .width(Length::Fill)
    .into()
}

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
            Column::Icon => glyph(
                row_glyph(row_data),
                match row_data.kind {
                    RowKind::File => theme::INK_FAINT,
                    RowKind::Folder | RowKind::Product { .. } => theme::ACCENT,
                },
            ),
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
                match &row_data.status {
                    RowStatus::Paused { trouble, .. } | RowStatus::Partial { trouble, .. }
                        if !trouble.is_empty() => tooltip(
                        cell,
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
    let bar = match onto_globe {
        Pace::Share(share) => Some((theme::ACCENT, share)),
        Pace::Unknown => Some((theme::ACCENT_HALF, 1.0)),
        Pace::Idle => None,
    };
    if strip {
        lines.push(match bar {
            Some((color, share)) => {
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

fn strip_shown(row: &Row, columns: &[Column]) -> bool {
    row.globe.any() && row.is_snapshot() && !columns.contains(&Column::Progress)
}

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

fn indent(depth: usize) -> f32 {
    depth as f32 * INDENT
}

fn glyph(glyph: &str, color: Color) -> Element<Msg> {
    theme::row_glyph::<Msg>(glyph, color)
}

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
        RowStatus::Partial { trouble, .. } if !trouble.is_empty() => (
            theme::DANGER,
            trouble.clone(),
            theme::DANGER_TEXT,
        ),
        RowStatus::Partial { done, .. } => (theme::ACCENT, format!("{} on disk", done), theme::ACCENT_TEXT),
    }
}

struct Load {
    parts: Vec<String>,
    share: Option<f32>,
    color: Color,
    hint: String,
}

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

struct Primary {
    glyph: &'static str,
    hint: &'static str,
    message: Msg,
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

fn primary(view: ViewId, row: &Row) -> Option<Primary> {
    if row.folded {
        return None;
    }
    let remote = || (!row.identifier.is_empty()).then(|| row.identifier.clone());
    let local = || (!row.name.is_empty()).then(|| row.name.clone());

    Some(match &row.status {
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
        RowStatus::Partial { .. } => return None,
    })
}

fn actions(view: ViewId, entry: &Row, context: Context<'_>) -> Element<Msg> {
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

struct Quick {
    glyph: &'static str,
    hint: String,
    label: &'static str,
    message: Option<Msg>,
    tone: theme::IconTone,
}

impl Quick {
    fn new(glyph: &'static str, label: &'static str, hint: String, message: Msg) -> Self {
        Self { glyph, hint, label, message: Some(message), tone: theme::IconTone::Rest }
    }

    fn idle(glyph: &'static str, label: &'static str, hint: String) -> Self {
        Self { glyph, hint, label, message: None, tone: theme::IconTone::Idle }
    }

    fn tone(self, tone: theme::IconTone) -> Self {
        Self { tone, ..self }
    }
}

fn quick(view: ViewId, row: &Row) -> Vec<Quick> {
    let mut quick = Vec::new();
    let key = row.snapshot_key().to_string();
    if !row.is_snapshot() || key.is_empty() {
        return quick;
    }

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

    if row.unviewable.is_empty() {
        let (tone, hint) = match row.globe {
            OnGlobe::Off => (theme::IconTone::Rest, "Show on the globe"),
            OnGlobe::Asked => (theme::IconTone::Half, "Cancel the show — asking the catalogue…"),
            OnGlobe::Assembling => (theme::IconTone::Half, "Cancel the show — placing on the globe…"),
            OnGlobe::Laid { hidden: true, .. } => {
                (theme::IconTone::Half, "Take off the globe — it lies hidden now")
            }
            OnGlobe::Laid { hidden: false, .. } => (theme::IconTone::Lit, "Take off the globe"),
        };
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
    .padding(1.0)
    .width(Length::Fill)
    .height(Length::Fixed(theme::ROW_BUTTON))
    .on_press(Msg::In(view, ViewMsg::Expand(row.key().to_string())))
    .into()
}

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

pub fn icon_button(
    glyph: &'static str,
    tone: theme::IconTone,
    hint: &str,
    message: Msg,
) -> Element<Msg> {
    hinted(theme::row_button_icon(glyph, tone).on_press(message), hint)
}

pub fn hinted(button: crate::proto::ui_service::Container<Msg>, hint: &str) -> Element<Msg> {
    tooltip(button, hint, TooltipPosition::TooltipTop)
        .style(theme::panel())
        .text_size(theme::TEXT_SMALL)
        .padding(6.0)
        .into()
}

fn menu_items(view: ViewId, row: &Row, here: &str) -> Vec<super::menu::Item> {
    use super::menu::Item;
    let mut items = Vec::new();

    if let Some(action) = primary(view, row).filter(|action| !action.transition) {
        items.push(Item::new(action.hint, action.message).glyph(action.glyph));
    }
    let key = row.snapshot_key().to_string();
    if row.kind.is_product()
        && row.kind.is_folder()
        && !key.is_empty()
        && !matches!(row.status, RowStatus::Complete)
    {
        let label = match row.size > 0 {
            true => format!("Download the whole snapshot — {}", format::bytes(row.size)),
            false => "Download the whole snapshot".to_string(),
        };
        items.push(Item::new(label, Msg::DownloadSnapshot(key.clone())).glyph(theme::glyph::DOWNLOAD));
    }
    let opens = matches!(row.status, RowStatus::Complete) && !row.kind.is_folder();
    if row.is_snapshot() && row.unviewable.is_empty() && !key.is_empty() && !opens {
        items.push(
            Item::new("View the snapshot", Msg::In(view, ViewMsg::PreviewProduct(key.clone())))
                .glyph(theme::glyph::EYE),
        );
    }
    if !row.product.is_empty() && !row.is_snapshot() && !row.kind.is_folder() {
        items.push(
            Item::new("On the globe with this file", Msg::GlobeFile(row.product.clone(), row.identifier.clone()))
                .glyph(theme::glyph::GLOBE),
        );
    }
    if !row.identifier.is_empty() && !row.is_snapshot() && !matches!(row.kind, RowKind::Folder) {
        let label = match row.globe.any() {
            true => "Take the snapshot off the globe",
            false => "Show the snapshot on the globe",
        };
        items.push(Item::new(label, Msg::In(view, ViewMsg::GlobeToggle(row.product_key().to_string()))));
    }
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
    if !row.identifier.is_empty() && !row.folded && matches!(row.status, RowStatus::Complete) {
        items.push(
            Item::new("Download again", Msg::Download(row.identifier.clone(), row.product.clone()))
                .danger(),
        );
    }
    if !row.name.is_empty() && !matches!(row.status, RowStatus::Remote) {
        items.push(Item::new(
            "Show in the file manager",
            Msg::Reveal(row.name.clone()),
        ));
    }
    if !row.name.is_empty() && !matches!(row.status, RowStatus::Remote) {
        let label = match row.status {
            RowStatus::Downloading { .. } | RowStatus::Paused { .. } => "Cancel the download",
            _ => "Delete from disk",
        };
        items.push(Item::new(label, Msg::Delete(row.name.clone())).danger());
    }
    let downloading = match row.folded {
        true => row.children.iter().any(|file| matches!(file.status, RowStatus::Downloading { .. })),
        false => row.kind.is_folder() && matches!(row.status, RowStatus::Downloading { .. }),
    };
    if downloading && !row.product.is_empty() {
        items.push(Item::new("Pause the snapshot", Msg::PauseSnapshot(row.product.clone())));
    }
    if row.folded && !row.product.is_empty() {
        items.push(
            Item::new("Delete the snapshot from disk", Msg::DeleteSnapshot(row.product.clone())).danger(),
        );
    }

    items
}
