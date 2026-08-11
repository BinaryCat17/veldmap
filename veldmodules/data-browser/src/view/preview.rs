//! view/preview.rs — вид предпросмотра снимка.
//!
//! Выход отсюда — закрытие вкладки, поэтому своей кнопки «назад» нет: она
//! знала бы, куда возвращаться, только назвав другой вид по имени, а
//! открывают превью из любого.

use veld_ui_service_wrap::{column, row};
use crate::proto::ui_service::{
    container, image, mono, scrollable, text, Alignment, Element, FontWeight, Length,
    Padding, ScrollDirection,
};
use crate::module::components::format;
use crate::module::state::{PreviewState, State};
use crate::module::{theme, Msg};

/// Ширина панели свойств. Фиксирована: колонка значений не должна ездить от
/// длины имени файла.
const PANEL_WIDTH: f32 = 290.0;

/// Шаг увеличения. Одинаковый в обе стороны, поэтому и делить, и умножать
/// можно на него же.
const ZOOM_STEP: f32 = 1.5;

pub fn view(state: &State, preview: &PreviewState) -> Element<Msg> {
    // Пока картинки нет — на экране одна строка состояния: ждём, не смогли
    // или нечего показывать.
    let status = if preview.is_loading() {
        Some("Загружается…".to_string())
    } else if let Some(error) = &preview.error {
        Some(error.clone())
    } else if preview.texture.is_none() {
        Some("Показать нечего: снимок не открылся".to_string())
    } else {
        None
    };

    if let Some(status) = status {
        return container(text::<Msg>(status).size(theme::TEXT_BODY).color(theme::INK_DIM))
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x()
            .center_y()
            .padding(Padding::new(20.0))
            .into();
    }

    let canvas: Element<Msg> = match (preview.zoom, preview.preview_size) {
        // Вписанный снимок занимает всё место и меняется вместе с окном;
        // масштабированный живёт своим размером и уезжает в прокрутку.
        (zoom, Some((width, height))) if zoom > 0.0 => scrollable(
            container(picture(preview, Length::Fixed(width as f32 * zoom), Length::Fixed(height as f32 * zoom)))
                .width(Length::Fill)
                .height(Length::Fill)
                .center_x()
                .center_y()
                .padding(Padding::new(20.0)),
        )
        .direction(ScrollDirection::ScrollBoth)
        .scrollbar(theme::scrollbar())
        .width(Length::Fill)
        .height(Length::Fill)
        .into(),
        _ => container(picture(preview, Length::Fill, Length::Fill))
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(Padding::new(20.0))
            .into(),
    };

    row![
        column![
            toolbar(state, preview),
            theme::hairline(theme::LINE_SOFT),
            container(canvas).background(theme::CHROME).width(Length::Fill).height(Length::Fill),
        ]
        .width(Length::Fill)
        .height(Length::Fill),
        properties(state, preview),
    ]
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

fn picture(preview: &PreviewState, width: Length, height: Length) -> Element<Msg> {
    image::<Msg>(crate::proto::ui_service::core::ResourceHandle {
        id: preview.texture.as_ref().map(|texture| texture.id()).unwrap_or_default(),
        size: 0,
    })
    .width(width)
    .height(height)
    .into()
}

/// Имя снимка и масштаб.
fn toolbar(state: &State, preview: &PreviewState) -> Element<Msg> {
    let name = mono::<Msg>(format::ellipsize(
        &preview.label,
        format::mono_fit(state.logical_width() - PANEL_WIDTH - 160.0, theme::TEXT_LABEL),
    ))
    .size(theme::TEXT_LABEL)
    .color(theme::INK);

    let step = |label: &str, zoom: f32| {
        theme::surface_button(text::<Msg>(label.to_string()).size(theme::TEXT_LABEL).single_line(), false)
            .width(Length::Fixed(27.0))
            .height(Length::Fixed(27.0))
            .on_press(Msg::Zoom(zoom))
    };

    // Текущий масштаб — он же кнопка возврата к вписанному: отдельной кнопке
    // «вписать» тут места нет, а подпись и так говорит, что показано сейчас.
    let current = theme::surface_button(
        text::<Msg>(if preview.zoom > 0.0 {
            format!("{:.0}%", preview.zoom * 100.0)
        } else {
            "вписан".to_string()
        })
        .size(theme::TEXT_LABEL)
        .single_line(),
        false,
    )
    .height(Length::Fixed(27.0))
    .padding(Padding { top: 0.0, bottom: 0.0, left: 11.0, right: 11.0 })
    .on_press(Msg::Zoom(0.0));

    let zoom = if preview.zoom > 0.0 { preview.zoom } else { 1.0 };
    row![
        container(name).width(Length::Fill).clip(),
        step("−", zoom / ZOOM_STEP),
        current,
        step("+", zoom * ZOOM_STEP),
    ]
    .spacing(5.0)
    .width(Length::Fill)
    .align_items(Alignment::Center)
    .padding(Padding { top: 11.0, bottom: 11.0, left: theme::GUTTER, right: theme::GUTTER })
    .into()
}

/// Свойства снимка: то, что о нём известно, не открывая файл заново. Размер и
/// время — из библиотеки, и только у скачанного: за удалённым записи нет, а
/// придумывать её здесь было бы вторым источником правды о диске.
fn properties(state: &State, preview: &PreviewState) -> Element<Msg> {
    let entry = preview.entry.as_ref().and_then(|name| {
        state.library.entries.iter().find(|entry| &entry.name == name)
    });

    let mut lines: Vec<Element<Msg>> = vec![
        text::<Msg>("Свойства снимка".to_string())
            .size(theme::TEXT_LABEL)
            .color(theme::INK_DIM)
            .weight(FontWeight::WeightBold)
            .single_line()
            .into(),
    ];

    if let Some((width, height)) = preview.source_size {
        lines.push(property("разрешение", format!("{} × {}", width, height)));
    }
    if let Some(entry) = entry {
        if entry.done > 0 {
            lines.push(property("размер", format::bytes(entry.done)));
        }
        if entry.modified > 0 {
            lines.push(property("скачан", format::date(entry.modified, format::now())));
        }
    }
    if let Some((width, height)) = preview.preview_size {
        lines.push(property("превью", format!("{} × {}", width, height)));
    }

    container(column(lines).spacing(8.0).width(Length::Fill))
        .background(theme::SHELF)
        .width(Length::Fixed(PANEL_WIDTH))
        .height(Length::Fill)
        .padding(Padding::new(theme::GUTTER))
        .into()
}

fn property(name: &str, value: String) -> Element<Msg> {
    column![
        row![
            text::<Msg>(name.to_string()).size(theme::TEXT_BODY).color(theme::INK_DIM).single_line(),
            container(mono::<Msg>(value).size(theme::TEXT_SMALL).color(theme::INK))
                .width(Length::Fill)
                .align_x(Alignment::End),
        ]
        .width(Length::Fill)
        .padding(Padding { top: 6.0, bottom: 6.0, left: 0.0, right: 0.0 })
        .align_items(Alignment::Center),
        theme::hairline(theme::LINE_SOFT),
    ]
    .width(Length::Fill)
    .into()
}
