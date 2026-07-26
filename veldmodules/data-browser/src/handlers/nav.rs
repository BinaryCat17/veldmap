use crate::module::state::{State, Screen, downloaded::{LocalFile, DATA_DIR, file_path, origin_path}};
use veldsdk::proto::fs::FsListRequest;
use crate::proto::ui_service::proto::UiEventResponse;

pub fn on_nav_browse(state: &mut State, _event: UiEventResponse) {
    state.current_screen = Screen::Browse;
    super::browse::request_path(state, state.browse.current_path.clone());
}

pub fn on_nav_search(state: &mut State, _event: UiEventResponse) {
    state.current_screen = Screen::Search;
}

pub fn on_nav_downloaded(state: &mut State, _event: UiEventResponse) {
    // Уход с экрана превью (его Back ведёт сюда): текстура освобождается.
    state.preview.clear();
    state.current_screen = Screen::Downloaded;
    request_list(state);
}

/// Перечитать каталог. Единственный способ обновить снимок — вызывается при
/// заходе на экран и после каждого терминального события (закачка кончилась,
/// файл удалён). Раньше вместо этого снимок «дополняли» правками из четырёх
/// хендлеров, и любой пропущенный путь оставлял UI врать до следующего
/// случайного листинга.
pub fn request_list(state: &mut State) {
    let correlation_id = state.downloaded.pending_list.begin(());
    crate::calls::fs::on_list(&FsListRequest {
        path: DATA_DIR.to_string(),
        correlation_id,
    });
}

/// Обработчик результата сканирования ФС. Broadcast-топик — сверяем
/// correlation_id, чтобы не принять устаревший или чужой ответ.
pub fn on_list_result(state: &mut State, response: veldsdk::proto::fs::FsListResult) {
    if state.downloaded.pending_list.take(&response.correlation_id).is_none() {
        return;
    }
    if !response.error.is_empty() {
        state.global.error_message = Some(format!("Failed to list files: {}", response.error));
        return;
    }

    // Снимок — только факты диска, без домыслов о происхождении файлов.
    state.downloaded.snapshot = response.entries.iter()
        // .origin — служебный сидкар, он описывает файл, а не является им.
        .filter(|e| !e.name.ends_with(".origin"))
        .map(|e| {
            let is_partial = e.name.ends_with(".part");
            LocalFile {
                path: file_path(&e.name),
                // Имя без .part — под ним файл известен сидкару и под ним же
                // появится после докачки.
                name: e.name.strip_suffix(".part").unwrap_or(&e.name).to_string(),
                size: e.size,
                is_partial,
            }
        }).collect();

    // origins — кэш сидкаров, а не независимая истина: подрезаем под то, что
    // реально лежит на диске, иначе файл, удалённый мимо приложения, остался
    // бы строкой-намерением до конца сессии. Исключение — сидкары, чья запись
    // ещё в полёте: на диске их закономерно нет, и срезать их значило бы
    // потерять строку только что начатой закачки.
    let on_disk: Vec<String> = response.entries.iter()
        .filter_map(|e| e.name.strip_suffix(".origin"))
        .map(str::to_string)
        .collect();
    let in_flight: Vec<&str> = state.downloaded.pending_sidecar_writes.values()
        .map(|w| w.filename.as_str())
        .collect();
    state.downloaded.origins.retain(|name, _| {
        on_disk.contains(name) || in_flight.contains(&name.as_str())
    });

    // Дочитываем то, чего ещё нет в памяти. Сидкар — единственное, что
    // переживает рестарт, так что для файлов с прошлого запуска это
    // единственный способ узнать remote-ключ и ожидаемый размер.
    for name in on_disk {
        if state.downloaded.origins.contains_key(&name) { continue; }
        if state.downloaded.pending_origin_reads.values().any(|n| n == &name) { continue; }
        let correlation_id = state.downloaded.pending_origin_reads.begin(name.clone());
        crate::calls::fs::on_read(&veldsdk::proto::fs::FsReadRequest {
            path: origin_path(&name),
            correlation_id,
        });
    }
}
