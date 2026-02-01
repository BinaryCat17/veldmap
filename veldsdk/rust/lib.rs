pub mod rpc;
pub mod core;

#[cfg(feature = "graphics")]
pub mod graphics;

#[cfg(feature = "iced")]
pub mod iced;

pub mod prelude {
    pub use crate::rpc::services::*;
    pub use crate::core::*;
    #[cfg(feature = "graphics")]
    pub use crate::graphics::*;
    #[cfg(feature = "iced")]
    pub use crate::iced::*;
}