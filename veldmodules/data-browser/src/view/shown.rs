//! view/shown.rs — "Now showing": what currently covers the globe.
//!
//! The shared list screen does not fit here, and not out of laziness: a table
//! row is a catalogue entry asked about size, date and download state, while a
//! row here is a layer asked about opacity and visibility. Only the name and
//! the glyph stay shared, and merging two grids into one for that would mean
//! columns left empty on every second row.
//!
//! The on-screen order is the reverse of the set order: newest on top, while on
//! the globe the last arrived lies above the rest (see state::overlay). The
//! flip lives here because "newest on top" is a property of the screen, not of
//! the set.
//!
//! Everything on the globe is shown here, not just rasters: a snapshot can sit
//! on it as a bare outline too, and then it can be removed nowhere else — its
//! glyph lives in a list still to be reached. Layers and outlines go in two
//! groups: these are two independent snapshot states (see handlers::outline),
//! and they are asked about different things — a layer about opacity and order,
//! an outline about nothing — so one grid for both would stay empty on every
//! second row.

use veld_ui_service_wrap::{column, popover, row, slider, Keyed};
use crate::proto::ui_service::{
    container, mono, scrollable, text, tooltip, Alignment, Color, Element, Length, Padding,
    ScrollDirection, TooltipPosition,
};
use crate::module::components::{format, list_screen, menu, preview_of, table, variables};
use crate::module::state::listing::Choice;
use crate::module::state::{globe::Outlined, overlay::OverlayState, Shift, State, ViewId};
use crate::module::{theme, Msg, ViewMsg};

/// Buttons in a layer row: hide, focus, remove and menu. Layer order and
/// jumps to the snapshot live in the menu (see [`options`]). The strip here is
/// its own, not the table's: a layer has an order, a catalogue entry does not.
const BUTTONS: f32 = 4.0;

/// What takes room in a row besides the name and the state label: the button
/// strip with its gaps, the screen gutters, the gap before the buttons, the
/// glyph on the left and the two gaps around it. Counted from the button count,
/// not tuned by hand: an added button would otherwise silently slide under the
/// clipping.
///
/// The state label is not here: its width is its own for every row, and a
/// constant cannot express it — "hidden" is six characters, while "step 1 of 9
/// · tiles 3/12" is twenty-seven. Named by one number, it would be cut off
/// exactly where it says the most (see [`row_name_chars`]).
const NAME_OVERHEAD: f32 = theme::ROW_BUTTON * BUTTONS
    + table::BUTTON_GAP * (BUTTONS - 1.0)
    + table::BUTTON_GAP * 2.0
    + theme::GUTTER * 2.0
    + NAME_GLYPH
    + NAME_GAPS;

/// The glyph left of the name and the two gaps around it (`row![…].spacing(8.0)`).
const NAME_GLYPH: f32 = 13.0;
const NAME_GAPS: f32 = 8.0 * 2.0;

/// How many name characters fit in a layer row beside its state label.
///
/// The label is measured by itself: it stands to the right of the name, grows
/// with the retrieval, and in a cramped pane takes twice as much from the name
/// as a constant would.
fn row_name_chars(width: f32, state: &str, buttoned: bool) -> usize {
    // A label button is wider than its text by the padding (see [`state_label`]).
    let button = if buttoned { 16.0 } else { 0.0 };
    let room = width - NAME_OVERHEAD - button - format::text_width(state, theme::TEXT_SMALL);
    format::mono_fit(room.max(60.0), theme::TEXT_MONO)
}

/// Height of a layer row: two lines — the name and the slider under it.
const ROW_HEIGHT: f32 = 54.0;

pub fn view(state: &State, view: ViewId) -> Element<Msg> {
    // The width for the name comes from the pane holding the list: the same
    // arithmetic as the table's, for the same reason.
    let width = state.pane_width(view);
    let shown = state.overlays.iter().filter(|overlay| !overlay.hidden).count();
    let ready = state.overlays.iter().filter(|overlay| overlay.on_globe()).count();
    // A snapshot lying as a raster is outlined on its own: showing it twice
    // would offer removing the same thing with two different buttons.
    let contours: Vec<&Outlined> = state
        .outlined
        .iter()
        .filter(|outlined| !state.overlays.iter().any(|layer| layer.identifier == outlined.key))
        .collect();

    let body: Element<Msg> = if state.overlays.is_empty() && contours.is_empty() {
        theme::empty("Empty. The box in the snapshot row outlines it here; the globe icon lays it as a raster.").into()
    } else {
        let mut lines: Vec<Element<Msg>> = Vec::new();
        // Bottom-up in the set — top-down on screen.
        lines.extend(
            state.overlays.iter().rev().map(|overlay| layer(state, view, overlay, width)),
        );
        if !contours.is_empty() {
            if !state.overlays.is_empty() {
                lines.push(divider("Outline only"));
            }
            lines.extend(
                contours.iter().map(|outlined| contour(state, view, outlined, width)),
            );
        }
        scrollable(column(lines).width(Length::Fill))
            .direction(ScrollDirection::ScrollVertical)
            .scrollbar(theme::scrollbar())
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    };

    column![
        header(state.overlays.len(), contours.len(), shown, ready),
        theme::hairline(theme::LINE_SOFT),
        container(body).width(Length::Fill).height(Length::Fill),
    ]
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

/// A caption over the second group. Not the list heading: there is one and it
/// stands above; this is a mark inside — to say that rows of another kind
/// follow.
fn divider(label: &str) -> Element<Msg> {
    container(
        text::<Msg>(label.to_string())
            .size(theme::TEXT_SMALL)
            .color(theme::INK_DIM)
            .single_line(),
    )
    .background(theme::SHELF)
    .width(Length::Fill)
    .height(Length::Fixed(theme::ROW_HEIGHT))
    .align_y(Alignment::Center)
    .padding(Padding { top: 0.0, bottom: 0.0, left: theme::GUTTER, right: theme::GUTTER })
    .into()
}

/// The heading: how many layers and what to do with them at once. Assembled by
/// the shared role (see `list_screen::heading`) — the heading here is the same
/// as over any list, and its own copy would diverge in size and height.
fn header(total: usize, contours: usize, shown: usize, ready: usize) -> Element<Msg> {
    let mut counts = format::counted(&[
        (total, ["layer", "layers", "layers"]),
        (contours, ["contour", "contours", "contours"]),
    ]);
    // What has not arrived yet is spoken of only while it has not arrived: a
    // "0 assembling" line on a finished set is noise.
    let assembling = total - ready;
    if assembling > 0 {
        counts.push_str(&format!(", {} assembling", assembling));
    } else if shown < total {
        counts.push_str(&format!(", {} on globe", shown));
    }

    let mut trailing = Vec::new();
    if total > 0 {
        // One button for both actions: it names what will happen, not what is
        // the case now. All hidden — offers to show.
        let (label, hidden) = match shown > 0 {
            true => ("Hide all", true),
            false => ("Show all", false),
        };
        trailing.push(theme::bar_button(label).on_press(Msg::OverlayHideAll(hidden)).into());
    }
    list_screen::heading("Now showing", counts, trailing)
}

/// One row: glyph, name, state and the opacity slider.
fn layer(state: &State, view: ViewId, overlay: &OverlayState, width: f32) -> Element<Msg> {
    let key = overlay.identifier.clone();
    let dim = overlay.hidden || !overlay.on_globe();
    // A hidden layer is dimmed by the background, not only by the ink: muted
    // ink is already taken here — it says "not on the globe yet", and hidden
    // would differ from assembling by one word on the right. Highlighting is
    // older: a picked layer is named by the strip under the globe and ringed
    // on it with a ribbon, and the row must say the same.
    let tint = if state.picked_key() == key {
        theme::RowTint::Picked
    } else if overlay.hidden {
        theme::RowTint::Dim
    } else {
        theme::RowTint::Plain
    };

    let said = state_said(overlay).map(|said| said.shown).unwrap_or_default();
    let name_chars = row_name_chars(width, &said, !overlay.variables.is_empty());
    let name = row![
        theme::row_glyph::<Msg>(
            theme::glyph::SATELLITE,
            if dim { theme::INK_FAINT } else { theme::ACCENT },
        ),
        mono::<Msg>(format::ellipsize(&overlay.label, name_chars))
            .size(theme::TEXT_MONO)
            .color(if dim { theme::INK_FAINT } else { theme::INK_SOFT }),
        theme::spacer(),
        state_label(state, overlay),
    ]
    .spacing(8.0)
    .align_items(Alignment::Center)
    .width(Length::Fill);
    let name = container(name).width(Length::Fill);

    // The slider is available for a hidden layer too: opacity is what a layer
    // returns to the globe already set with, and dimming it would force showing
    // it first.
    let opacity = row![
        container(
            slider(0.0..=1.0, overlay.opacity, {
                let key = key.clone();
                move |value| Msg::OverlayOpacity(key, value)
            })
            // The step is not about precision but about cost: opacity is baked
            // into the overlay vertices, and every new value rebuilds them all.
            // iced sends an event only on a value change, so the step cuts
            // hundreds of messages per drag to a couple of dozen, and five
            // percent is enough for the eye.
            .step(0.05)
            .height(14.0)
            .style(theme::slider())
            .width(Length::Fill),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .align_y(Alignment::Center),
        container(
            mono::<Msg>(format!("{:.0}%", overlay.opacity * 100.0))
                .size(theme::TEXT_SMALL)
                .color(theme::INK_MUTED),
        )
        // The width for "100%" is fixed: otherwise the slider jumps on every
        // digit while it is dragged.
        .width(Length::Fixed(38.0))
        .align_x(Alignment::End),
    ]
    .spacing(8.0)
    .align_items(Alignment::Center)
    .width(Length::Fill);

    let buttons = vec![
        table::icon_button(
            if overlay.hidden { theme::glyph::EYE_OFF } else { theme::glyph::EYE },
            // Hiding is an enabled mode of the row, and the button that holds
            // it stands pressed: otherwise one crossed-out eye in eleven dots
            // speaks of it.
            match overlay.hidden {
                true => theme::IconTone::Raised,
                false => theme::IconTone::Rest,
            },
            if overlay.hidden { "Show on globe" } else { "Hide" },
            Msg::OverlayHidden(key.clone(), !overlay.hidden),
        ),
        table::icon_button(
            theme::glyph::FOCUS,
            theme::IconTone::Rest,
            "Focus camera and highlight",
            Msg::OutlineFocus(key.clone()),
        ),
        table::icon_button(
            theme::glyph::TRASH,
            theme::IconTone::Rest,
            "Remove",
            Msg::OverlayRemove(key.clone()),
        ),
        options(state, view, overlay),
    ];

    line(
        column![name, opacity].spacing(6.0).width(Length::Fill).into(),
        buttons,
        key,
        tint,
        ROW_HEIGHT,
    )
}

/// One outline row: a snapshot that sits on the globe only as an outline.
///
/// One line, not two: an outline has neither opacity nor order — it is either
/// there or not. Hence fewer buttons: lay as raster, remove the outline, and
/// the menu with jumps to the snapshot itself.
fn contour(state: &State, view: ViewId, outlined: &Outlined, width: f32) -> Element<Msg> {
    let key = outlined.key.clone();

    let name = row![
        theme::row_glyph::<Msg>(theme::glyph::SATELLITE, theme::INK_FAINT),
        mono::<Msg>(format::ellipsize(&outlined.label, row_name_chars(width, "contour", false)))
            .size(theme::TEXT_MONO)
            .color(theme::INK_SOFT),
        theme::spacer(),
        text::<Msg>("contour".to_string())
            .size(theme::TEXT_SMALL)
            .color(theme::INK_FAINT)
            .single_line(),
    ]
    .spacing(8.0)
    .align_items(Alignment::Center)
    .width(Length::Fill);

    // The glyphs are the same and in the same order as in the layer row, and
    // the repeating ones are exactly the same: layers and outlines lie in one
    // list under each other, and an identically labelled action must be one
    // and the same. They are told apart by the first glyph — what the neighbor
    // row does not have: an outline is laid as a raster, while a layer is
    // hidden. The "lay as raster" mark is the same one this action is named
    // with in the list row (see `table::quick`) — one action, one mark.
    let buttons = vec![
        table::icon_button(
            theme::glyph::GLOBE,
            theme::IconTone::Rest,
            "Lay as raster",
            Msg::In(view, ViewMsg::GlobeToggle(key.clone())),
        ),
        table::icon_button(
            theme::glyph::FOCUS,
            theme::IconTone::Rest,
            "Focus camera and highlight",
            Msg::OutlineFocus(key.clone()),
        ),
        table::icon_button(
            theme::glyph::TRASH,
            theme::IconTone::Rest,
            "Remove outline",
            Msg::OutlineRemove(key.clone()),
        ),
        menu_button(state, key.clone(), jumps(state, view, &key, outlined.folder)),
    ];

    let tint = match state.picked_key() == key {
        true => theme::RowTint::Picked,
        false => theme::RowTint::Plain,
    };
    line(
        container(name).width(Length::Fill).into(),
        buttons,
        key,
        tint,
        theme::ROW_HEIGHT,
    )
}

/// The tail shared by both list rows: the body on the left, the button strip
/// on the right, the highlight under them and the hairline under the row. One
/// for a layer and an outline — they stand in one list under each other, and,
/// diverging in padding, button gap or hairline height, would read as two
/// different lists.
///
/// The height is declared here too, not only on the wrapper: `Length::Fill` on
/// the buttons inside a `Shrink`-height row collapses to zero, and the buttons
/// vanish without a trace in the layout (the same rule as for text wrapping —
/// Fill is needed on both levels).
fn line(
    body: Element<Msg>,
    buttons: Vec<Element<Msg>>,
    key: String,
    tint: theme::RowTint,
    height: f32,
) -> Element<Msg> {
    let content = row![
        body,
        container(row(buttons).spacing(table::BUTTON_GAP).align_items(Alignment::Center))
            .height(Length::Fill)
            .align_y(Alignment::Center),
    ]
    .spacing(12.0)
    .align_items(Alignment::Center)
    .width(Length::Fill)
    .height(Length::Fill)
    .padding(Padding {
        top: 0.0,
        bottom: 0.0,
        left: theme::GUTTER,
        right: theme::GUTTER,
    });

    column![
        theme::layer_row(content, tint).width(Length::Fill).height(Length::Fixed(height)),
        theme::hairline(theme::LINE_ROW),
    ]
    .width(Length::Fill)
    .key(key)
    .into()
}

/// The row menu: the "more" glyph, the panel opened under it and its closing.
/// One for a layer and an outline — the opened one is addressed by the
/// snapshot key (see `State::layer_menu`), and a second copy of this rule
/// would once leave a panel hanging over a closed row.
fn menu_button(state: &State, key: String, items: Vec<menu::Item>) -> Element<Msg> {
    let open = state.layer_menu(&key);
    let anchor = theme::row_button_icon(
        theme::glyph::MORE,
        match open {
            true => theme::IconTone::Raised,
            false => theme::IconTone::Rest,
        },
    )
    .on_press(Msg::OverlayMenu(if open { None } else { Some(key) }));
    popover(anchor, open, || menu::panel(items))
        .align_x(Alignment::End)
        .gap(4.0)
        .on_dismiss(Msg::OverlayMenu(None))
        .into()
}

/// Items that lead from the row to the snapshot itself: view it and find it in
/// the catalogue. The same for a layer and an outline — there is one snapshot
/// behind them, and an identically labelled action must be one and the same.
///
/// `folder` — whether the snapshot lies in storage as a catalogue folder: this
/// is what decides how to open it (see [`preview_of`]).
fn jumps(state: &State, view: ViewId, key: &str, folder: bool) -> Vec<menu::Item> {
    vec![
        menu::Item::new("View snapshot", Msg::In(view, preview_of(&state.library, key, folder)))
            .glyph(theme::glyph::EYE),
        menu::Item::new("Show in catalogue", Msg::In(view, ViewMsg::InCatalog(key.to_string())))
            .glyph(theme::glyph::FOLDER),
    ]
}

/// The layer menu: what is done rarely.
///
/// Layer order lives here too. It is moved rarely, and every glyph on the row
/// takes room from the name, which is already cut to half the width; a strip
/// grown to seven marks stops being readable before the room runs out. Jumps
/// to the snapshot are here for the same reason: from a layer one goes to what
/// it is made of, and not every minute either.
///
/// The order in the set is bottom-up, on screen top-down, so "higher" on screen
/// is `Shift::Up` in the set: the flip is one and lives here.
fn options(state: &State, view: ViewId, overlay: &OverlayState) -> Element<Msg> {
    let key = overlay.identifier.clone();
    let mut items = vec![
        menu::Item::new(Shift::Up.title(), Msg::OverlayShift(key.clone(), Shift::Up))
            .glyph(theme::glyph::UP),
        menu::Item::new(Shift::Down.title(), Msg::OverlayShift(key.clone(), Shift::Down))
            .glyph(theme::glyph::DOWN),
    ];
    items.extend(jumps(state, view, &key, overlay.folder));
    menu_button(state, key, items)
}

/// What is up with the layer right now. Empty for a settled visible layer:
/// there is nothing to say about it beyond what is already seen on the globe.
///
/// Three states and three sources: assembling is ours (rasters are still being
/// opened), hidden is ours too, while how many tiles are left is known only to
/// the globe, which tells it itself (see `state::overlay::Progress`). A
/// snapshot takes tens of seconds to wait for, and without this label it looks
/// stuck.
/// How many characters of a trouble label fit in the row.
///
/// The limit is not decoration: [`row_name_chars`] computes the name width from
/// what is left of the label, and the reason is written not by the layout but
/// by whoever refused — it has no length at all. Unlimited, it would leave the
/// snapshot name its lower bound of sixty points, that is, a stub the row
/// cannot be recognized by.
const TROUBLE_CHARS: usize = 44;

/// The layer state label: what stands in the row, in what color, and what to
/// say in full.
///
/// In full — separate from the shown, because the shown is compressed: a
/// reason cut mid-word explains nothing, and the one place in the row where it
/// fits whole is the tooltip.
struct Said {
    shown: String,
    color: Color,
    whole: Option<String>,
}

/// How many characters of the file name fit before the words about it. The
/// name is cut separately from the words: cut together, they would lose what
/// they stand for — "no finer than preview" behind a thirty-character name
/// would never reach the row.
const FILE_CHARS: usize = 16;

/// How many characters in a tooltip: one line, it cannot be wider than the
/// window — the same limit as the download list. The label parts share it
/// among themselves ([`format::share`]), each cut its own way.
const TOOLTIP_CHARS: usize = 90;

fn state_said(overlay: &OverlayState) -> Option<Said> {
    let plain = |shown: &str, color| Said { shown: shown.to_string(), color, whole: None };
    let said = match (overlay.on_globe(), overlay.hidden) {
        (false, _) => plain("preparing…", theme::INK_FAINT),
        (true, true) => plain("hidden", theme::INK_FAINT),
        // The raster refusal is spoken last and stays forever: while the layer
        // waits for something, the waiting matters more, and once it has
        // waited — only the refusal explains why zooming in will give nothing
        // more.
        (true, false) => match overlay.progress.said() {
            Some(said) => plain(&said, theme::ACCENT_TEXT),
            None => match settled_said(overlay) {
                Some((shown, whole)) => Said { shown, color: theme::INK_FAINT, whole: Some(whole) },
                None => return None,
            },
        },
    };
    Some(said)
}

/// The label of a settled layer: the name of the file lying as detailed, its
/// variable (`variable`) and the words about it — about it alone
/// (`detailed_trouble`) — then the rest (`trouble`). A preview limit or a
/// binding refusal under the detailed file name would read as said about it,
/// so the boundary between them comes from the globe and is not drawn here. A
/// name without words — the layer has settled, and there is nothing more to
/// say about it beyond which file lay down and which of its variables it is;
/// the rest goes after it behind a semicolon, as its own statement. Empty —
/// there is neither a name nor words.
///
/// The shown is assembled from parts, each with its own limit: the name —
/// [`FILE_CHARS`], the words about the detailed one — what is left of the row,
/// the rest — only if there is still room. The variable stands in the row
/// while there is nothing worse to say about the file: trouble words outrank
/// the name of what did not show up. The tooltip is the same label in full, as
/// far as its line ([`TOOLTIP_CHARS`]) holds: the parts share the line evenly,
/// short ones take their own ([`format::share`]), so the name with the variable
/// units and the gist of the trouble arrive for any file name. The name is cut
/// in the middle (the edges are recognizable), the words — from the tail (the
/// gist comes first).
fn settled_said(overlay: &OverlayState) -> Option<(String, String)> {
    /// How a part is cut: the name — in the middle, the words — from the tail.
    enum Cut {
        Middle,
        Tail,
    }
    let mut shown: Vec<String> = Vec::new();
    // The tooltip parts in full, with how to cut each; the rest — last.
    let mut parts: Vec<(String, Cut)> = Vec::new();
    let room = |shown: &[String]| {
        let taken: usize = shown.iter().map(|part| part.chars().count()).sum();
        match shown.is_empty() {
            true => TROUBLE_CHARS,
            false => TROUBLE_CHARS.saturating_sub(taken + 3).max(TROUBLE_CHARS / 4),
        }
    };
    if let Some(file) = &overlay.detailed {
        shown.push(format::ellipsize(file, FILE_CHARS));
        parts.push((file.clone(), Cut::Middle));
    }
    if let Some(variable) = &overlay.variable {
        if overlay.detailed_trouble.is_empty() {
            shown.push(format::head(format::variable_name(&variable.path), room(&shown)));
        }
        parts.push((format::variable(&variable.path, &variable.said, &variable.units), Cut::Tail));
    }
    if !overlay.detailed_trouble.is_empty() {
        let said = &overlay.detailed_trouble;
        shown.push(format::head(said, room(&shown)));
        parts.push((said.clone(), Cut::Tail));
    }
    let mut shown = shown.join(" · ");
    let about_detailed = parts.len();
    if !overlay.trouble.is_empty() {
        let rest = &overlay.trouble;
        if about_detailed == 0 {
            shown = format::head(rest, TROUBLE_CHARS);
        } else {
            let room = TROUBLE_CHARS.saturating_sub(shown.chars().count() + 2);
            if room >= TROUBLE_CHARS / 4 {
                shown = format!("{}; {}", shown, format::head(rest, room));
            }
        }
        parts.push((rest.clone(), Cut::Tail));
    }
    if parts.is_empty() {
        return None;
    }
    // Separators take three characters per joint (" · " is one longer than
    // "; ", and that is a surplus, not a shortfall).
    let lengths: Vec<usize> = parts.iter().map(|(text, _)| text.chars().count()).collect();
    let shares = format::share(&lengths, TOOLTIP_CHARS.saturating_sub(3 * (parts.len() - 1)));
    let mut whole = String::new();
    for (at, ((text, cut), share)) in parts.into_iter().zip(shares).enumerate() {
        if at > 0 {
            whole.push_str(if at < about_detailed { " · " } else { "; " });
        }
        whole.push_str(&match cut {
            Cut::Middle => format::ellipsize(&text, share),
            Cut::Tail => format::head(&text, share),
        });
    }
    Some((shown, whole))
}

/// The same label as an element. Separate from [`state_said`] because its
/// width must be known before the row is assembled: the name room is computed
/// from it.
///
/// On a layer whose file has many variables the label is a button: under it
/// sits the variable list (`OverlayState.variables`), and an item names a
/// variable to the globe. The checkmark is on the one the globe shows; the
/// named one it refused has no checkmark.
fn state_label(state: &State, overlay: &OverlayState) -> Element<Msg> {
    let Some(said) = state_said(overlay) else { return theme::nothing() };
    let label = text::<Msg>(said.shown).size(theme::TEXT_SMALL).color(said.color).single_line();
    let label: Element<Msg> = match said.whole {
        // The tooltip is assembled into its own line ([`TOOLTIP_CHARS`]); the
        // cut here is a safeguard, not a rule.
        Some(whole) => tooltip(label, format::ellipsize(&whole, TOOLTIP_CHARS), TooltipPosition::TooltipTop)
            .style(theme::panel())
            .text_size(theme::TEXT_SMALL)
            .into(),
        None => label.into(),
    };
    if overlay.variables.is_empty() {
        return label;
    }
    let key = overlay.identifier.clone();
    let open = state.layer_variables(&key);
    let anchor = theme::surface_button(label, open)
        .padding(Padding { top: 2.0, bottom: 2.0, left: 8.0, right: 8.0 })
        .on_press(Msg::OverlayVariables(if open { None } else { Some(key.clone()) }));
    let shown = overlay.variable.as_ref().map(|shown| shown.path.as_str()).unwrap_or_default();
    let named: Vec<variables::Named<'_>> = overlay.variables.iter().map(variables::Named::from).collect();
    popover(anchor, open, || {
        menu::panel(variables::items(&named, shown, |path| Msg::OverlayVariable(key.clone(), path)))
    })
    .align_x(Alignment::End)
    .gap(4.0)
    .on_dismiss(Msg::OverlayVariables(None))
    .into()
}
