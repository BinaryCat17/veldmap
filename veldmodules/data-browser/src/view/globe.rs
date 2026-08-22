//! Вкладка глобуса: область во весь экран и подпись под ней.
//!
//! Своего содержимого у вкладки почти нет — всё, что видно, рисует чужой
//! модуль. Здесь только место под него и то, что вокруг.

use veld_ui_service_wrap::{column, row, viewport};

use crate::proto::ui_service::{container, mono, text, Alignment, Element, Length};
use crate::module::components::{self, format};
use crate::module::state::{GlobeState, State, ViewId};
use crate::module::{theme, Msg, ViewMsg};


/// Чем управляют шаром. Величина, а не строка на месте: её ширину меряет
/// [`hint_fits`], и разойтись им нельзя.
const HINT: &str = "Тащите — вращает, колесо — приближает к курсору, щелчок — выбирает";

/// Помещается ли подсказка в полосу вместе со всем, что важнее её.
///
/// Важнее — размер области и кнопка «Снять с шара»: первое говорит, куда
/// смотрят, вторая делает дело, а подсказка лишь напоминает, чем вращают шар.
/// Правило то же, что у имени снимка ([`picked_chars`]): место кнопкам полоса
/// отводит первым.
fn hint_fits(width: f32, clearable: bool) -> bool {
    let mut taken = format::text_width(HINT, theme::TEXT_LABEL)
        + format::text_width("область ещё не размечена", theme::TEXT_LABEL)
        + crate::module::view::BAR_SPACING
        + theme::GUTTER * 2.0;
    if clearable {
        taken += format::text_width("Снять с шара", theme::TEXT_SMALL)
            + BUTTON_PAD
            + crate::module::view::BAR_SPACING;
    }
    taken <= width
}

/// Поля кнопки по обе стороны — те же, что ставит `theme::bar_button`.
const BUTTON_PAD: f32 = 8.0 * 2.0;

/// Сколько знаков имени помещается в полосу шириной `width`.
///
/// Считается, а не назначается числом: имя укорачивает клиент
/// (`format::ellipsize`) — способа сказать тексту «сожмись» в этом протоколе
/// нет, — и названное с запасом оно выдавит за край кнопки, то единственное,
/// ради чего в эту полосу и смотрят. Постоянным числом это не выражается:
/// полоса бывает и во весь экран, и в половину.
///
/// Кнопки меряются по своим подписям — своей ширины у них нет, они её и есть,
/// — и считаются все три, даже когда показаны не все: лишний запас укорачивает
/// имя, недостача выдавливает кнопку.
fn picked_chars(width: f32) -> usize {
    const LABELS: [&str; 3] = ["Смотреть", "В каталоге", "Снять с шара"];
    let buttons: f32 = LABELS
        .iter()
        .map(|label| {
            format::text_width(label, theme::TEXT_SMALL)
                + BUTTON_PAD
                + crate::module::view::BAR_SPACING
        })
        .sum();
    format::mono_fit((width - buttons - theme::GUTTER * 2.0).max(0.0), theme::TEXT_SMALL)
}

/// Снимок, о котором говорит полоса: чем его подписать, чем адресовать и каким
/// способом смотреть. Три поля, а не тройка: имя и ключ у снимка похожи с виду,
/// и перепутать их местами в тройке ничто не мешает.
struct Subject<'a> {
    label: &'a str,
    key: &'a str,
    folder: bool,
}

pub fn view(state: &State, view: ViewId, globe: &GlobeState) -> Element<Msg> {
    // Текстуру область получает от нас же: мы её выделили в ответ на
    // предыдущий on_resized. На первом кадре её ещё нет — место занимается
    // пустым, и это нормально: следующим событием оно придёт.
    let mut area = viewport::<Msg>()
        .width(Length::Fill)
        .height(Length::Fill)
        .on_resized(move |size| Msg::In(view, ViewMsg::GlobeResized(size)))
        .on_pointer(move |pointer| Msg::In(view, ViewMsg::GlobePointer(pointer)));
    if let Some(surface) = &globe.surface {
        area = area.texture(surface.handle());
    }

    column![
        container(area).width(Length::Fill).height(Length::Fill),
        theme::hairline(theme::LINE),
        caption(state, view, globe),
    ]
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

/// Полоса под областью: чем управляют слева, что на шаре справа.
///
/// Названный снимок вытесняет размер места: пока не выбрано и не наложено
/// ничего, размер — то единственное, что можно сказать о безымянной области,
/// а как только снимок назван, он и есть ответ на вопрос «на что я смотрю».
/// Рядом с именем — то, что с ним делают, глядя на шар: посмотреть его самого
/// и найти его в каталоге. У наложений «снять» — разом все; по одному их
/// снимают в «На просмотре», здесь кнопка на случай, когда той вкладки не
/// открыто.
fn caption(state: &State, view: ViewId, globe: &GlobeState) -> Element<Msg> {
    // Что назвать: выбранный щелчком контур, а без него — верхний слой на шаре.
    // Обычно это один и тот же снимок; расходятся они, когда щёлкнули по
    // соседнему контуру, и тогда именем отвечает выбор — он свежее.
    //
    // Верхний из тех, что видно: слоёв бывает несколько, назвать одной строкой
    // можно только один, а собирающийся или скрытый назвал бы то, чего на шаре
    // нет.
    // Выбранный ищется и среди слоёв: контура у снимка может не быть вовсе —
    // геометрию каталог знает не про всё, а отметку с него могли снять, — и
    // тогда назвать его больше нечем. Молчаливый откат к верхнему слою здесь
    // хуже всего: полоса называет не тот снимок, к которому только что привели,
    // и её кнопки уводят тоже не к нему.
    let picked = state.picked_key();
    let layer_named = |key: &str| {
        state.overlays.iter().find(move |overlay| overlay.identifier == key).map(|overlay| {
            Subject { label: &overlay.label, key: &overlay.identifier, folder: overlay.folder }
        })
    };
    let subject: Option<Subject<'_>> = state
        .picked()
        .map(|outlined| Subject {
            label: &outlined.label,
            key: &outlined.key,
            folder: outlined.folder,
        })
        .or_else(|| layer_named(picked))
        .or_else(|| {
            state
                .overlays
                .iter()
                .rev()
                .find(|overlay| overlay.on_globe() && !overlay.hidden)
                .map(|overlay| Subject {
                    label: &overlay.label,
                    key: &overlay.identifier,
                    folder: overlay.folder,
                })
        });

    let named = subject.is_some();
    let mut trailing: Vec<Element<Msg>> = Vec::new();
    match subject {
        Some(subject) => {
            // Имя занимает всё, что осталось от полосы: место кнопкам разметка
            // отводит первым, и никакое имя их за край не вытолкнет.
            trailing.push(
                container(
                    mono::<Msg>(format::ellipsize(subject.label, picked_chars(state.pane_width(view))))
                        .size(theme::TEXT_SMALL)
                        .color(theme::INK_SOFT)
                        .single_line(),
                )
                .width(Length::Fill)
                .align_x(Alignment::End)
                .into(),
            );
            trailing.push(
                theme::bar_button("Смотреть")
                    .on_press(Msg::In(
                        view,
                        components::preview_of(&state.library, subject.key, subject.folder),
                    ))
                    .into(),
            );
            trailing.push(
                theme::bar_button("В каталоге")
                    .on_press(Msg::In(view, ViewMsg::InCatalog(subject.key.to_string())))
                    .into(),
            );
        }
        None => {
            let label = match &globe.surface {
                Some(surface) => format!("{}×{}", surface.width, surface.height),
                None => "область ещё не размечена".to_string(),
            };
            trailing.push(
                text::<Msg>(label).size(theme::TEXT_LABEL).color(theme::INK_FAINT).single_line().into(),
            );
        }
    }
    // Контуры считаются наравне с растрами: очерченный шар — тоже «что-то на
    // нём лежит», и снимать это надо тем же рычагом.
    if !state.overlays.is_empty() || !state.outlined.is_empty() {
        trailing.push(theme::bar_button("Снять с шара").on_press(Msg::GlobeClear).into());
    }

    // Подсказка уступает место названному снимку — как и размер области выше:
    // что делают со снимком, важнее того, чем вращают шар. И кнопке уступает
    // тоже, если вдвоём они в полосу не влезли. Уступает целиком, а не
    // ужимается: огрызок «Тащите — враща…» не научит ничему и всё равно
    // отберёт место у того единственного, ради чего в эту полосу и смотрят.
    let mut parts: Vec<Element<Msg>> = Vec::new();
    if !named {
        if hint_fits(state.pane_width(view), trailing.len() > 1) {
            parts.push(
                text::<Msg>(HINT.to_string())
                    .size(theme::TEXT_LABEL)
                    .color(theme::INK_DIM)
                    .single_line()
                    .into(),
            );
        }
        // Распорка нужна только здесь: при названном снимке остаток полосы
        // занимает его имя, и второй тянущийся элемент отобрал бы у него
        // половину места.
        parts.push(theme::spacer().into());
    }
    parts.extend(trailing);

    theme::chrome_bar(
        row(parts)
            .spacing(crate::module::view::BAR_SPACING)
            .width(Length::Fill)
            .height(Length::Fill)
            .align_items(Alignment::Center),
    )
    .into()
}

#[cfg(test)]
mod tests {
    use super::{hint_fits, picked_chars};

    /// Имени достаётся то, что осталось от кнопок, — и в узкой панели не
    /// остаётся ничего. Ноль здесь честнее любого минимума: полоса, в которой
    /// имени нет места, обязана показать кнопки целыми, а огрызок имени в две
    /// буквы не назовёт снимка и всё равно отберёт у них место.
    #[test]
    fn the_name_gives_the_bar_buttons_their_room_first() {
        let wide = picked_chars(1600.0);
        let half = picked_chars(800.0);
        assert!(wide > half, "широкая полоса вмещает больше: {} против {}", wide, half);
        assert_eq!(picked_chars(260.0), 0, "кнопки занимают полосу целиком");
        assert!(half > 0, "в половине экрана имя ещё помещается");
    }

    /// Подсказка уступает полосу тому, что делает дело. Проверяется на ширине
    /// панели, поделённой пополам: там подсказка с кнопкой не уживаются, и
    /// уйти обязана подсказка — обрезанная кнопка не нажимается.
    #[test]
    fn the_hint_gives_the_bar_button_its_room_first() {
        // Ширины логические, как и `pane_width`: окно 2048 физических пикселей
        // при двукратном масштабе — это тысяча с небольшим, половина панели —
        // пятьсот.
        assert!(hint_fits(1024.0, true), "в целой полосе помещаются оба");
        assert!(!hint_fits(508.0, true), "подсказка выдавила кнопку из узкой полосы");
        assert!(hint_fits(700.0, false), "без кнопки подсказке хватает и половины");
    }
}
