//! Действия над библиотекой: скачать/пауза, удалить — и приём её состояния.
//!
//! Здесь нет ни путей, ни сидкаров, ни знания о том, как на диске отличается
//! недокачанное: всё это делает data-library. Модуль только переводит нажатие
//! в запрос к ней, а состояние получает рассылкой.

use crate::proto::data_library::{DownloadRequest, ItemRequest, LibraryState as LibraryStateMsg};

use crate::module::state::State;
use crate::module::state::library::status_of;
use crate::proto::data_library::LibraryStatus;

/// Пользователь нажал «скачать». Повторное нажатие на идущую закачку —
/// пауза: скачанное сохраняется, следующее нажатие продолжит с него.
pub fn on_download_pressed(state: &mut State, identifier: String, product: String) {
    if identifier.is_empty() { return; }

    let downloading = state.library.by_identifier(&identifier)
        .filter(|e| status_of(e) == LibraryStatus::LibDownloading)
        .map(|e| e.name.clone());

    if let Some(name) = downloading {
        crate::calls::data_library::on_pause(&ItemRequest { name });
        return;
    }

    crate::calls::data_library::on_download(&DownloadRequest { identifier, product });
}

/// Пользователь нажал «удалить» — на любой записи библиотеки (полной,
/// недокачанной или заявленной одним лишь намерением). Идущую закачку это
/// заодно отменяет; отдельного «отменить» нет, потому что оставить после себя
/// он обязан то же самое — ничего (см. меню строки в components::table).
pub fn on_delete_pressed(_state: &mut State, name: String) {
    if name.is_empty() { return; }

    crate::calls::data_library::on_delete(&ItemRequest { name });
}

/// Показать запись в файловом менеджере. Путь считает библиотека: раскладка
/// хранения её, и знать её нам незачем.
pub fn on_reveal_pressed(_state: &mut State, name: String) {
    if name.is_empty() { return; }

    crate::calls::data_library::on_reveal(&ItemRequest { name });
}

/// Выбросить снимок целиком. Библиотека про снимки не знает — она ведёт учёт
/// файлам, — поэтому разворачиваем его здесь, в том же месте, где строку
/// снимка и собрали (`downloaded_rows`), и просим удалить каждый её файл.
///
/// Отдельного «удали всё по снимку» в контракте нет намеренно: он потребовал
/// бы от библиотеки знать границу снимка, а знает её провайдер, и второй
/// носитель этого факта разошёлся бы с первым.
pub fn on_delete_snapshot(state: &mut State, product: String) {
    if product.is_empty() { return; }

    let names: Vec<String> = state
        .library
        .entries
        .iter()
        .filter(|entry| entry.product == product)
        .map(|entry| entry.name.clone())
        .collect();
    for name in names {
        crate::calls::data_library::on_delete(&ItemRequest { name });
    }
}

/// Библиотека прислала состояние — своё или в ответ на наш запрос. Второго
/// источника правды о скачанном у нас нет, поэтому просто заменяем.
///
/// Отказ показывается до следующего удачного состояния и им же снимается:
/// сообщение, которое некому погасить, висит на экране до перезапуска и врёт
/// тем дольше, чем дольше работает приложение.
pub fn on_state(state: &mut State, msg: LibraryStateMsg) {
    if !msg.error.is_empty() {
        state.error = Some(format!("Library: {}", msg.error));
        return;
    }
    state.error = None;
    state.library.entries = msg.entries;
    measure_speed(state);
}

/// Скорость закачки — из двух соседних состояний: сколько байт прибавилось за
/// сколько секунд. Своего поля под неё у библиотеки нет, и быть не должно —
/// она рассылает то, что есть сейчас, а не то, как быстро оно росло.
///
/// Замер сбрасывается, когда качать нечего: скорость закончившейся закачки —
/// это уже не скорость, а последнее увиденное число.
fn measure_speed(state: &mut State) {
    let (count, done, _) = state.library.downloading();
    if count == 0 {
        state.speed = 0.0;
        state.measured = None;
        return;
    }

    let now = crate::module::components::format::now();

    if let Some((measured_at, measured_done)) = state.measured {
        let seconds = now - measured_at;
        // Того же мгновения мало: разделив на ноль секунд, получим не скорость,
        // а бесконечность. Прошлый замер при этом сохраняется — следующее
        // состояние сравнится уже с ним.
        if seconds > 0 {
            state.speed = done.saturating_sub(measured_done) as f32 / seconds as f32;
            state.measured = Some((now, done));
        }
        return;
    }
    state.measured = Some((now, done));
}
