//! Handlers для data-browser

pub mod search;
pub mod browse;
pub mod globe;
pub mod library;
pub mod listing;
pub mod nav;
pub mod outline;
pub mod persist;
pub mod overlay;
pub mod preview;
pub mod window;

use veldsdk::graphics::TextureFormat;

/// Открыть файл ресурсом тем, чей он: скачанный — библиотекой, потому что файл
/// под рукой, прочее — провайдером, по сети. `local` — имя записи библиотеки,
/// если файл лежит на диске целиком (`LibraryState::local_name`); им же
/// вкладка и сборка наложения помнят, чем открыт файл. Оба открывателя
/// отвечают одним `core.ResourceOpened` на эту корреляцию, и дальше разницы
/// нет. Учёт ожидания у каждого зовущего свой, поэтому корреляцию он заводит
/// сам.
pub fn open_resource(identifier: String, local: Option<String>, correlation: &str) {
    match local {
        Some(name) => crate::calls::data_library::on_open(
            &crate::proto::data_library::OpenRequest { name },
            correlation,
        ),
        None => crate::calls::data_provider::on_open(
            &crate::proto::data_provider::OpenRequest { identifier },
            correlation,
        ),
    }
}

/// Формат места под чужой рендер — и у канвы просмотра, и у шара. UNORM с
/// sRGB-числами: разметка сэмплит эту текстуру как обычную картинку и
/// линеаризует сама, а закодируй её GPU второй раз — вкладка приехала бы
/// темнее соседней. Один на оба места, потому что правило одно: разъехавшись,
/// они дали бы ровно эту разницу, и видна она была бы только их сравнением.
pub const SURFACE_FORMAT: TextureFormat = TextureFormat::TexRgba8Unorm;

#[derive(serde::Deserialize)]
pub struct Config {
    /// Вид в стартовой вкладке: "search" (умолчание), "browse", "downloaded".
    /// Превью здесь нет: оно открывается на конкретный файл, а его в конфиге
    /// не назовёшь.
    ///
    /// Спрашивается только на первом запуске: дальше окно открывается тем, как
    /// его сложили в прошлый раз (см. handlers::persist).
    pub initial_view: Option<String>,
}
