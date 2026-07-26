use crate::proto::data_provider::{DownloadRequest, DownloadStarted, DownloadProgress, Downloaded, CancelDownloadRequest};
use crate::proto::ui_service::proto::UiEventResponse;

use crate::module::state::{State, downloaded::{
    Download, OriginSidecar, SidecarWrite, PROVIDER_NAME, filename_from_key, file_path, origin_path,
}};

/// Пользователь нажал кнопку скачать.
/// Повторное нажатие на файл, который уже скачивается — отмена загрузки.
pub fn on_download_pressed(
    state: &mut State,
    event: UiEventResponse,
) {
    let s3_key = event.value;
    let filename = filename_from_key(&s3_key);

    if s3_key.is_empty() { return; }

    // Отмена активной загрузки: data-provider пришлёт Downloaded{cancelled},
    // и on_downloaded снимет задачу. Реестр закачек — единственный источник
    // "качается ли s3_key сейчас", отдельной таблицы не заводим.
    if let Some((task_id, _)) = state.downloaded.active_download(&s3_key) {
        let task_id = task_id.to_string();
        veldsdk::vinfo!(veldsdk::FLAG_UI_HANDLERS, "[data-browser] on_download_pressed: cancelling task={} for s3_key={}", task_id, s3_key);
        state.global.status_message = format!("Cancelling download: {}", filename);
        crate::calls::data_provider::on_cancel_download(&CancelDownloadRequest { task_id });
        return;
    }
    veldsdk::vinfo!(veldsdk::FLAG_UI_HANDLERS, "[data-browser] on_download_pressed: starting new download for s3_key={}", s3_key);

    // Origin — сразу, до ответа data-provider: даже если приложение упадёт
    // на первом байте, `.origin` на диске уже будет знать, откуда он взялся,
    // и строка-намерение не пропадёт из списка (см. components::row).
    // total_bytes — если он уже известен из прошлой попытки, сразу пишем и
    // его: не нужно ждать первого progress-события, чтобы узнать известное.
    let known_total = state.downloaded.origins.get(&filename).and_then(|o| o.total_bytes);
    write_origin_sidecar(state, &filename, &s3_key, known_total);

    crate::calls::data_provider::on_download(&DownloadRequest {
        identifier: s3_key,
        destination: file_path(&filename),
    });
}

/// Пишет origin-sidecar (`<имя>.origin`) через fs/on_write. Сидкар — не кэш
/// ради скорости, а единственное, что переживает рестарт: `origins` в памяти
/// заполняется чтением именно этих файлов при листинге (см. handlers::nav).
fn write_origin_sidecar(state: &mut State, filename: &str, identifier: &str, total_bytes: Option<u64>) {
    state.downloaded.origins.insert(filename.to_string(), OriginSidecar {
        provider: PROVIDER_NAME.to_string(),
        identifier: identifier.to_string(),
        total_bytes,
    });

    let sidecar = OriginSidecar { provider: PROVIDER_NAME.to_string(), identifier: identifier.to_string(), total_bytes };
    let Ok(json) = serde_json::to_vec(&sidecar) else { return };

    let Some(region_id) = veldsdk::abi::arena_alloc_cpu(json.len() as u64) else { return };
    veldsdk::abi::arena_write(region_id, 0, &json);

    // Гранта на "fs" не нужно (и он бы не сработал: fs — хостовый нативный
    // модуль, не wasm-плагин, dispatcher.instance_of его не резолвит — эта
    // таблица заполняется только для wasm-плагинов, см. plugins.rs). on_write
    // проверяет доступ по requestor_id — паблишеру события, то есть нам же,
    // а мы и так владелец региона: доступ есть по праву владения.
    let correlation_id = state.downloaded.pending_sidecar_writes.begin(SidecarWrite {
        region: region_id,
        filename: filename.to_string(),
    });
    crate::calls::fs::on_write(&veldsdk::proto::fs::FsWriteRequest {
        path: origin_path(filename),
        handle: Some(veldsdk::proto::core::ResourceHandle { id: region_id, size: json.len() as u64, content_hash: Vec::new() }),
        correlation_id,
    });
}

/// fs прочитал наш буфер (успешно или нет) — регион больше не нужен.
pub fn on_write_result(state: &mut State, response: veldsdk::proto::fs::FsWriteResult) {
    let Some(write) = state.downloaded.pending_sidecar_writes.take(&response.correlation_id) else { return; };
    veldsdk::abi::arena_free(write.region);
    if !response.error.is_empty() {
        veldsdk::vwarn!(veldsdk::FLAG_SDK, "[data-browser] failed to persist origin sidecar: {}", response.error);
    }
}

/// Ответ на fs/on_read сидкара, запрошенный handlers::nav при листинге.
/// Отсутствие сидкара (файл скачан до появления этой фичи) — не ошибка,
/// просто re-download/докачка для него останутся недоступны.
pub fn on_read_result(state: &mut State, response: veldsdk::proto::fs::FsReadResult) {
    let Some(filename) = state.downloaded.pending_origin_reads.take(&response.correlation_id) else { return; };
    let Some(handle) = response.handle else { return; };

    let bytes = veldsdk::abi::arena_read(handle.id, 0, handle.size);
    veldsdk::abi::arena_free(handle.id);
    let Some(bytes) = bytes else { return; };

    let Ok(sidecar) = serde_json::from_slice::<OriginSidecar>(&bytes) else { return; };
    if sidecar.provider != PROVIDER_NAME { return; }

    state.downloaded.origins.insert(filename, sidecar);
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

/// Data-provider сообщил что загрузка началась. Строка в списке отсюда НЕ
/// добавляется: она выводится из реестра закачек (см. components::row),
/// поэтому регистрации задачи достаточно.
pub fn on_download_started(
    state: &mut State,
    event: DownloadStarted,
) {
    let filename = filename_from_key(&event.identifier);

    // Засеваем тем, что уже известно, а не нулями: до первого progress-события
    // строка рисуется из этой записи, и «0 B» на возобновлении недокачанного
    // файла было бы враньём — байты на диске уже есть, host продолжит именно
    // с них (resume_offset по `.part`, см. network::download.rs). Берём размер
    // ТОЛЬКО у недокачанной записи: у полной `.part` нет, re-download пойдёт
    // с нуля, и её размер выдал бы «361 MB / 361 MB» с падением на 0.
    let done = state.downloaded.entry_for(&filename)
        .filter(|e| e.is_partial)
        .map(|e| e.size)
        .unwrap_or(0);

    state.downloaded.downloads.insert(event.task_id, Download {
        s3_key: event.identifier,
        filename: filename.clone(),
        done,
        // Из сидкара, если Content-Length видели в прошлой попытке.
        total: state.downloaded.total_bytes(&filename),
    });
    state.global.status_message = format!("Starting download: {}", filename);
}

pub fn on_download_progress(
    state: &mut State,
    event: DownloadProgress,
) {
    // event.progress игнорируем — доля выводится из байт при отрисовке.
    let Some(dl) = state.downloaded.downloads.get_mut(&event.task_id) else { return; };
    dl.done = event.downloaded_bytes;
    dl.total = event.total_bytes;

    // Content-Length пишем в сидкар, когда узнали новое значение (не на каждом
    // событии): иначе после рестарта "из скольки" снова неизвестно, пока не
    // начнётся новая закачка — сидкар на диске это единственное, что
    // переживает перезапуск.
    if event.total_bytes > 0 {
        let (s3_key, filename) = (dl.s3_key.clone(), dl.filename.clone());
        if state.downloaded.total_bytes(&filename) != event.total_bytes {
            write_origin_sidecar(state, &filename, &s3_key, Some(event.total_bytes));
        }
    }
}

pub fn on_downloaded(
    state: &mut State,
    event: Downloaded,
) {
    // s3_key/filename этой закачки — только в реестре, отдельной таблицы не
    // держим. Снимаем с учёта сразу: реестр описывает идущие закачки, а эта
    // кончилась — дальше строку опишет снимок диска.
    let Some(dl) = state.downloaded.downloads.take(&event.task_id) else { return; };
    let filename = dl.filename;

    // Корзина нажималась во время закачки — тогда это не отмена ради отмены,
    // а отложенный delete (см. on_delete_pressed): `.part` остаётся ровно
    // там, где abort его бросил, теперь его можно безопасно удалить.
    if let Some(path) = state.downloaded.pending_delete_on_cancel.take(&event.task_id) {
        delete_local(state, path, &filename);
        state.global.status_message = format!("Deleted: {}", filename);
        return;
    }

    if event.success {
        state.global.status_message = format!("Downloaded: {}", filename);
    } else if event.cancelled {
        // Пауза — не ошибка: `.part` остаётся на диске, следующее "скачать"
        // продолжит с него. Красный баннер был бы враньём про нормальный жест.
        state.global.status_message = format!("Paused: {}", filename);
    } else {
        state.global.error_message = Some(format!("Download failed: {}", event.error));
    }

    // Перечитываем каталог: размер `.part`/готового файла берётся с диска, а
    // не переносится руками из реестра в строку. Ровно этот перенос раньше и
    // забывали на отдельных путях — отсюда "0 B" на паузе.
    super::nav::request_list(state);
}

/// Пользователь нажал "удалить" — на любом локальном файле (полном,
/// недокачанном или заявленном одним лишь сидкаром), на любом из экранов:
/// все они используют один и тот же components::file_list.
pub fn on_delete_pressed(
    state: &mut State,
    event: UiEventResponse,
) {
    let path = event.value;
    if path.is_empty() { return; }

    // Имя без .part — под ним живёт сидкар и под ним файл известен реестру.
    let filename = path.rsplit('/').next().unwrap_or(&path);
    let filename = filename.strip_suffix(".part").unwrap_or(filename).to_string();

    // Файл прямо сейчас качается — host держит `.part` открытым на запись,
    // удалять поверх активной записи нельзя. Сначала отменяем закачку; сам
    // delete сработает в on_downloaded, когда придёт подтверждение отмены.
    let active = state.downloaded.origin_key(&filename)
        .and_then(|key| state.downloaded.active_download(key))
        .map(|(task_id, _)| task_id.to_string());

    if let Some(task_id) = active {
        veldsdk::vinfo!(veldsdk::FLAG_UI_HANDLERS, "[data-browser] on_delete_pressed: active download, cancelling task={} before delete of {}", task_id, path);
        state.downloaded.pending_delete_on_cancel.insert(task_id.clone(), path);
        state.global.status_message = format!("Cancelling download: {}", filename);
        crate::calls::data_provider::on_cancel_download(&CancelDownloadRequest { task_id });
        return;
    }

    veldsdk::vinfo!(veldsdk::FLAG_UI_HANDLERS, "[data-browser] on_delete_pressed: deleting {} immediately (no active download)", path);
    delete_local(state, path, &filename);
}

/// Удаляет файл вместе с его сидкаром — иначе `.origin` остался бы сиротой и
/// файл воскрес бы строкой-намерением. `origins` правим сразу: он кэш диска,
/// и следующий листинг всё равно приведёт его к тому же виду.
fn delete_local(state: &mut State, path: String, filename: &str) {
    state.downloaded.origins.remove(filename);
    crate::calls::fs::on_delete(&veldsdk::proto::fs::FsDeleteRequest {
        path: origin_path(filename),
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
    }
    // Снимок перечитываем в любом случае: при ошибке — чтобы показать то, что
    // на диске на самом деле, при успехе — чтобы строка ушла.
    super::nav::request_list(state);
}
