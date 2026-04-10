//! VeldMap Data Browser - GIS data discovery and download application

mod app;
mod screens;
pub mod task_manager;
pub mod widgets;

pub mod styles;
pub mod common;
pub mod service;
pub use task_manager::{TaskManager, TaskKind, TaskInfo};

pub use app::{AppState, AppMessage};
pub use common::{BrowserItem, LocalConfig};
pub use veldsdk::core::task::TaskStatus;
pub use veldmap_api::dataprovider::DataProduct;

use veld_ui::{VeldUiApp, UiRunner, Element};
use veldsdk::define_module;

pub struct DataBrowserApp {
    pub state: AppState,
}

impl VeldUiApp for DataBrowserApp {
    type Message = AppMessage;
    type Config = LocalConfig;

    fn init(config: Self::Config) -> anyhow::Result<(Self, veldsdk::core::Command<Self::Message>)> {
        let (state, cmd) = app::handlers::module_init(config)?;
        Ok((Self { state }, cmd))
    }

    fn update(&mut self, message: Self::Message) -> veldsdk::core::Command<Self::Message> {
        app::handlers::internal_update(&mut self.state, message)
    }

    fn view(&self) -> Element<Self::Message> {
        app::view::view(&self.state)
    }
}

define_module! {
    config: LocalConfig,
    state: UiRunner<DataBrowserApp>,
    init: UiRunner::<DataBrowserApp>::new,
    handlers: {
        "handle_ui_event" => veld_ui::handle_ui_event::<DataBrowserApp> : veldsdk::rpc::app::UiEvent => veld_ui::proto::HandleUiEventResponse,
    }
}
