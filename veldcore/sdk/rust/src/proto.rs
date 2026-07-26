//! Сгенерированные prost-типы протокола платформы (veldcore/interface):
//! словарь шины, транспорт, контракты платформенных сервисов и ABI-графики.
//!
//! Дерево модулей собирает prost по пакетам .proto (`veldmap.<name>` →
//! `proto::<name>`), поэтому список сервисов здесь не дублируется: что
//! просканировал build.rs, то и появилось.

mod generated {
    include!(concat!(env!("OUT_DIR"), "/_protos.rs"));
}

pub use generated::veldmap::*;
