pub mod cdse;
pub mod s3;

use aws_smithy_runtime_api::client::identity::Identity;

#[derive(serde::Deserialize, Clone)]
pub struct Config {
    pub access_key: String,
    pub secret_key: String,
}

#[derive(Clone)]
pub struct State {
    pub identity: Identity,
    /// Запрос листинга, ожидающий S3-ответа: id генерируется в on_list_path
    /// для внутреннего вызова network, снимается в on_http_result.
    pub pending_http: veldsdk::Correlator<PendingList>,
    /// Открываемые удалённые ресурсы: correlation_id → имя заказчика, которому
    /// уйдёт владение (см. cdse::on_open).
    pub opening: veldsdk::Correlator<String>,
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

    // inputs
    on_sign,
    on_list_path,
    on_search,
    on_open,

    // subs
    on_http_result,
    on_open_result,
};
