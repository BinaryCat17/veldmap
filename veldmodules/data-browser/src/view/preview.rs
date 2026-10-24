use veld_ui_service_wrap::{column, popover, row, viewport};
use crate::proto::ui_service::{
    container, mono, text, Alignment, Element, Length, Padding,
};
use crate::module::components::{format, menu, variables};
use crate::module::state::{PreviewState, State, ViewId};
use crate::module::{theme, Msg, ViewMsg};

const CONTROLS_WIDTH: f32 = 230.0;

const PROGRESS_WIDTH: f32 = 260.0;

const VARIABLE_CHARS_FLOOR: usize = 16;

const FACT_SEPARATOR: &str = "   ·   ";

const BUTTON_ROOM: f32 = 8.0 + 8.0 + crate::module::view::BAR_SPACING;

pub fn view(state: &State, view: ViewId, preview: &PreviewState) -> Element<Msg> {
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

fn progress_line(preview: &PreviewState) -> Option<String> {
    if preview.request.is_pending() {
        return Some("opening…".to_string());
    }
    let view = preview.view_state.as_ref()?;
    if !view.trouble.is_empty() {
        return Some(format::ellipsize(&view.trouble, 66));
    }
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
