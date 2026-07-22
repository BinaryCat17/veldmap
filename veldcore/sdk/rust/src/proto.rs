//! Сгенерированные prost-типы протокола платформы (veldcore/interface).
//! Словарь шины: конверты, разделяемые примитивы и контракты
//! платформенных сервисов. Контракт graphics — в graphics::proto.

pub mod core {
    include!(concat!(env!("OUT_DIR"), "/veldmap.core.rs"));
}

pub mod app {
    include!(concat!(env!("OUT_DIR"), "/veldmap.app.rs"));
}

pub mod fs {
    include!(concat!(env!("OUT_DIR"), "/veldmap.fs.rs"));
}

pub mod network {
    include!(concat!(env!("OUT_DIR"), "/veldmap.network.rs"));
}

pub mod tasks {
    include!(concat!(env!("OUT_DIR"), "/veldmap.tasks.rs"));
}
