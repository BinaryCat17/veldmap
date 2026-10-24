use veld_ui_service_wrap::{column, row};
use crate::proto::ui_service::{
    container, scrollable, text, Alignment, Element, FontWeight, Length, Padding,
    ScrollDirection, ScrollTo,
};
use crate::module::components::{arrange, controls, format, table, Row};
use crate::module::state::{Batch, ViewId};
use crate::module::state::listing::{ListingState, Menu};
use crate::module::{theme, Msg, ViewMsg};

pub struct Screen<'a> {
    pub title: &'a str,
    pub subtitle: String,
    pub path: Option<&'a str>,
    pub empty: &'a str,
    pub controls: Option<Element<Msg>>,
    pub rows: Vec<Row>,
    pub picked: &'a str,
    pub batch: Batch,
    pub menu: Option<&'a Menu>,
    pub target: &'a str,
}

const SUBTITLE_CHARS: usize = 72;

pub fn heading(title: &str, subtitle: String, trailing: Vec<Element<Msg>>) -> Element<Msg> {
    let mut line = row![
        text::<Msg>(title.to_string())
            .size(theme::TEXT_TITLE)
            .color(theme::INK)
            .weight(FontWeight::WeightBold)
            .single_line(),
        text::<Msg>(format::ellipsize(&subtitle, SUBTITLE_CHARS))
            .size(theme::TEXT_BODY)
            .color(theme::INK_DIM)
            .single_line(),
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

pub fn view(
    view: ViewId,
    screen: Screen<'_>,
    listing: &ListingState,
    width: f32,
) -> Element<Msg> {
    let arranged = arrange::arrange(&screen.rows, listing);
    let optional = table::Optional {
        twisty: arranged.shown().any(Row::expandable),
        checkable: arranged.marks().next().is_some(),
    };
    let table::Fit { columns, name: name_width, compact } = table::fit(width, optional);

    let mut trailing: Vec<Element<Msg>> = Vec::new();
    if !listing.selected.is_empty() {
        trailing.push(
            text::<Msg>(format!("{} selected", listing.selected.len()))
                .size(theme::TEXT_LABEL)
                .color(theme::ACCENT_TEXT)
                .single_line()
                .into(),
        );
        let mut act = |glyph: &'static str, tone: theme::IconTone, hint: &str, msg: ViewMsg| {
            trailing.push(table::icon_button(glyph, tone, hint, Msg::In(view, msg)));
        };
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
