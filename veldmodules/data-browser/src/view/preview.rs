//! view/preview.rs — вид предпросмотра снимка.
//!
//! Сам кадр рисует канва (image-view) в делегированную ей текстуру; здесь —
//! место под неё, тулбар с масштабом и полоса свойств под кадром. Правда о
//! показе (размеры источника, масштаб, ход производства) приходит рассылкой
//! канвы и лежит в `PreviewState::view_state` — своей копии этой правды у нас
//! нет.
//!
//! Свойства идут полосой, а не колонкой сбоку: сказать о снимке можно четыре
//! вещи, и колонка под них отнимала бы у кадра треть ширины ради четырёх строк.
//! Полоса — та же, что под глобусом (`theme::chrome_bar`): вид, у которого всё
//! содержимое рисует чужой модуль, обрамляется одинаково.
//!
//! Выход отсюда — закрытие вкладки, поэтому своей кнопки «назад» нет: она
//! знала бы, куда возвращаться, только назвав другой вид по имени, а
//! открывают превью из любого.

use veld_ui_service_wrap::{column, row, viewport};
use crate::proto::ui_service::{
    container, mono, text, Alignment, Element, Length, Padding,
};
use crate::module::components::format;
use crate::module::state::{PreviewState, State, ViewId};
use crate::module::{theme, Msg, ViewMsg};

/// Сколько места оставить имени в тулбаре: всё, кроме кнопок масштаба.
const CONTROLS_WIDTH: f32 = 230.0;

pub fn view(state: &State, view: ViewId, preview: &PreviewState) -> Element<Msg> {
    // Отказ вытесняет канву: показывать поверх мёртвого кадра нечего, а
    // причина отказа — единственное, что тут можно сообщить. Неполный кадр
    // сюда не относится — он живой, и говорит о себе полоса внизу.
    let body: Element<Msg> = match preview.failure() {
        Some(error) => theme::empty(error).into(),
        None => canvas(view, preview),
    };

    column![
        toolbar(state, view, preview),
        theme::hairline(theme::LINE_SOFT),
        container(body).background(theme::CHROME).width(Length::Fill).height(Length::Fill),
        theme::hairline(theme::LINE),
        properties(state, preview),
    ]
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

/// Место под кадр канвы. Текстуру область получает от нас же — мы выделили её
/// в ответ на прошлый on_resized; на первом кадре её ещё нет, и место стоит
/// пустым, пока событие не приедет.
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

/// Имя снимка и масштаб. Ход показа сюда не входит: он не про снимок, а про
/// то, что с ним сейчас происходит, и место ему в полосе состояния внизу.
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

    // Подпись масштаба — она же кнопка «вписать»: отдельной кнопке тут места
    // нет, а подпись и так говорит, что показано сейчас.
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

/// Строка хода показа. `None` — показывать нечего: канва не занята и ни на что
/// не жалуется.
///
/// Жалоба старше хода: «читается…» рядом с недоехавшей ступенью говорило бы,
/// что всё идёт своим чередом, — а оно как раз не идёт.
fn progress_line(preview: &PreviewState) -> Option<String> {
    if preview.request.is_pending() {
        return Some("открывается…".to_string());
    }
    let view = preview.view_state.as_ref()?;
    // Причина — от канвы, а сколько её показать, решаем мы: в полосе она
    // соседствует со свойствами и, не будучи укорочена, вытолкнула бы их.
    // Целиком она всегда в логе.
    // Как есть: причина приезжает уже сказанной словами («кадр застыл: …»,
    // «производство: …»), и своя приставка тут только врала бы — застывший
    // кадр не «неполный».
    if !view.trouble.is_empty() {
        return Some(format::ellipsize(&view.trouble, 66));
    }
    if !view.working {
        return None;
    }
    if view.read_bytes > 0 && view.total_bytes > 0 {
        return Some(format!(
            "читается… {} из {}",
            format::bytes(view.read_bytes),
            format::bytes(view.total_bytes),
        ));
    }
    Some("готовится…".to_string())
}

/// Полоса под кадром: чем снимок является слева, что с ним сейчас происходит
/// справа.
///
/// Размеры — от канвы (описал тайлер); размер и время на диске — из библиотеки,
/// и только у скачанного: за удалённым записи нет. Ничего из этого не
/// вычисляется здесь — полоса только называет.
fn properties(state: &State, preview: &PreviewState) -> Element<Msg> {
    let entry = preview.entry.as_ref().and_then(|name| {
        state.library.entries.iter().find(|entry| &entry.name == name)
    });

    let mut facts: Vec<String> = Vec::new();
    if let Some(view) = &preview.view_state {
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
            facts.push(format!("скачан {}", format::date(entry.modified, format::now())));
        }
    }

    let mut parts: Vec<Element<Msg>> = vec![
        mono::<Msg>(facts.join("   ·   "))
            .size(theme::TEXT_SMALL)
            .color(theme::INK_SOFT)
            .single_line()
            .into(),
        theme::spacer().into(),
    ];
    // Ход показа — справа, у края: он меняется на глазах, и рядом со свойствами
    // дёргал бы их с места.
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
