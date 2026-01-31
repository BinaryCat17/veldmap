pub mod common {
    include!(concat!(env!("OUT_DIR"), "/veldmap.common.rs"));
}

pub mod dataprovider {
    include!(concat!(env!("OUT_DIR"), "/veldmap.dataprovider.rs"));
}

pub mod services {
    include!(concat!(env!("OUT_DIR"), "/veldmap.services.rs"));
}

pub mod geomath {
    include!(concat!(env!("OUT_DIR"), "/veldmap.geomath.rs"));
}

pub mod storage {
    include!(concat!(env!("OUT_DIR"), "/veldmap.storage.rs"));
}

pub mod render {
    include!(concat!(env!("OUT_DIR"), "/veldmap.render.rs"));
}

pub mod ui {
    include!(concat!(env!("OUT_DIR"), "/veldmap.ui.rs"));
}

pub mod host;

#[cfg(feature = "client")]
pub mod client;