//! Действия над библиотекой: скачать/пауза, удалить — и приём её состояния.
//!
//! Здесь не осталось ни путей, ни сидкаров, ни знания о том, как на диске
//! отличается недокачанное: всё это делает data-library. Модуль только
//! переводит нажатие в запрос к ней, а состояние получает рассылкой.

use crate::proto::data_library::{DownloadRequest, ItemRequest, LibraryState as LibraryStateMsg};
use crate::proto::ui_service::proto::UiEventResponse;

use crate::module::state::State;
use crate::module::state::library::status_of;
use crate::proto::data_library::LibraryStatus;

/// Пользователь нажал «скачать». Повторное нажатие на идущую закачку —
/// пауза: скачанное сохраняется, следующее нажатие продолжит с него.
pub fn on_download_pressed(state: &mut State, event: UiEventResponse) {
    let identifier = event.value;
    if identifier.is_empty() { return; }

    let downloading = state.library.by_identifier(&identifier)
        .filter(|e| status_of(e) == LibraryStatus::LibDownloading)
        .map(|e| e.name.clone());

    if let Some(name) = downloading {
        state.global.status_message = format!("Cancelling download: {}", name);
        crate::calls::data_library::on_cancel(&ItemRequest { name });
        return;
    }

    state.global.status_message = format!("Starting download: {}", identifier);
    crate::calls::data_library::on_download(&DownloadRequest { identifier });
}

/// Пользователь нажал «удалить» — на любой записи библиотеки (полной,
/// недокачанной или заявленной одним лишь намерением).
pub fn on_delete_pressed(state: &mut State, event: UiEventResponse) {
    let name = event.value;
    if name.is_empty() { return; }

    state.global.status_message = format!("Deleting: {}", name);
    crate::calls::data_library::on_delete(&ItemRequest { name });
}

/// Библиотека прислала состояние — своё или в ответ на наш запрос. Второго
/// источника правды о скачанном у нас нет, поэтому просто заменяем.
pub fn on_state(state: &mut State, msg: LibraryStateMsg) {
    if !msg.error.is_empty() {
        state.global.error_message = Some(format!("Library: {}", msg.error));
        return;
    }
    state.library.entries = msg.entries;
}
