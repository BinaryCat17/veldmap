//! Обработчики топиков data-provider: продукты CDSE как файлы и как ресурсы.
//!
//! Как устроен бакет — в s3.rs; здесь только жизненный цикл запросов и задач.

use crate::proto::data_provider::{
    DownloadRequest, DownloadStarted, DownloadProgress, Downloaded, CancelDownloadRequest,
    ListPathRequest, ListPathResponse
};
use aws_smithy_runtime_api::client::identity::Identity;
use super::{s3, Config, PendingList, StartingDownload, State};

pub fn module_init(config: Config) -> anyhow::Result<State> {
    let credentials = aws_credential_types::Credentials::new(
        config.access_key, 
        config.secret_key, 
        None, None, "veldmap"
    );
    let identity = Identity::new(credentials, None);
    
    Ok(State {
        identity,
        tasks: veldsdk::TaskTracker::new(),
        pending_http: veldsdk::Correlator::new(),
        starting: veldsdk::Correlator::new(),
        opening: veldsdk::Correlator::new(),
    })
}

/// Открыть продукт как ресурс, не скачивая его.
///
/// Подписать запрос к S3 может только этот модуль (ключи у него), поэтому
/// открывает он — а владение готовым ресурсом сразу передаёт заказчику:
/// дальше тот читает его как обычный файл и сам решает, когда закрыть.
pub fn on_open(state: &mut State, request: crate::proto::data_provider::OpenRequest) {
    let owner = veldsdk::abi::event_publisher();
    if owner.is_empty() {
        emit_open_error(request.correlation_id, "on_open пришёл от хоста: ресурс передать некому");
        return;
    }

    let object = s3::object(&state.identity, &request.identifier);
    state.opening.insert(request.correlation_id.clone(), owner);

    crate::calls::network::on_open(&veldsdk::proto::network::RemoteOpenRequest {
        url: object.url,
        headers: object.headers,
        correlation_id: request.correlation_id,
    });
}

/// network открыл удалённый ресурс — передаём владение заказчику.
pub fn on_open_result(state: &mut State, opened: veldsdk::proto::core::ResourceOpened) {
    let Some(owner) = state.opening.take(&opened.correlation_id) else { return };
    let correlation_id = opened.correlation_id;

    if !opened.error.is_empty() {
        emit_open_error(correlation_id, &opened.error);
        return;
    }
    let Some(handle) = opened.handle else {
        emit_open_error(correlation_id, "network вернул пустой handle");
        return;
    };
    if !veldsdk::abi::arena_transfer(handle.id, &owner) {
        veldsdk::abi::arena_free(handle.id);
        emit_open_error(correlation_id, &format!("не удалось передать ресурс сервису '{}'", owner));
        return;
    }

    crate::emit::on_open_result(&veldsdk::proto::core::ResourceOpened {
        handle: Some(handle),
        error: String::new(),
        correlation_id,
    });
}

fn emit_open_error(correlation_id: String, error: &str) {
    crate::emit::on_open_result(&veldsdk::proto::core::ResourceOpened {
        handle: None,
        error: error.to_string(),
        correlation_id,
    });
}

//  inputs ---------------------------------------------------------------------------------------------------------------------------

pub fn on_search(
    _state: &mut State, 
    _request: crate::proto::data_provider::SearchRequest
) {
    // TODO: Implement search via OData/OpenSearch
    log::info!(target: "handlers", "Search requested (not implemented)");
}

pub fn on_download(
    state: &mut State, 
    request: DownloadRequest
) {
    let task_id = veldsdk::generate_id();
    let destination = request.destination.clone();
    let identifier = request.identifier.clone();

    // Повторный запрос, пока предыдущий не зарегистрирован: у подписчика в
    // этом окне ещё нет task_id, то есть нет и способа увидеть, что закачка
    // уже идёт — отсечь дубль может только тот, кто её запустил.
    if state.starting.values().any(|s| s.identifier == identifier) {
        log::info!(target: "handlers", "Download of {} already starting, ignoring duplicate", identifier);
        return;
    }

    // DownloadStarted НЕ шлём здесь. Задача появится в реестре платформы
    // только когда network обработает on_fs_download — а до тех пор гарантия
    // "finished придёт для любой зарегистрированной задачи" (tasks.proto) на
    // этот task_id не распространяется: tasks/cancel вернул бы NotFound и
    // молча ничего не сделал, а подписчик, ждущий терминального события,
    // завис бы навсегда. Переставить emit ниже нельзя — шина асинхронная,
    // публикация после вызова тоже не значит "после регистрации". Ждём
    // on_task_started: платформа эмитит его ровно в момент регистрации.
    state.starting.insert(task_id.clone(), StartingDownload {
        identifier: identifier.clone(),
        destination: destination.clone(),
    });

    // Тот же адрес и та же подпись, что у открытия по сети: разойтись им
    // теперь негде, оба пути идут через s3::object.
    let object = s3::object(&state.identity, &identifier);
    state.tasks.track(task_id.clone());
    crate::calls::network::on_fs_download(&veldsdk::proto::network::FsDownloadRequest {
        url: object.url,
        path: destination,
        headers: object.headers,
        correlation_id: task_id,
    });
}

/// Задача зарегистрирована платформой — с этого момента она отменяема и на
/// неё распространяется гарантия терминального события. Только теперь
/// сообщаем подписчикам, что закачка началась.
pub fn on_task_started(
    state: &mut State,
    event: veldsdk::proto::tasks::TaskStarted
) {
    let Some(pending) = state.starting.take(&event.task_id) else { return; };
    crate::emit::on_download_started(&DownloadStarted {
        task_id: event.task_id,
        identifier: pending.identifier,
        destination: pending.destination,
    });
}

pub fn on_cancel_download(
    state: &mut State,
    request: CancelDownloadRequest
) {
    // Отменяем только задачи, которые этот модуль реально запустил.
    // Отмену выполнит платформа (топик tasks/cancel, проверка lease);
    // доменное уведомление — в on_task_finished, когда отмена случится.
    if !state.tasks.cancel(&request.task_id, crate::calls::tasks::on_cancel) {
        // Задача нам неизвестна: чужой task_id либо уже завершившаяся
        // закачка. Терминального события по ней не будет, поэтому молчать
        // нельзя — подписчик ждёт ответа на свою отмену.
        log::warn!(target: "handlers", "Cancel for unknown task {}", request.task_id);
        crate::emit::on_downloaded(&Downloaded {
            task_id: request.task_id,
            success: false,
            error: "Unknown task".to_string(),
            cancelled: true,
        });
    }
}

/// Терминальное событие платформы. Доменный результат (fs_download_result)
/// приходит первым и снимает задачу с учёта; сюда доходят только отмены —
/// при abort доменного результата не было, уведомляем подписчиков сами.
pub fn on_task_finished(
    state: &mut State,
    event: veldsdk::proto::tasks::TaskFinished
) {
    if !event.cancelled || !state.tasks.finish(&event.task_id) {
        return;
    }
    crate::emit::on_downloaded(&Downloaded {
        task_id: event.task_id,
        success: false,
        error: "Cancelled by user".to_string(),
        cancelled: true,
    });
}

pub fn on_list_path(
    state: &mut State, 
    request: ListPathRequest
) {
    let listing = s3::listing(&state.identity, &request.path, &request.token);
    let internal_id = state.pending_http.begin(PendingList {
        path: request.path,
        correlation_id: request.correlation_id,
    });

    log::info!(target: "handlers", "Requesting S3 list: {}", listing.url);

    crate::calls::network::on_http(&veldsdk::proto::network::HttpTaskRequest {
        url: listing.url,
        method: "GET".to_string(),
        headers: listing.headers,
        body: Vec::new(),
        correlation_id: internal_id,
    });
}

// subs ---------------------------------------------------------------------------------------------------------------------------

pub fn on_http_result(
    state: &mut State,
    response: veldsdk::proto::network::HttpTaskResponse,
) {
    let Some(pending) = state.pending_http.take(&response.correlation_id) else {
        return;
    };

    let listing = if response.status >= 200 && response.status < 300 {
        s3::parse_listing(&response.body, &pending.path).map_err(|e| e.to_string())
    } else {
        Err(format!("HTTP error: {}", response.status))
    };

    let (items, next_token, error) = match listing {
        Ok(listing) => (listing.items, listing.next_token, String::new()),
        Err(error) => {
            log::warn!(target: "handlers", "Листинг '{}' не удался: {}", pending.path, error);
            (Vec::new(), String::new(), error)
        }
    };

    crate::emit::on_list_path_result(&ListPathResponse {
        items,
        next_token,
        error,
        correlation_id: pending.correlation_id,
    });
}

pub fn on_fs_download_progress(
    state: &mut State,
    event: veldsdk::proto::network::FsDownloadProgress,
) {
    // Транслируем прогресс только своих активных загрузок.
    if !state.tasks.is_pending(&event.correlation_id) {
        log::info!(target: "handlers", "Dropping progress for unknown/finished task {}", event.correlation_id);
        return;
    }
    log::info!(target: "handlers", "Relaying progress task={} bytes={}/{}", event.correlation_id, event.downloaded_bytes, event.total_bytes);
    crate::emit::on_download_progress(&DownloadProgress {
        task_id: event.correlation_id,
        progress: event.progress,
        downloaded_bytes: event.downloaded_bytes,
        total_bytes: event.total_bytes,
    });
}

pub fn on_fs_download_result(
    state: &mut State,
    response: veldsdk::proto::network::FsDownloadResponse,
) {
    let correlation_id = response.correlation_id;
    if !state.tasks.finish(&correlation_id) {
        return;
    }

    let success = response.error.is_empty();
    crate::emit::on_downloaded(&Downloaded {
        task_id: correlation_id,
        success,
        error: response.error,
        // Доменный результат — закачка дошла до конца сама: отмена сюда не
        // попадает, она приходит только через on_task_finished (см. выше).
        cancelled: false,
    });
}
