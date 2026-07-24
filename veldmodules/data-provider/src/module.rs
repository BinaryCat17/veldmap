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
    /// path, ожидающий S3 list-ответа: id генерируется в on_list_path,
    /// снимается в on_http_result по correlation_id из ответа.
    pub pending_http: veldsdk::Correlator<String>,
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
    on_task_finished,
};
