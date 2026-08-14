//! Действия над библиотекой: скачать/пауза, удалить — и приём её состояния.
//!
//! Здесь нет ни путей, ни сидкаров, ни знания о том, как на диске отличается
//! недокачанное: всё это делает data-library. Модуль только переводит нажатие
//! в запрос к ней, а состояние получает рассылкой.

use crate::proto::data_library::{
    DownloadRequest, ItemRequest, LibraryState as LibraryStateMsg, SnapshotFiles,
};
use crate::proto::data_provider::{ListPathRequest, ListPathResponse};

use crate::module::state::{Listing, State};
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

/// Скачать снимок целиком. Библиотека качает по файлу — снимка она не знает
/// вовсе, — поэтому сперва спрашиваем провайдера, из чего снимок состоит.
///
/// Рекурсивным листингом, а не обходом по ярусам: файлы .SAFE разложены в
/// четыре-пять уровней, и обход стоил бы запроса на каждый.
pub fn on_download_snapshot(state: &mut State, product: String) {
    if product.is_empty() { return; }

    let path = crate::module::components::folder_path(&product);
    let correlation_id = state.listings.begin(Listing::Snapshot { product, files: 0, queued: 0 });
    crate::calls::data_provider::on_list_path(
        &ListPathRequest { path, token: String::new(), recursive: true },
        &correlation_id,
    );
}

/// Снимок и то, что о нём насчитано страницами: обход идёт по одной, а оба
/// числа — итоговые.
pub struct Counted {
    pub product: String,
    /// Сколько файлов снимка уже перечислено.
    pub files: u32,
    /// Сколько из них поставлено в закачку.
    pub queued: usize,
}

/// Файлы снимка приехали — ставим в закачку то, чего на диске ещё нет, под тем
/// снимком, из которого их позвали.
///
/// Папок в рекурсивном листинге не бывает (см. `ListPathRequest.recursive`),
/// поэтому отсеивать по роду нечего: всё, что пришло, — файлы.
pub fn on_snapshot_files(
    state: &mut State,
    counted: Counted,
    correlation_id: String,
    response: ListPathResponse,
) {
    let Counted { product, mut files, mut queued } = counted;
    if !response.error.is_empty() {
        state.notice = Some(format!("Снимок «{}» не перечислился: {}", product, response.error));
        return;
    }

    files += response.entries.len() as u32;
    for entry in response.entries {
        // Уже доведённое не трогаем. Перекачка сносит готовый файл до старта
        // (см. data-library::download), поэтому «докачать снимок» на снимке,
        // где половина уже на диске, стирало бы ровно то, что в нём есть, — и
        // качало бы гигабайты заново. Перекачивают по одному файлу, из его
        // меню, где это и помечено как необратимое.
        let done = state
            .library
            .by_identifier(&entry.key)
            .is_some_and(|record| status_of(record) == LibraryStatus::LibComplete);
        if done {
            continue;
        }
        crate::calls::data_library::on_download(&DownloadRequest {
            identifier: entry.key,
            product: product.clone(),
        });
        queued += 1;
    }

    // Страница не последняя — дочитываем той же корреляцией. Потолок тот же,
    // что у обхода папки: снимок конечен, но раскладка, в которой он не
    // кончается, не должна превращаться в бесконечную закачку. Молча
    // обрывать её нельзя — недокачанный снимок с виду ничем не отличается от
    // целого.
    let short = product.rsplit('/').next().unwrap_or(&product).to_string();
    if response.next_token.is_empty() {
        veldsdk::log::info!(target: "handlers", "снимок '{}': {} файлов, в закачке {}", product, files, queued);
        // Обход дошёл до конца — значит, состав снимка известен целиком, и это
        // единственный момент, когда его можно назвать. Библиотека без этого
        // числа считает полным всякий снимок, у которого доведено всё, что
        // качали, — три файла из двадцати шести читались бы как «на диске».
        crate::calls::data_library::on_snapshot(&SnapshotFiles { product, files });
        state.notice = Some(match (files, queued) {
            // Обход кончился, не встретив ни файла. «Уже на диске» тут — ложь:
            // на диске ничего нет, и качать хранилищу тоже нечего.
            (0, _) => format!("В снимке «{}» нет файлов", short),
            (_, 0) => format!("Снимок «{}» уже на диске", short),
            (_, n) => format!("Снимок «{}»: в закачке {} файлов", short, n),
        });
        return;
    }
    if queued >= super::browse::MAX_ITEMS {
        // Состав снимка здесь не называем: обход оборван, и перечисленное —
        // не весь снимок. Сказать библиотеке насчитанное значило бы объявить
        // полным ровно тот снимок, который мы не дочитали.
        state.notice =
            Some(format!("Снимок «{}»: поставлено {} файлов, остальные пропущены", short, queued));
        return;
    }

    let path = crate::module::components::folder_path(&product);
    state.listings.insert(correlation_id.clone(), Listing::Snapshot { product, files, queued });
    crate::calls::data_provider::on_list_path(
        &ListPathRequest { path, token: response.next_token, recursive: true },
        &correlation_id,
    );
}

/// Приостановить закачку снимка целиком.
///
/// Разворачивается он так же, как при удалении: библиотека снимков не знает, а
/// файлов у одного бывает под три десятка, и жать паузу на каждом — работа,
/// которую приложение обязано сделать само. Приостанавливается только идущее:
/// пауза на доведённом файле означала бы «начать и остановить».
pub fn on_pause_snapshot(state: &mut State, product: String) {
    if product.is_empty() { return; }

    let names: Vec<String> = state
        .library
        .entries
        .iter()
        .filter(|entry| entry.product == product)
        .filter(|entry| status_of(entry) == LibraryStatus::LibDownloading)
        .map(|entry| entry.name.clone())
        .collect();
    for name in names {
        crate::calls::data_library::on_pause(&ItemRequest { name });
    }
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
