pub mod common {
    include!(concat!(env!("OUT_DIR"), "/veldmap.common.rs"));
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



pub mod host;
