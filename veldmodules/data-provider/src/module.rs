pub mod cdse;

use aws_smithy_runtime_api::client::identity::Identity;

#[derive(serde::Deserialize, Clone)]
pub struct Config {
    pub access_key: String,
    pub secret_key: String,
}

#[derive(Clone)]
pub struct State {
    pub identity: Identity,
    /// Учёт запущенных модулем задач: фильтрация broadcast-событий
    /// и отмена через платформенный протокол tasks/* (veldsdk).
    pub tasks: veldsdk::TaskTracker,
    /// Запрос листинга, ожидающий S3-ответа: id генерируется в on_list_path
    /// для внутреннего вызова network, снимается в on_http_result.
    pub pending_http: veldsdk::Correlator<PendingList>,
    /// Закачки, запрошенные у network, но ещё не зарегистрированные
    /// платформой. Пока задачи нет в реестре хоста, гарантия
    /// tasks/task_finished на неё не распространяется (см. tasks.proto), а
    /// значит и отменить её нельзя — tasks/cancel вернёт NotFound молча.
    /// Поэтому DownloadStarted наружу уходит не отсюда, а из on_task_started.
    pub starting: veldsdk::Correlator<StartingDownload>,
}

/// Закачка между «попросили network» и «платформа зарегистрировала задачу».
#[derive(Clone)]
pub struct StartingDownload {
    pub identifier: String,
    pub destination: String,
}

/// Контекст запроса на листинг: path — для дедупликации "самого себя" из
/// S3-листинга, correlation_id — внешний id вызывающего (data-browser),
/// эхом возвращается в ListPathResponse.
#[derive(Clone)]
pub struct PendingList {
    pub path: String,
    pub correlation_id: String,
}

// Re-export handlers to match the expected names in generated code
pub use cdse::{
    module_init as hook_init,

    // calls
    on_download,
    on_cancel_download,
    on_list_path,
    on_search,

    // subs
    on_fs_download_result,
    on_fs_download_progress,
    on_http_result,
    on_task_started,
    on_task_finished,
};
