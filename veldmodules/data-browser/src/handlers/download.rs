use crate::proto::data_provider::{DownloadRequest, DownloadStarted, DownloadProgress, Downloaded, CancelDownloadRequest};
use crate::proto::ui_service::proto::UiEventResponse;

use crate::module::state::{State, downloaded::{OriginSidecar, PROVIDER_NAME, filename_from_key}};
use crate::module::components::task_manager::TaskKind;

/// Пользователь нажал кнопку скачать.
/// Повторное нажатие на файл, который уже скачивается — отмена загрузки.
pub fn on_download_pressed(
    state: &mut State,
    event: UiEventResponse,
) {
    let s3_key = event.value;
    let filename = filename_from_key(&s3_key);

    if s3_key.is_empty() { return; }

    // Отмена активной загрузки: data-provider пришлёт Downloaded{success:false},
    // и on_downloaded снимет задачу с панели. TaskManager — единственный
    // источник "качается ли s3_key сейчас", отдельной таблицы не заводим.
    if let Some(task) = state.global.task_manager.active_download(&s3_key) {
        let task_id = task.id.clone();
        veldsdk::vinfo!(veldsdk::FLAG_UI_HANDLERS, "[data-browser] on_download_pressed: cancelling task={} for s3_key={}", task_id, s3_key);
        state.global.status_message = format!("Cancelling download: {}", filename);
        crate::calls::data_provider::on_cancel_download(&CancelDownloadRequest { task_id });
        return;
    }
    veldsdk::vinfo!(veldsdk::FLAG_UI_HANDLERS, "[data-browser] on_download_pressed: starting new download for s3_key={}", s3_key);

    // Origin — сразу, до ответа data-provider: даже если приложение упадёт
    // на первом байте, .part на диске уже будет знать, откуда он взялся.
    // total_bytes — если он уже известен из прошлой попытки в этой же
    // сессии (докачка после паузы), сразу пишем и его — не нужно ждать
    // первого progress-события заново, чтобы узнать то, что уже знаем.
    let known_total = state.downloaded.known_total_bytes.get(&filename).copied();
    write_origin_sidecar(state, &filename, &s3_key, known_total);

    crate::calls::data_provider::on_download(&DownloadRequest {
        identifier: s3_key,
        destination: format!("data/dem/source/{}", filename),
    });
}

/// Пишет origin-sidecar (`<имя>.origin`) через fs/on_write — durable-копия
/// known_origins и known_total_bytes, переживающая рестарт приложения (см.
/// handlers::nav — восстановление читает её обратно для файлов без
/// known_origins в памяти, и on_read_result ниже, который заодно поднимает
/// total_bytes). `total_bytes: None` — Content-Length ещё не увиден в этой
/// сессии (обычный случай при первом старте закачки).
fn write_origin_sidecar(state: &mut State, filename: &str, identifier: &str, total_bytes: Option<u64>) {
    state.downloaded.known_origins.insert(filename.to_string(), identifier.to_string());

    let sidecar = OriginSidecar { provider: PROVIDER_NAME.to_string(), identifier: identifier.to_string(), total_bytes };
    let Ok(json) = serde_json::to_vec(&sidecar) else { return };

    let Some(region_id) = veldsdk::abi::arena_alloc_cpu(json.len() as u64) else { return };
    veldsdk::abi::arena_write(region_id, 0, &json);

    // Гранта на "fs" не нужно (и он бы не сработал: fs — хостовый нативный
    // модуль, не wasm-плагин, dispatcher.instance_of его не резолвит — эта
    // таблица заполняется только для wasm-плагинов, см. plugins.rs). on_write
    // проверяет доступ по requestor_id — паблишеру события, то есть нам же,
    // а мы и так владелец региона: доступ есть по праву владения.
    let correlation_id = state.downloaded.pending_sidecar_writes.begin(region_id);
    crate::calls::fs::on_write(&veldsdk::proto::fs::FsWriteRequest {
        path: format!("data/dem/source/{}.origin", filename),
        handle: Some(veldsdk::proto::core::ResourceHandle { id: region_id, size: json.len() as u64, content_hash: Vec::new() }),
        correlation_id,
    });
}

/// fs прочитал наш буфер (успешно или нет) — регион больше не нужен.
pub fn on_write_result(state: &mut State, response: veldsdk::proto::fs::FsWriteResult) {
    let Some(region_id) = state.downloaded.pending_sidecar_writes.take(&response.correlation_id) else { return; };
    veldsdk::abi::arena_free(region_id);
    if !response.error.is_empty() {
        veldsdk::vwarn!(veldsdk::FLAG_SDK, "[data-browser] failed to persist origin sidecar: {}", response.error);
    }
}

/// Ответ на fs/on_read origin-sidecar'а, запрошенный handlers::nav для файла
/// без known_origins в памяти (переживший рестарт .part или скачанный файл).
/// Отсутствие sidecar (файл скачан до появления этой фичи) — не ошибка,
/// просто re-download/докачка для него останутся недоступны.
pub fn on_read_result(state: &mut State, response: veldsdk::proto::fs::FsReadResult) {
    let Some(filename) = state.downloaded.pending_origin_reads.take(&response.correlation_id) else { return; };
    let Some(handle) = response.handle else { return; };

    let bytes = veldsdk::abi::arena_read(handle.id, 0, handle.size);
    veldsdk::abi::arena_free(handle.id);
    let Some(bytes) = bytes else { return; };

    let Ok(sidecar) = serde_json::from_slice::<OriginSidecar>(&bytes) else { return; };
    if sidecar.provider != PROVIDER_NAME { return; }

    state.downloaded.known_origins.insert(filename.clone(), sidecar.identifier.clone());
    if let Some(total) = sidecar.total_bytes {
        state.downloaded.known_total_bytes.insert(filename.clone(), total);
    }
    if let Some(f) = state.downloaded.local_files.iter_mut().find(|f| f.name == filename) {
        f.origin_key = Some(sidecar.identifier);
    }
}

/// Пользователь нажал кнопку просмотра
pub fn on_view_pressed(
    state: &mut State,
    event: UiEventResponse,
) {
    let value = event.value;
    if value.is_empty() { return; }

    state.current_screen = crate::module::state::Screen::Preview;
    state.preview.current_path = value;
    state.preview.is_loading = false;
    // TODO: wasm-модуль image ещё не реализован — загрузка превью появится вместе с ним.
    state.global.error_message = Some("Image preview: модуль image ещё не реализован".to_string());
}

/// Data-provider сообщил что загрузка началась
pub fn on_download_started(
    state: &mut State,
    event: DownloadStarted,
) {
    let filename = filename_from_key(&event.identifier);

    state.global.task_manager.spawn(
        event.task_id.clone(),
        TaskKind::Download { 
            task_id: event.task_id.clone(),
            s3_key: event.identifier.clone(), 
            filename: filename.clone(),
        }
    );
    
    // Строка в Downloaded — сразу, не дожидаясь ни следующего захода на экран
    // (снимок fs/on_list берётся только при навигации), ни появления .part на
    // диске (сетевой запрос к CDSE может занять секунды до первых байт) — всё
    // нужное для строки уже известно из этого события.
    if !state.downloaded.local_files.iter().any(|f| f.name == filename) {
        state.downloaded.local_files.push(crate::module::state::downloaded::LocalFile {
            path: format!("data/dem/source/{}.part", filename),
            name: filename.clone(),
            size: 0,
            origin_key: Some(event.identifier),
            is_partial: true,
        });
    }

    state.global.status_message = format!("Starting download: {}", filename);
    // Рендер происходит автоматически в on_frame
}

pub fn on_download_progress(
    state: &mut State,
    event: DownloadProgress,
) {
    // TODO(debug): временно, снять после диагностики "прогресс не виден
    // в реальном времени" — проверяем, доходит ли событие вообще и с
    // какими значениями.
    veldsdk::vinfo!(veldsdk::FLAG_UI_HANDLERS, "[data-browser] download_progress task={} progress={} bytes={}/{}", event.task_id, event.progress, event.downloaded_bytes, event.total_bytes);
    state.global.task_manager.update_download_progress(
        &event.task_id, event.progress, event.downloaded_bytes, event.total_bytes,
    );

    // Content-Length переживает паузу: TaskManager после отмены помечает
    // задачу finished и активный прогресс из неё больше не читают (см.
    // browser_list::view — Incomplete-строка не видит TaskInfo), а
    // known_total_bytes — обычная карта по имени файла, живёт независимо
    // от жизненного цикла задачи, так что "Incomplete" продолжит знать
    // "из скольки", пока закачка на паузе. Пишем это и в sidecar (не при
    // каждом событии — только когда узнали новое значение), иначе после
    // рестарта приложения total снова неизвестен, пока не начнётся новая
    // закачка в новой сессии: sidecar на диске это единственное, что
    // переживает перезапуск, known_total_bytes — только память.
    if event.total_bytes > 0 {
        let target = state.global.task_manager.get(&event.task_id).and_then(|t| match &t.kind {
            TaskKind::Download { s3_key, filename, .. } => Some((s3_key.clone(), filename.clone())),
            _ => None,
        });
        if let Some((s3_key, filename)) = target {
            if state.downloaded.known_total_bytes.get(&filename) != Some(&event.total_bytes) {
                state.downloaded.known_total_bytes.insert(filename.clone(), event.total_bytes);
                write_origin_sidecar(state, &filename, &s3_key, Some(event.total_bytes));
            }
        }
    }
    // Рендер происходит автоматически в on_frame
}

pub fn on_downloaded(
    state: &mut State,
    event: Downloaded,
) {
    // s3_key/filename этой задачи — только в TaskManager (TaskKind::Download),
    // отдельной таблицы не держим. Байты снимаем тут же: после finish()
    // задача остаётся в Correlator (до cleanup_completed), но нам нужен
    // именно последний прогресс, а не гадать по нему после.
    let (filename, bytes_downloaded, total_bytes) = match state.global.task_manager.get(&event.task_id) {
        Some(task) => match &task.kind {
            TaskKind::Download { filename, .. } => (filename.clone(), task.bytes_downloaded, task.total_bytes),
            _ => return,
        },
        None => return,
    };
    state.global.task_manager.finish(&event.task_id);

    // Корзина нажималась во время закачки — тогда это не отмена ради отмены,
    // а отложенный delete (см. on_delete_pressed): .part на диске остаётся
    // ровно там, где abort его бросил (см. network::download.rs), теперь его
    // можно безопасно удалить.
    if let Some(path) = state.downloaded.pending_delete_on_cancel.take(&event.task_id) {
        delete_local_file(state, path);
        state.global.status_message = format!("Deleted: {}", filename);
        return;
    }

    if event.success {
        // known_origins для filename уже выставлен в write_origin_sidecar
        // при старте закачки — здесь просто финальный статус. Запись в
        // local_files тоже правим сразу (не дожидаясь следующего захода на
        // экран, симметрично оптимистичному push в on_download_started) —
        // иначе .part-путь и is_partial:true висят в UI до следующего
        // fs/on_list, хотя файл на диске уже переименован в конечное имя.
        if let Some(f) = state.downloaded.local_files.iter_mut().find(|f| f.name == filename) {
            f.path = format!("data/dem/source/{}", filename);
            f.is_partial = false;
            f.size = if total_bytes > 0 { total_bytes } else { bytes_downloaded };
        }
        state.global.status_message = format!("Downloaded: {}", filename);
    } else {
        state.global.error_message = Some(format!("Download failed: {}", event.error));
        state.global.task_manager.fail(&event.task_id, event.error);
    }
    // Рендер происходит автоматически в on_frame
}

/// Пользователь нажал "удалить" — на любом локальном файле (полном или
/// недокачанном), на обоих экранах (Browse/Downloaded используют один и
/// тот же browser_list-компонент, см. components::browser_list).
pub fn on_delete_pressed(
    state: &mut State,
    event: UiEventResponse,
) {
    let path = event.value;
    if path.is_empty() { return; }

    // Файл прямо сейчас качается — host держит `.part` открытым на запись
    // (см. network::download.rs), удалять поверх активной записи нельзя.
    // Сначала отменяем закачку; сам delete сработает в on_downloaded, когда
    // придёт подтверждение отмены.
    let active = state.downloaded.local_files.iter()
        .find(|f| f.path == path)
        .and_then(|f| Some((f.name.clone(), f.origin_key.as_deref()?.to_string())))
        .and_then(|(name, key)| state.global.task_manager.active_download(&key).map(|t| (name, t.id.clone())));

    if let Some((filename, task_id)) = active {
        veldsdk::vinfo!(veldsdk::FLAG_UI_HANDLERS, "[data-browser] on_delete_pressed: active download, cancelling task={} before delete of {}", task_id, path);
        state.downloaded.pending_delete_on_cancel.insert(task_id.clone(), path);
        state.global.status_message = format!("Cancelling download: {}", filename);
        crate::calls::data_provider::on_cancel_download(&CancelDownloadRequest { task_id });
        return;
    }

    veldsdk::vinfo!(veldsdk::FLAG_UI_HANDLERS, "[data-browser] on_delete_pressed: deleting {} immediately (no active download)", path);
    delete_local_file(state, path);
}

/// Sidecar живёт под "чистым" именем (без .part) — удаляем вместе с
/// файлом, чтобы не копить сироты; результат не отслеживаем, отсутствие
/// sidecar (например, файл скачан до этой фичи) — не ошибка.
fn delete_local_file(state: &mut State, path: String) {
    let base = path.strip_suffix(".part").unwrap_or(&path);
    crate::calls::fs::on_delete(&veldsdk::proto::fs::FsDeleteRequest {
        path: format!("{}.origin", base),
        correlation_id: String::new(),
    });

    let correlation_id = state.downloaded.pending_delete.begin(path.clone());
    crate::calls::fs::on_delete(&veldsdk::proto::fs::FsDeleteRequest { path, correlation_id });
}

/// Broadcast-топик — сверяем correlation_id, чтобы не принять устаревший
/// или чужой ответ.
pub fn on_delete_result(
    state: &mut State,
    response: veldsdk::proto::fs::FsDeleteResult,
) {
    let Some(path) = state.downloaded.pending_delete.take(&response.correlation_id) else { return; };

    if !response.error.is_empty() {
        state.global.error_message = Some(format!("Failed to delete {}: {}", path, response.error));
        return;
    }
    state.downloaded.local_files.retain(|f| f.path != path);
}
