use veld_ui_service_wrap::{column, popover, row, slider, Keyed};
use crate::proto::ui_service::{
    container, mono, scrollable, text, tooltip, Alignment, Color, Element, Length, Padding,
    ScrollDirection, TooltipPosition,
};
use crate::module::components::{format, list_screen, menu, preview_of, table, variables};
use crate::module::state::listing::Choice;
use crate::module::state::{globe::Outlined, overlay::OverlayState, Shift, State, ViewId};
use crate::module::{theme, Msg, ViewMsg};

const BUTTONS: f32 = 4.0;

const NAME_OVERHEAD: f32 = theme::ROW_BUTTON * BUTTONS
    + table::BUTTON_GAP * (BUTTONS - 1.0)
    + table::BUTTON_GAP * 2.0
    + theme::GUTTER * 2.0
    + NAME_GLYPH
    + NAME_GAPS;

const NAME_GLYPH: f32 = 13.0;
const NAME_GAPS: f32 = 8.0 * 2.0;

fn row_name_chars(width: f32, state: &str, buttoned: bool) -> usize {
    let button = if buttoned { 16.0 } else { 0.0 };
    let room = width - NAME_OVERHEAD - button - format::text_width(state, theme::TEXT_SMALL);
    format::mono_fit(room.max(60.0), theme::TEXT_MONO)
}

const ROW_HEIGHT: f32 = 54.0;

pub fn view(state: &State, view: ViewId) -> Element<Msg> {
    let width = state.pane_width(view);
    let shown = state.overlays.iter().filter(|overlay| !overlay.hidden).count();
    let ready = state.overlays.iter().filter(|overlay| overlay.on_globe()).count();
    let contours: Vec<&Outlined> = state
        .outlined
        .iter()
        .filter(|outlined| !state.overlays.iter().any(|layer| layer.identifier == outlined.key))
        .collect();

    let body: Element<Msg> = if state.overlays.is_empty() && contours.is_empty() {
        theme::empty("Empty. The box in the snapshot row outlines it here; the globe icon lays it as a raster.").into()
    } else {
        let mut lines: Vec<Element<Msg>> = Vec::new();
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

fn header(total: usize, contours: usize, shown: usize, ready: usize) -> Element<Msg> {
    let mut counts = format::counted(&[
        (total, ["layer", "layers", "layers"]),
        (contours, ["contour", "contours", "contours"]),
    ]);
    let assembling = total - ready;
    if assembling > 0 {
        counts.push_str(&format!(", {} assembling", assembling));
    } else if shown < total {
        counts.push_str(&format!(", {} on globe", shown));
    }

    let mut trailing = Vec::new();
    if total > 0 {
        let (label, hidden) = match shown > 0 {
            true => ("Hide all", true),
            false => ("Show all", false),
        };
        trailing.push(theme::bar_button(label).on_press(Msg::OverlayHideAll(hidden)).into());
    }
    list_screen::heading("Now showing", counts, trailing)
}

fn layer(state: &State, view: ViewId, overlay: &OverlayState, width: f32) -> Element<Msg> {
    let key = overlay.identifier.clone();
    let dim = overlay.hidden || !overlay.on_globe();
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

    let opacity = row![
        container(
            slider(0.0..=1.0, overlay.opacity, {
                let key = key.clone();
                move |value| Msg::OverlayOpacity(key, value)
            })
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
        .width(Length::Fixed(38.0))
        .align_x(Alignment::End),
    ]
    .spacing(8.0)
    .align_items(Alignment::Center)
    .width(Length::Fill);

    let buttons = vec![
        table::icon_button(
            if overlay.hidden { theme::glyph::EYE_OFF } else { theme::glyph::EYE },
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

fn jumps(state: &State, view: ViewId, key: &str, folder: bool) -> Vec<menu::Item> {
    vec![
        menu::Item::new("View snapshot", Msg::In(view, preview_of(&state.library, key, folder)))
            .glyph(theme::glyph::EYE),
        menu::Item::new("Show in catalogue", Msg::In(view, ViewMsg::InCatalog(key.to_string())))
            .glyph(theme::glyph::FOLDER),
    ]
}

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

const TROUBLE_CHARS: usize = 44;

struct Said {
    shown: String,
    color: Color,
    whole: Option<String>,
}

const FILE_CHARS: usize = 16;

const TOOLTIP_CHARS: usize = 90;

fn state_said(overlay: &OverlayState) -> Option<Said> {
    let plain = |shown: &str, color| Said { shown: shown.to_string(), color, whole: None };
    let said = match (overlay.on_globe(), overlay.hidden) {
        (false, _) => plain("preparing…", theme::INK_FAINT),
        (true, true) => plain("hidden", theme::INK_FAINT),
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

fn settled_said(overlay: &OverlayState) -> Option<(String, String)> {
    enum Cut {
        Middle,
        Tail,
    }
    let mut shown: Vec<String> = Vec::new();
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

fn state_label(state: &State, overlay: &OverlayState) -> Element<Msg> {
    let Some(said) = state_said(overlay) else { return theme::nothing() };
    let label = text::<Msg>(said.shown).size(theme::TEXT_SMALL).color(said.color).single_line();
    let label: Element<Msg> = match said.whole {
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
