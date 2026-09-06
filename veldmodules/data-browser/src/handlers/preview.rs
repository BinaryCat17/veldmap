//! Вид предпросмотра: открыть файл ресурсом, отдать его канве (image-view) и
//! переводить жесты над ней в намерения камеры.
//!
//! Ресурс открываем мы, а не канва: она не должна знать, лежит файл на диске
//! или на той стороне сети, — и не знает, потому что оба открывателя отвечают
//! общим `core.ResourceOpened`, а дальше разницы нет. Владение уходит канве
//! вместе с on_show: показ переживает наш обработчик.
//!
//! Жест наш, камера — её: какой кнопкой тащат и что делает колесо, видно
//! только отсюда, а во что превращается сдвиг — знает только та, у кого
//! камера (ровно то же разделение, что у глобуса).
//!
//! Единицы при этом другие, чем у глобуса, и это не оплошность: наружу уходят
//! пиксели канвы и готовый множитель, а не щелчки. Множитель — потому что тем
//! же полем правят и кнопки `±` тулбара, а договорись мы «в щелчках», тулбару
//! пришлось бы знать шаг колеса, к которому он отношения не имеет.
//!
//! «Чей это ответ» и «актуален ли он» — два разных вопроса: на первый
//! отвечает таблица маршрутов `State::previews`, на второй — `Latest` внутри
//! вида. Вкладку могли закрыть, пока ответ шёл, и тогда ответ наш, а
//! показывать его негде.

use crate::module::state::{Open, State, ViewId, ViewKind};
use crate::proto::data_provider::{ImageryResponse, ImageryRole};
use crate::proto::image_view::{
    camera_command::Command, CameraCommand, Canvas, Fit, Pan, ShowRequest, VariableRequest, ViewState, ZoomAt,
};
use crate::proto::ui_service::{PointerAction, PointerEvent, ViewportSize};
use veldsdk::proto::core::ResourceOpened;

// Во сколько раз шаг колеса меняет масштаб — из того же файла, которым шаг
// считает шар: колесо одно, и щелчок у него один. Щелчок приезжает долями с
// инерцией, так что итоговое приближение — плавная степень этого числа.
use veldmap_globe_wrap::wheel::ZOOM_PER_CLICK;

/// Шаг кнопок ± в тулбаре.
const ZOOM_STEP: f32 = 1.5;

/// Просмотр скачанного файла: открывает библиотека — файл её, и где он лежит,
/// знает только она.
pub fn on_view_local_pressed(state: &mut State, from: ViewId, name: String) {
    if name.is_empty() { return; }

    let correlation_id = begin_open(state, from, name.clone(), Some(name.clone()));
    super::open_resource(name.clone(), Some(name), &correlation_id);
}

/// Просмотр ещё не скачанного. Ресурс открывает data-provider (подписать
/// запрос к хранилищу может только он); дальше путь тот же, что у локального:
/// по проводу идут только те окна файла, которые конвейер тайлов действительно
/// прочитал.
pub fn on_view_remote_pressed(state: &mut State, from: ViewId, identifier: String) {
    if identifier.is_empty() { return; }

    let correlation_id = begin_open(state, from, identifier.clone(), None);
    super::open_resource(identifier, None, &correlation_id);
}

/// Показать снимок, лежащий папкой. Прямо его не открыть: `GET` по пути
/// каталога отвечает 404, а какой из лежащих внутри растров показывать —
/// раскладка хранилища, и знает её только провайдер. Поэтому здесь лишний ход:
/// спрашиваем растры, вкладку заводим сразу.
///
/// Вкладка заводится до ответа намеренно: ход к каталогу занимает секунды, и
/// молчание в ответ на нажатие читается как «не сработало».
pub fn on_view_product_pressed(state: &mut State, from: ViewId, identifier: String) {
    if identifier.is_empty() { return; }

    state.close_menus();
    let pane = super::nav::pane_of(state, from);
    let view = super::nav::open_preview(state, pane, identifier.clone(), None);
    let correlation_id = state.preview_mut(view).expect("вид только что открыт").begin();
    state.preview_imagery.insert(correlation_id.clone(), view);
    crate::calls::data_provider::on_imagery(
        &crate::proto::data_provider::ImageryRequest {
            downloaded: state.library.whole(&identifier),
            identifier,
        },
        &correlation_id,
    );
}

/// Растры снимка приехали. `false` — ответ не наш (его ждало наложение).
///
/// Из двух ролей берём подробную: смотрят снимок затем, чтобы разглядеть, а
/// квиклук для этого мал. Нет подробной — показываем что есть: маленькая
/// картинка лучше пустой вкладки с отказом.
pub fn on_imagery_result(state: &mut State, response: &ImageryResponse) -> bool {
    let correlation_id = veldsdk::correlation();
    let Some(view) = state.preview_imagery.take(&correlation_id) else { return false };
    let Some(preview) = state.preview_mut(view) else { return true };
    if preview.request.settle(&correlation_id) != veldsdk::Reply::Current {
        return true;
    }

    let raster = response
        .rasters
        .iter()
        .find(|raster| raster.role == ImageryRole::ImageryDetailed as i32)
        .or_else(|| response.rasters.first());
    let Some(raster) = raster else {
        preview.error = Some(match response.error.is_empty() {
            true => "внутри снимка нечего показать".to_string(),
            false => response.error.clone(),
        });
        return true;
    };

    let identifier = raster.identifier.clone();

    // Чем открывать растр, решается тем же правилом, что у строки списка и у
    // наложения на шаре (`LibraryState::local_name`): скачанное открывает
    // библиотека, потому что файл под рукой. Спрашивается это здесь, а не на
    // нажатии: нажимают на снимок, а на диске лежит не он, а растр внутри
    // него, и называет растр только этот ответ.
    let local = state.library.local_name(&identifier).map(str::to_string);

    let Some(preview) = state.preview_mut(view) else { return true };
    preview.label = identifier.clone();
    // Запись библиотеки помнится и здесь: по ней вкладка, восстановленная из
    // раскладки, откроется с диска сразу, не повторяя хода к провайдеру.
    preview.entry = local.clone();
    let correlation_id = preview.begin();
    state.previews.insert(correlation_id.clone(), view);
    super::open_resource(identifier, local, &correlation_id);
    true
}

/// Открыть просмотр заново — так возвращается вкладка из сохранённой раскладки
/// (см. handlers::persist).
///
/// Ход тот же, что по нажатию, и различаются они ровно двумя вещами: панель
/// названа прямо (у восстанавливаемой вкладки нет строки, из которой её
/// позвали) и снимок берётся тем, чем он был подписан. Скачанный открывает
/// библиотека, прочее — провайдер: это и есть весь смысл `entry`.
pub fn reopen(
    state: &mut State,
    pane: crate::module::state::PaneId,
    label: String,
    entry: Option<String>,
    variable: String,
) {
    if label.is_empty() {
        return;
    }
    let view = super::nav::open_preview(state, pane, label.clone(), entry.clone());
    let preview = state.preview_mut(view).expect("вид только что открыт");
    preview.variable = variable;
    let correlation_id = preview.begin();
    state.previews.insert(correlation_id.clone(), view);
    super::open_resource(label, entry, &correlation_id);
}

/// Общее начало обоих путей: новая вкладка и корреляция, по которой её найдёт
/// ответ открывателя.
fn begin_open(state: &mut State, from: ViewId, label: String, entry: Option<String>) -> String {
    // Меню строки закрываем сами — по той же причине, что и показ на шаре:
    // просмотр уводит с этого экрана, а открытым оно осталось бы до
    // возвращения.
    state.close_menus();
    let pane = super::nav::pane_of(state, from);
    let view = super::nav::open_preview(state, pane, label, entry);
    let correlation_id = state.preview_mut(view)
        .expect("вид только что открыт")
        .begin();
    state.previews.insert(correlation_id.clone(), view);
    correlation_id
}

/// Ресурс открыт — неважно кем: библиотекой (скачанный файл) или провайдером
/// (читается по сети). Владение уходит канве вместе с именем вида; отсюда
/// показ ведёт она. `false` — ответ не наш.
///
/// Устаревший ответ (вкладку закрыли или запрос вытеснили) всё равно наш:
/// ресурс уже принадлежит нам, и бросить его значит потерять и регион, и
/// открытый на той стороне дескриптор.
pub fn on_resource_opened(state: &mut State, opened: &ResourceOpened) -> bool {
    let correlation_id = veldsdk::correlation();
    let Some(view) = state.previews.take(&correlation_id) else { return false };

    let current = match state.preview_mut(view) {
        Some(preview) => preview.request.settle(&correlation_id) == veldsdk::Reply::Current,
        // Вкладку закрыли, пока ответ шёл.
        None => false,
    };

    if !current {
        if let Some(handle) = &opened.handle {
            veldsdk::resource::release(handle.clone());
        }
        return true;
    }

    let resource = match veldsdk::resource::accept(opened) {
        Ok(handle) => handle,
        Err(error) => {
            fail(state, view, error);
            return true;
        }
    };

    // Передача владения — до on_show: получив событие, канва вправе сразу
    // считать ресурс своим. При отказе хелпер уже освободил его сам.
    let resource = match veldsdk::resource::hand_off(resource, "image-view") {
        Ok(handle) => handle,
        Err(error) => {
            fail(state, view, error);
            return true;
        }
    };

    let Some(preview) = state.preview_mut(view) else { return true };
    crate::calls::image_view::on_show(&ShowRequest {
        view: view.to_string(),
        resource: Some(resource),
        label: preview.label.clone(),
        variable: preview.variable.clone(),
    });
    true
}

/// Неудача открытия: на экран и в лог. Экран видит только тот, кто на него
/// смотрит, а причина (истёкшая подпись, нет записи) нужна и после того, как
/// пользователь ушёл на другую вкладку.
fn fail(state: &mut State, view: ViewId, error: String) {
    let Some(preview) = state.preview_mut(view) else { return };
    veldsdk::log::warn!(target: "handlers", "превью '{}': {}", preview.label, error);
    preview.error = Some(error);
}

/// Канве активного превью досталось новое место. Пока размер тот же, ничего
/// не делаем — перевыделение сменило бы id текстуры на каждый пересчёт
/// разметки (см. то же у глобуса).
pub fn on_resized(state: &mut State, view: ViewId, size: ViewportSize) {
    let Some(preview) = state.preview_mut(view) else { return };
    if veldsdk::surface::Delegated::covers(preview.surface.as_ref(), size.width, size.height) {
        return;
    }
    place(state, view, size.width, size.height);
}

/// Выделяет канве место под кадр и отдаёт его ей.
///
/// Отдельно от [`on_resized`] потому, что зовут её двое: событие области — с
/// новым размером, и ответ канвы «места нет» — с прежним (см.
/// [`on_view_state`]). Второму ждать события бессмысленно: оно приезжает
/// только на смену размера, а размер не менялся.
fn place(state: &mut State, view: ViewId, width: u32, height: u32) {
    let scale = state.scale;
    let Some(preview) = state.preview_mut(view) else { return };
    let key = view.to_string();
    preview.surface = veldsdk::surface::delegate(
        preview.surface.take(),
        width,
        height,
        scale,
        super::SURFACE_FORMAT as i32,
        "image-view",
        // Рисует канва, показывает разметка: право чтения — её рендереру.
        &["ui-service"],
        |surface| {
            crate::calls::image_view::on_canvas(&Canvas {
                view: key.clone(),
                surface: Some(surface.clone()),
            });
        },
    );
}

/// Указатель над канвой: тащат — панорама, колесо — масштаб вокруг курсора.
/// Правая и средняя кнопки свободны — как и у глобуса, занимать их «пока
/// чем-нибудь» значит переучивать потом.
pub fn on_pointer(state: &mut State, view: ViewId, event: PointerEvent) {
    let Some(preview) = state.preview_mut(view) else { return };
    if preview.surface.is_none() {
        return;
    }
    let view = view.to_string();

    match event.action() {
        PointerAction::PointerPressed if event.button == 1 => {
            preview.dragging = Some(crate::module::state::preview::Drag { last: (event.x, event.y) });
        }
        PointerAction::PointerMoved => {
            let Some(drag) = &mut preview.dragging else { return };
            let (dx, dy) = (event.x - drag.last.0, event.y - drag.last.1);
            drag.last = (event.x, event.y);
            if dx != 0.0 || dy != 0.0 {
                send(&view, Command::Pan(Pan { dx, dy }));
            }
        }
        PointerAction::PointerReleased | PointerAction::PointerLeft => {
            preview.dragging = None;
        }
        PointerAction::PointerScrolled => {
            if event.scroll_y != 0.0 {
                send(&view, Command::ZoomAt(ZoomAt {
                    x: event.x,
                    y: event.y,
                    factor: ZOOM_PER_CLICK.powf(f64::from(event.scroll_y)) as f32,
                }));
            }
        }
        _ => {}
    }
}

/// Кнопки тулбара: вписать и шаг масштаба вокруг центра канвы.
pub fn on_fit(state: &mut State, view: ViewId) {
    let Some(_) = state.preview_mut(view) else { return };
    send(&view.to_string(), Command::Fit(Fit {}));
}

pub fn on_zoom_step(state: &mut State, view: ViewId, direction: f32) {
    let Some(preview) = state.preview_mut(view) else { return };
    let Some(surface) = &preview.surface else { return };
    let factor = if direction > 0.0 { ZOOM_STEP } else { 1.0 / ZOOM_STEP };
    send(&view.to_string(), Command::ZoomAt(ZoomAt {
        x: surface.width as f32 / 2.0,
        y: surface.height as f32 / 2.0,
        factor,
    }));
}

/// Список величин под кадром: раскрыть, закрыть щелчком мимо или повторным
/// нажатием. Раскрытое одно на экран (см. `state::Open`).
pub fn on_variables(state: &mut State, view: ViewId, open: bool) {
    match open {
        true => state.toggle_open(Open::Variables(view)),
        false => state.close_menus(),
    }
}

/// Другая величина файла — по выбору в списке под кадром: канва описывает
/// ресурс заново с ней. Уже показанная или уже названная — нечего делать,
/// кроме как закрыть список.
pub fn on_variable(state: &mut State, view: ViewId, variable: String) {
    state.close_menus();
    let Some(preview) = state.preview_mut(view) else { return };
    let shown = preview.view_state.as_ref().and_then(|state| state.variable.as_ref()).map(|shown| shown.path.as_str());
    if shown == Some(variable.as_str()) || preview.variable == variable {
        return;
    }
    preview.variable = variable.clone();
    crate::calls::image_view::on_variable(&VariableRequest { view: view.to_string(), variable });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::module::state::Open;
    use crate::proto::image_view::Variable;

    fn state_with_preview() -> (State, ViewId) {
        let mut state = State::new(crate::module::handlers::Config { initial_view: None }).expect("состояние");
        let pane = state.focused();
        let view = super::super::nav::open_preview(&mut state, pane, "снимок".to_string(), None);
        (state, view)
    }

    /// Выбор величины закрывает список, запоминается у вкладки и не
    /// повторяется для показанной: выбранная по умолчанию тайлером остаётся
    /// его выбором (пустым), а не становится названной.
    #[test]
    fn выбор_величины_запоминается_и_не_повторяется() {
        let (mut state, view) = state_with_preview();
        state.toggle_open(Open::Variables(view));
        // Канва показывает выбор тайлера — щелчок по нему в списке ничего не
        // называет: описание заново было бы тем же самым.
        state.preview_mut(view).expect("вкладка").view_state = Some(ViewState {
            variable: Some(Variable { path: "/PRODUCT/co".to_string(), ..Default::default() }),
            ..Default::default()
        });
        on_variable(&mut state, view, "/PRODUCT/co".to_string());
        assert!(matches!(state.open, Open::Nothing), "список не закрылся");
        assert_eq!(state.preview_mut(view).expect("вкладка").variable, "", "показанная названа заново");

        on_variable(&mut state, view, "/PRODUCT/qa_value".to_string());
        assert_eq!(state.preview_mut(view).expect("вкладка").variable, "/PRODUCT/qa_value");
    }

    /// Список величин переживает отказ канвы: отчёт без списка прежнего не
    /// стирает, отчёт со списком — заменяет.
    #[test]
    fn список_величин_переживает_отказ_канвы() {
        let (mut state, view) = state_with_preview();
        let listed = |paths: &[&str]| ViewState {
            view: view.to_string(),
            variables: paths.iter().map(|path| Variable { path: path.to_string(), ..Default::default() }).collect(),
            ..Default::default()
        };
        on_view_state(&mut state, listed(&["/a", "/b"]));
        on_view_state(&mut state, ViewState { view: view.to_string(), error: "пуста".to_string(), ..Default::default() });
        let names = |state: &mut State| {
            state.preview_mut(view).expect("вкладка").variables.iter().map(|v| v.path.clone()).collect::<Vec<_>>()
        };
        assert_eq!(names(&mut state), ["/a", "/b"], "отказ стёр список");
        on_view_state(&mut state, listed(&["/c"]));
        assert_eq!(names(&mut state), ["/c"]);
    }
}

/// Рассылка канвы о ходе показа. Чей это вид, сказано в самом сообщении;
/// вкладку могли уже закрыть — тогда правда никому здесь не нужна.
pub fn on_view_state(state: &mut State, view_state: ViewState) {
    let Ok(view) = view_state.view.parse::<ViewId>() else { return };
    let Some(ViewKind::Preview(preview)) = state.get_mut(view) else { return };
    // Канва говорит, что места под кадр у неё нет. Выдать его заново может
    // только владелец разметки — то есть мы, — и никто нас об этом больше не
    // попросит: `on_resized` приезжает лишь на смену размера, а размер не
    // менялся. Поэтому выдаём прямо здесь.
    //
    // Без этого канва оставалась бы пустой до тех пор, пока человек не потянет
    // границу панели, — а причина отказа бывает мгновенной: текстуру сменили,
    // пока событие шло.
    //
    // Одна попытка на жалобу: мгновенный отказ лечится первой же, а
    // устойчивый (не хватило видеопамяти) не лечится повтором вовсе — и без
    // счёта мы с канвой гоняли бы текстуры по кругу (см. `replaced`). Счёт
    // взводится там же, где попытка и делается: жалоба без выданного места
    // (его ещё ни разу не выдавали) попытки не тратит.
    let retry = view_state.needs_place && !preview.replaced;
    if !view_state.needs_place {
        preview.replaced = false;
    }
    // Список величин остаётся от последнего описания, назвавшего его: канва,
    // отказавшая названной величине, шлёт пустой, а выбрать другую нужно как
    // раз тогда.
    if !view_state.variables.is_empty() {
        preview.variables = view_state.variables.clone();
    }
    let size = preview.surface.as_ref().map(|place| (place.width, place.height));
    let label = preview.label.clone();
    preview.view_state = Some(view_state);
    if let (true, Some((width, height))) = (retry, size) {
        veldsdk::log::info!(target: "handlers",
            "превью '{}': место под кадр не собралось — выдаём заново", label);
        if let Some(preview) = state.preview_mut(view) {
            preview.replaced = true;
        }
        place(state, view, width, height);
    }
}

fn send(view: &str, command: Command) {
    crate::calls::image_view::on_camera(&CameraCommand {
        view: view.to_string(),
        command: Some(command),
    });
}
