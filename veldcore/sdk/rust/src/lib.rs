pub mod abi;
pub mod proto;
pub mod graphics;
pub mod correlator;
pub mod latest;
pub mod resource;
pub mod snapshot;
pub mod surface;
pub mod time;

/// Мост `log` → ABI хоста. Ставится сгенерированным клеем модуля; прикладной
/// код пишет обычными макросами крейта `log` (реэкспортирован ниже).
#[doc(hidden)]
pub mod logging;

/// Внутренности для сгенерированного клея модуля (buildgen, lib.rs.j2):
/// хранение состояния между вызовами хоста и диспетчеризация обработчиков.
/// Прикладной код сюда не обращается — модуль пишет только обработчики,
/// а вызывает их кодоген.
#[doc(hidden)]
pub mod runtime;

pub use serde_json;
pub use prost;
pub use anyhow;
pub use log;

pub use proto::core::ResourceHandle;
pub use abi::generate_id;
pub use abi::event_publisher;
pub use abi::answered_by_host;
pub use abi::correlation;
pub use correlator::Correlator;
pub use latest::{Latest, Reply};
pub use resource::{OwnedResource, ResourceReader};
