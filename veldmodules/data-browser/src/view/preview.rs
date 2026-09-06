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

use veld_ui_service_wrap::{column, popover, row, viewport};
use crate::proto::ui_service::{
    container, mono, text, Alignment, Element, Length, Padding,
};
use crate::module::components::{format, menu};
use crate::module::state::{PreviewState, State, ViewId};
use crate::module::{theme, Msg, ViewMsg};

/// Сколько места оставить имени в тулбаре: всё, кроме кнопок масштаба.
const CONTROLS_WIDTH: f32 = 230.0;

/// Сколько ширины полосы свойств отдано ходу показа справа: он появляется и
/// исчезает на ходу, и величина, занявшая его место, толкала бы его за край.
const PROGRESS_WIDTH: f32 = 260.0;

/// Меньше этого знаков величине не оставляют: имя с единицами должно быть
/// узнаваемо и в узкой панели, пусть и срезанным.
const VARIABLE_CHARS_FLOOR: usize = 16;

/// Разделитель фактов полосы; его ширина входит в счёт места под величину.
const FACT_SEPARATOR: &str = "   ·   ";

/// Сколько величин файла перечисляет раскрытый список; остальные — числом.
/// У гранулы Sentinel-5P годных три десятка, и панель длиннее окна не
/// раскрыть. Семнадцать строк списка — около 520 pt по метрикам
/// `menu::line`: в окно в 768 pt встают; в панель деления в половину окна —
/// нет, там всплывашка упирается в край и накрывает кнопку, оставаясь
/// закрываемой щелчком мимо.
const VARIABLES_LISTED: usize = 16;

/// Что кнопка величины добавляет к ширине своего текста: отступы по краям и
/// зазор полосы до следующего факта.
const BUTTON_ROOM: f32 = 8.0 + 8.0 + crate::module::view::BAR_SPACING;

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
        properties(state, view, preview),
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
/// Жалоба старше хода: ход рядом с недоехавшей ступенью говорил бы, что всё
/// идёт своим чередом, — а оно как раз не идёт.
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
    // Ход называется тем же словарём, что и у снимка на шаре: работа у них
    // одна и та же — набирается пирамида тайлов, — и два способа рассказать о
    // ней заставляли бы смотрящего гадать, не разные ли это вещи.
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

/// Полоса под кадром: чем снимок является слева, что с ним сейчас происходит
/// справа.
///
/// Размеры — от канвы (описал тайлер); размер и время на диске — из библиотеки,
/// и только у скачанного: за удалённым записи нет. Ничего из этого не
/// вычисляется здесь — полоса только называет.
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
            facts.push(format!("скачан {}", format::date(entry.modified, format::now())));
        }
    }

    let mut parts: Vec<Element<Msg>> = Vec::new();
    // Чем снимок является — первым: у файла многих величин размеры без имени
    // величины не говорят, что́ показано. Ей достаётся то, что остальные факты
    // и место под ход показа оставили от ширины панели — полоса одной строкой
    // и не переносится. Слова файла бывают в полторы строки и встают только
    // целиком: обрезанные, они читались бы хуже, чем имя с единицами без них.
    // Величина — кнопка: под ней список всего, из чего выбирали.
    if let Some((shown, variables)) = view_state.and_then(|view| view.variable.as_ref().map(|shown| (shown, &view.variables))) {
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
        parts.push(
            popover(anchor, open, || menu::panel(variable_items(view, variables, &shown.path)))
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

/// Пункты списка величин файла: все, из кого выбирали, в порядке тайлера,
/// показанная — с галочкой. Пока только список: пункт закрывает его, выбора
/// другой величины за ним нет — запросы тайлера величины не несут. Длинный
/// список обрезается числом ([`VARIABLES_LISTED`]), но показанной место
/// гарантировано: обрезанная, она уходила бы в «и ещё N», и список из одних
/// не показанных читался бы как «показана ни одна из них».
fn variable_items(
    view: ViewId,
    variables: &[crate::proto::image_view::Variable],
    shown: &str,
) -> Vec<menu::Item> {
    let item = |variable: &crate::proto::image_view::Variable| {
        menu::Item::new(
            format::variable(&variable.path, &variable.said, &variable.units),
            Msg::In(view, ViewMsg::PreviewVariables(false)),
        )
        .selected(variable.path == shown)
    };
    let mut items: Vec<menu::Item> = variables.iter().take(VARIABLES_LISTED).map(item).collect();
    if let Some(at) = variables.iter().position(|variable| variable.path == shown).filter(|&at| at >= VARIABLES_LISTED) {
        items.truncate(VARIABLES_LISTED - 1);
        items.push(item(&variables[at]));
    }
    if variables.len() > VARIABLES_LISTED {
        items.push(menu::Item::note(format!("и ещё {}", variables.len() - VARIABLES_LISTED)));
    }
    items
}

#[cfg(test)]
mod tests {
    use super::{variable_items, VARIABLES_LISTED};
    use crate::module::state::ViewId;
    use crate::proto::image_view::Variable;

    fn variable(path: &str, units: &str) -> Variable {
        Variable { path: path.to_string(), said: String::new(), units: units.to_string() }
    }

    /// Список называет все величины в порядке тайлера, отмечает показанную и
    /// обрезает длинный хвост числом.
    #[test]
    fn the_list_names_every_variable_and_marks_the_shown_one() {
        let view: ViewId = "7".parse().expect("ViewId из строки");
        let few = [variable("/PRODUCT/a", "K"), variable("/PRODUCT/b", "")];
        let items = variable_items(view, &few, "/PRODUCT/b");
        assert_eq!(items.iter().map(|item| item.named()).collect::<Vec<_>>(), ["a, K", "b"]);
        assert_eq!(items.iter().map(|item| item.marked()).collect::<Vec<_>>(), [false, true]);

        let many: Vec<Variable> = (0..VARIABLES_LISTED + 5).map(|at| variable(&format!("/v{at}"), "")).collect();
        let items = variable_items(view, &many, "/v0");
        assert_eq!(items.len(), VARIABLES_LISTED + 1);
        assert_eq!(items.last().map(|item| item.named()), Some("и ещё 5"));
        assert!(items[0].marked() && !items[1].marked());

        // Показанная за обрезкой встаёт последней из перечисленных — с галочкой.
        let items = variable_items(view, &many, "/v20");
        assert_eq!(items.len(), VARIABLES_LISTED + 1);
        assert_eq!(items[VARIABLES_LISTED - 1].named(), "v20");
        assert!(items[VARIABLES_LISTED - 1].marked() && !items[0].marked());
        assert_eq!(items.last().map(|item| item.named()), Some("и ещё 5"));
    }
}
