//! view/preview.rs — the preview of a scene.
//!
//! The frame itself is drawn by the canvas (image-view) into the texture
//! delegated to it; here is the place for it, the toolbar with the scale, and
//! the properties bar under the frame. The truth about the display (source
//! dimensions, scale, production progress) arrives by the canvas broadcast and
//! lies in `PreviewState::view_state` — there is no own copy of that truth
//! here.
//!
//! The properties go as a bar, not as a column at the side: four things can be
//! said about a scene, and a column for them would take a third of the frame's
//! width for four lines. The bar is the same as under the globe
//! (`theme::chrome_bar`): a view whose whole content another module draws is
//! framed the same way.
//!
//! The way out of here is closing the tab, so there is no own "back" button: it
//! could know where to return only by naming another view by name, and the
//! preview is opened from any of them.

use veld_ui_service_wrap::{column, popover, row, viewport};
use crate::proto::ui_service::{
    container, mono, text, Alignment, Element, Length, Padding,
};
use crate::module::components::{format, menu, variables};
use crate::module::state::{PreviewState, State, ViewId};
use crate::module::{theme, Msg, ViewMsg};

/// How much room to leave for the name in the toolbar: everything but the scale
/// buttons.
const CONTROLS_WIDTH: f32 = 230.0;

/// How much of the properties bar width is given to the display progress on
/// the right: it appears and disappears on the fly, and a value that took its
/// place would push it past the edge.
const PROGRESS_WIDTH: f32 = 260.0;

/// No fewer characters than this are left for the variable: the name with units
/// must stay recognizable even in a narrow pane, even if cut.
const VARIABLE_CHARS_FLOOR: usize = 16;

/// Separator of the bar facts; its width counts into the room left for the
/// variable.
const FACT_SEPARATOR: &str = "   ·   ";

/// What the variable button adds to the width of its text: the padding at the
/// edges and the bar gap up to the next fact.
const BUTTON_ROOM: f32 = 8.0 + 8.0 + crate::module::view::BAR_SPACING;

pub fn view(state: &State, view: ViewId, preview: &PreviewState) -> Element<Msg> {
    // A failure displaces the canvas: there is nothing to show over a dead
    // frame, and the reason of the failure is the only thing to report here. An
    // incomplete frame is not that case — it is alive, and the bar at the bottom
    // speaks for it.
    let body: Element<Msg> = match preview.failure() {
        Some(error) => theme::empty(error).into(),
        None => canvas(view, preview),
    };

    column![
        toolbar(state, view, preview),
        theme::hairline(theme::LINE_SOFT),
        container(body).background(theme::CHROME).width(Length::Fill).height(Length::Fill),
        theme::hairline(theme::LINE),
        properties(state, view, preview),
    ]
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

/// The place for the canvas frame. The area gets its texture from us — we
/// allocated it in answer to the previous on_resized; on the first frame it is
/// not there yet, and the place stands empty until the event arrives.
fn canvas(view: ViewId, preview: &PreviewState) -> Element<Msg> {
    let mut area = viewport::<Msg>()
        .width(Length::Fill)
        .height(Length::Fill)
        .on_resized(move |size| Msg::In(view, ViewMsg::PreviewResized(size)))
        .on_pointer(move |pointer| Msg::In(view, ViewMsg::PreviewPointer(pointer)));
    if let Some(surface) = &preview.surface {
        area = area.texture(surface.handle());
    }
    container(area).width(Length::Fill).height(Length::Fill).into()
}

/// The scene name and the scale. The display progress is not here: it is not
/// about the scene, but about what is happening to it now, and its place is the
/// status bar at the bottom.
fn toolbar(state: &State, view: ViewId, preview: &PreviewState) -> Element<Msg> {
    let name = mono::<Msg>(format::ellipsize(
        &preview.label,
        format::mono_fit(state.pane_width(view) - CONTROLS_WIDTH, theme::TEXT_LABEL),
    ))
    .size(theme::TEXT_LABEL)
    .color(theme::INK);

    let step = |label: &str, direction: f32| {
        theme::surface_button(text::<Msg>(label.to_string()).size(theme::TEXT_LABEL).single_line(), false)
            .width(Length::Fixed(27.0))
            .height(Length::Fixed(27.0))
            .on_press(Msg::In(view, ViewMsg::PreviewZoom(direction)))
    };

    // The scale label is also the "fit" button: there is no room for a separate
    // button here, and the label already says what is shown now.
    let scale = preview.view_state.as_ref().map(|view| view.scale).unwrap_or_default();
    let current = theme::surface_button(
        text::<Msg>(if scale > 0.0 {
            format!("{:.0}%", scale * 100.0)
        } else {
            "…".to_string()
        })
        .size(theme::TEXT_LABEL)
        .single_line(),
        false,
    )
    .height(Length::Fixed(27.0))
    .padding(Padding { top: 0.0, bottom: 0.0, left: 11.0, right: 11.0 })
    .on_press(Msg::In(view, ViewMsg::PreviewFit));

    row![
        container(name).width(Length::Fill),
        step("−", -1.0),
        current,
        step("+", 1.0),
    ]
    .spacing(5.0)
    .width(Length::Fill)
    .align_items(Alignment::Center)
    .padding(Padding { top: 11.0, bottom: 11.0, left: theme::GUTTER, right: theme::GUTTER })
    .into()
}

/// The display progress line. `None` — there is nothing to show: the canvas is
/// not busy and complains about nothing.
///
/// A complaint outranks the progress: progress next to a step that has not
/// arrived would say that all goes as usual — while it does not.
fn progress_line(preview: &PreviewState) -> Option<String> {
    if preview.request.is_pending() {
        return Some("opening…".to_string());
    }
    let view = preview.view_state.as_ref()?;
    // The reason comes from the canvas, but how much of it to show is decided
    // here: in the bar it neighbors the properties and, not being shortened, it
    // would push them out. In full it is always in the log.
    // As is: the reason arrives already phrased in words ("frame froze: …",
    // "production: …"), and a prefix of one's own here would only lie — a
    // frozen frame is not "incomplete".
    if !view.trouble.is_empty() {
        return Some(format::ellipsize(&view.trouble, 66));
    }
    // The progress is named in the same vocabulary as for a scene on the globe:
    // the work is one and the same — a tile pyramid is being gathered — and two
    // ways to tell of it would make the viewer guess whether these are
    // different things.
    crate::module::state::overlay::Progress {
        ready: view.ready,
        total: view.total,
        step: view.step,
        steps: view.steps,
        pass: (view.read_bytes, view.total_bytes),
        working: view.working,
        ..Default::default()
    }
    .said()
}

/// The bar under the frame: what the scene is on the left, what is happening to
/// it now on the right.
///
/// The dimensions come from the canvas (the tiler described them); the size and
/// the time on disk come from the library, and only for a downloaded one: a
/// removed one has no record. None of this is computed here — the bar only
/// names.
fn properties(state: &State, view: ViewId, preview: &PreviewState) -> Element<Msg> {
    let entry = preview.entry.as_ref().and_then(|name| {
        state.library.entries.iter().find(|entry| &entry.name == name)
    });

    let view_state = preview.view_state.as_ref();
    let mut facts: Vec<String> = Vec::new();
    if let Some(view) = view_state {
        if view.source_width > 0 {
            facts.push(format!("{} × {}", view.source_width, view.source_height));
        }
        if view.scale > 0.0 {
            facts.push(format!("{:.0}%", view.scale * 100.0));
        }
    }
    if let Some(entry) = entry {
        if entry.done > 0 {
            facts.push(format::bytes(entry.done));
        }
        if entry.modified > 0 {
            facts.push(format!("downloaded {}", format::date(entry.modified, format::now())));
        }
    }

    let mut parts: Vec<Element<Msg>> = Vec::new();
    // What the scene is goes first: for a file of many variables the dimensions
    // without the variable name do not say what is shown. The variable gets
    // whatever the other facts and the room for the display progress leave of
    // the pane width — the bar is one line and does not wrap. The file's words
    // run up to a line and a half and fit only whole: cut, they would read
    // worse than the name with units without them. The variable is a button:
    // under it is the list of everything to choose from. The shown one — by the
    // canvas's word, and when the canvas refused the named one — the named one:
    // the button and the list must survive a refusal, otherwise another one
    // cannot be chosen.
    let shown = view_state
        .and_then(|view| view.variable.clone())
        .or_else(|| preview.variables.iter().find(|variable| variable.path == preview.variable).cloned());
    if let Some(shown) = shown {
        let variables = preview.variables.as_slice();
        let taken: usize =
            facts.iter().map(|fact| fact.chars().count() + FACT_SEPARATOR.chars().count()).sum();
        let room = format::mono_fit(state.pane_width(view) - PROGRESS_WIDTH - BUTTON_ROOM, theme::TEXT_SMALL)
            .saturating_sub(taken)
            .max(VARIABLE_CHARS_FLOOR);
        let full = format::variable(&shown.path, &shown.said, &shown.units);
        let said = match full.chars().count() <= room {
            true => full,
            false => format::head(&format::variable(&shown.path, "", &shown.units), room),
        };
        let open = state.variables_menu(view);
        let anchor = theme::surface_button(
            mono::<Msg>(said).size(theme::TEXT_SMALL).color(theme::INK_SOFT).single_line(),
            open,
        )
        .padding(Padding { top: 2.0, bottom: 2.0, left: 8.0, right: 8.0 })
        .on_press(Msg::In(view, ViewMsg::PreviewVariables(!open)));
        // The tick is on what the canvas shows; the named one it refused has no
        // tick: it is not on the screen.
        let ticked = view_state.and_then(|view| view.variable.as_ref()).map(|shown| shown.path.as_str()).unwrap_or_default();
        let named: Vec<variables::Named<'_>> = variables.iter().map(variables::Named::from).collect();
        parts.push(
            popover(anchor, open, || {
                menu::panel(variables::items(&named, ticked, |path| Msg::In(view, ViewMsg::PreviewVariable(path))))
            })
                .gap(4.0)
                .on_dismiss(Msg::In(view, ViewMsg::PreviewVariables(false)))
                .into(),
        );
        if !facts.is_empty() {
            facts.insert(0, String::new());
        }
    }
    parts.push(
        mono::<Msg>(facts.join(FACT_SEPARATOR))
            .size(theme::TEXT_SMALL)
            .color(theme::INK_SOFT)
            .single_line()
            .into(),
    );
    parts.push(theme::spacer().into());
    // The display progress sits on the right, at the edge: it changes before
    // one's eyes, and next to the properties it would jerk them out of place.
    if let Some(line) = progress_line(preview) {
        parts.push(
            text::<Msg>(line).size(theme::TEXT_LABEL).color(theme::INK_DIM).single_line().into(),
        );
    }

    theme::chrome_bar(
        row(parts)
            .spacing(crate::module::view::BAR_SPACING)
            .width(Length::Fill)
            .height(Length::Fill)
            .align_items(Alignment::Center),
    )
    .into()
}
