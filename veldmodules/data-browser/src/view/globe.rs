//! Вкладка глобуса: область во весь экран и подпись под ней.
//!
//! Своего содержимого у вкладки почти нет — всё, что видно, рисует чужой
//! модуль. Здесь только место под него и то, что вокруг.

use veld_ui_service_wrap::{column, row, viewport};

use crate::proto::ui_service::{container, mono, text, Alignment, Element, Length};
use crate::module::components::{self, format};
use crate::module::state::{GlobeState, State, ViewId};
use crate::module::{theme, Msg, ViewMsg};


/// Сколько знаков имени выбранного снимка помещается в подпись. Числом, а не
/// шириной: укорачивает имя многоточие в середине (см. `format::ellipsize`), а
/// не разметка, — способа сказать тексту «сожмись» в этом протоколе нет.
///
/// Столько, сколько остаётся от полосы после трёх кнопок в панели шириной в
/// пол-экрана: имя продукта длиной под семьдесят знаков целиком не показать
/// всё равно, а вылезшее за край утащило бы за собой кнопки — то единственное,
/// ради чего в эту полосу и смотрят.
const PICKED_CHARS: usize = 34;

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
                    mono::<Msg>(format::ellipsize(subject.label, PICKED_CHARS))
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
    // что делают со снимком, важнее того, чем вращают шар. Уступает целиком, а
    // не ужимается: в полосе узкой панели вместе они не помещаются, и
    // выдавливает подсказка как раз кнопки — то единственное, ради чего в эту
    // полосу и смотрят.
    let mut parts: Vec<Element<Msg>> = Vec::new();
    if !named {
        parts.push(
            text::<Msg>("Тащите — вращает, колесо — приближает, щелчок — выбирает".to_string())
                .size(theme::TEXT_LABEL)
                .color(theme::INK_DIM)
                .single_line()
                .into(),
        );
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
