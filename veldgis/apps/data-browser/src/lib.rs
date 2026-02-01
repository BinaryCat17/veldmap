mod app;
mod common;
mod utils;
mod search;
mod browse;
mod downloaded;
mod preview;

use veldsdk::define_iced_module;
use veldsdk::iced::{IcedModule, IcedSettings};
use crate::app::VeldMapToolsGui;

impl IcedModule for VeldMapToolsGui {
    type Message = crate::app::Message;
    type Config = crate::app::LocalConfig;

    fn init(config: Self::Config) -> anyhow::Result<(Self, IcedSettings)> {
        let (gui, _task) = VeldMapToolsGui::new(config);
        let settings = IcedSettings {
            default_font: iced_core::Font::with_name("VeldMap"),
            fonts: vec![
                ("DejaVuSans", common::DEJAVU_FONT_DATA),
                ("NotoColorEmoji", common::EMOJI_FONT_DATA),
            ],
        };
        Ok((gui, settings))
    }

    fn update(&mut self, message: Self::Message) {
        let _ = self.update(message);
    }

    fn view(&self) -> iced_core::Element<'_, Self::Message, iced_core::Theme, iced_tiny_skia::Renderer> {
        self.view()
    }
}

define_iced_module!(VeldMapToolsGui);
