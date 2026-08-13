pub mod catalogue;
pub mod cdse;
pub mod imagery;
pub mod mgrs;
pub mod s3;
pub mod time;

use aws_smithy_runtime_api::client::identity::Identity;

#[derive(serde::Deserialize, Clone)]
pub struct Config {
    pub access_key: String,
    pub secret_key: String,
}

pub struct State {
    pub identity: Identity,
    /// Запросы, ожидающие HTTP-ответа: id генерируется, когда мы зовём network,
    /// и снимается в on_http_result.
    pub pending_http: veldsdk::Correlator<Pending>,
    /// Открываемые удалённые ресурсы: correlation_id → имя заказчика, которому
    /// уйдёт владение (см. cdse::on_open).
    pub opening: veldsdk::Correlator<String>,
}

/// Что мы спросили у сети и кому отвечать.
///
/// Хранилище и каталог отвечают в один и тот же топик, поэтому вид запроса
/// записан здесь, а не выводится из ответа: догадываться по содержимому значит
/// однажды разобрать листинг как поиск.
pub struct Pending {
    /// Внешний id заказчика (data-browser); эхом возвращается в его ответ.
    pub correlation_id: String,
    pub what: Asked,
}

pub enum Asked {
    /// Листинг пути. Сам путь нужен, чтобы отсеять из ответа S3 запрошенный
    /// префикс — в списке содержимого папке незачем быть собой.
    List(String),
    Search,
    /// Растры продукта: поддерево листается страницами, найденные ключи
    /// копятся здесь, пока хранилище не отдаст последнюю.
    Imagery { identifier: String, keys: Vec<String> },
    /// Продукт по точному имени — обратный ход поиска. Имя хранится для
    /// объяснения пустого ответа: каталог на «не нашлось» отвечает молча.
    Locate { name: String },
}

// Реэкспорт обработчиков: имена совпадают с ключами топиков в schema.yaml,
// по ним их зовёт сгенерированный клей.
pub use cdse::{
    module_init as hook_init,

    // inputs
    on_sign,
    on_list_path,
    on_search,
    on_open,
    on_imagery,
    on_locate,

    // subs
    on_http_result,
    on_open_result,
};
