//! view/downloaded.rs — экран скачанных файлов

use veld_ui_service_wrap::{column, Keyed};
use crate::proto::ui_service::{text, Element, Length};
use crate::module::state::State;
use crate::module::components::{Row, RowStatus, downloaded_rows, render_list, list_screen, ItemActions};
use crate::module::{styles, Msg};

pub fn view(state: &State) -> Element<Msg> {
    let rows = downloaded_rows(&state.library);
    let (pending, complete): (Vec<&Row>, Vec<&Row>) = rows.iter()
        .partition(|r| !matches!(r.status, RowStatus::Complete { .. }));

    let actions = ItemActions {
        browse: None,
        view_local: Some(Msg::ViewLocal),
        view_remote: None, // всё уже на диске — смотреть удалённо нечего

        download: Some(Msg::Download), // Докачка/перезакачка
        delete: Some(Msg::Delete),
    };

    // Незавершённые и полные рендерятся одним и тем же компонентом, что и
    // Browse — это тот же файл в том же виде, секции только группируют.
    let section = |title: &str, color, items: Vec<&Row>| -> Option<Element<Msg>> {
        // Секция названа своим заголовком: «Incomplete» исчезает, когда
        // недокачанных не осталось, и «Complete» встаёт на её место.
        (!items.is_empty()).then(|| column![
            text(title.to_string()).size(14.0).color(color).single_line(),
            render_list(items, actions),
        ].spacing(8.0).width(Length::Fill).key(title.to_string()))
    };

    // Заголовок симметричен "Incomplete" (тот же spacing(8.0) до своего
    // списка): без подписи граница между секциями читалась как случайно
    // больший зазор между строками, а не как начало новой группы. Обе секции
    // живут в одном body — если незавершённых много, они тоже скроллятся.
    let incomplete = section("Incomplete", styles::COLOR_WARNING, pending);
    let complete = section("Complete", styles::COLOR_SUCCESS, complete)
        .unwrap_or_else(|| text("No downloaded files found").size(16.0).into());

    let title: Element<Msg> = text("Local Files").size(20.0).into();
    let body: Element<Msg> = column(incomplete.into_iter().chain([complete]))
        .spacing(15.0)
        .width(Length::Fill)
        .into();

    list_screen(vec![title], body)
}
