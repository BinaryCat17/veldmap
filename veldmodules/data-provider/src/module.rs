pub mod catalogue;
pub mod cdse;
pub mod imagery;
pub mod manifest;
pub mod mgrs;
pub mod s3;
pub mod scene;
pub mod time;

use aws_smithy_runtime_api::client::identity::Identity;
use crate::proto::data_provider::{DataProduct, SearchRequest};

#[derive(serde::Deserialize, Clone)]
pub struct Config {
    pub access_key: String,
    pub secret_key: String,
}

pub struct State {
    /// Чем подписываются запросы к хранилищу. `None` — ключей нет: конфиг
    /// оставил их пустыми, потому что `.env` не завели (см. `cdse::NO_KEYS`).
    /// Именно `Option`, а не пустые ключи внутри подписи: подписать ими можно,
    /// и разница видна только на той стороне, кодом 403.
    pub identity: Option<Identity>,
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
    /// Поиск. Запрос хранится, потому что ответ может его не закрыть: своё окно
    /// свежести мы вправе снять и спросить заново (см. `catalogue::FRESH_DAYS`),
    /// а `widened` говорит, что это уже второй заход и третьего не будет.
    Search { request: SearchRequest, widened: bool },
    /// Растры продукта: поддерево листается страницами, найденные ключи
    /// копятся здесь, пока хранилище не отдаст последнюю.
    /// Обход поддерева продукта под растры. `downloaded` едет с самого
    /// запроса и до самого выбора: от него зависит, предлагать ли подробный
    /// растр, который читается только целым проходом по файлу
    /// (см. `ImageryRequest.downloaded`).
    Imagery { identifier: String, keys: Vec<String>, downloaded: bool },
    /// Манифест продукта — второй ход после обхода: ключи уже собраны, а какой
    /// из них измерение, называет он (см. `manifest`). Ключи едут с собой,
    /// потому что ответ на `on_imagery` собирается из них обоих.
    /// Манифест продукта — за ним идут, когда выбор растра иначе был бы
    /// догадкой. `downloaded` едет и здесь: он про то же самое, что и в
    /// [`Asked::Imagery`], и до выбора добирается только через это ожидание.
    Manifest { identifier: String, keys: Vec<String>, downloaded: bool },
    /// Продукт по точному имени — обратный ход поиска. Имя хранится для
    /// объяснения пустого ответа: каталог на «не нашлось» отвечает молча.
    Locate { name: String },
    /// Упаковки той же съёмки — второй ход после `Locate`: спросили об одной
    /// упаковке, а показывать надо снимок. `facts` — факты найденного, по
    /// которым из ответа отбираются свои (`scene::same_scene`); `found` — то,
    /// что нашлось по имени, и ответ, если упаковок не подъехало.
    Siblings { facts: scene::Facts, found: DataProduct },
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
    on_product_roots,

    // subs
    on_http_result,
    on_open_result,
};
