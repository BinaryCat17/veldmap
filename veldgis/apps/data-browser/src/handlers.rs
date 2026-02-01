use crate::{LocalConfig, LocalState};
use veldsdk::rpc::ui::UiEvent;
use veldsdk::rpc::services::RpcResponse;
use veldmap_gis_api::common::Empty;
use iced_core::{mouse, keyboard};
use crate::app::VeldMapToolsGui;
use crate::common;

pub(crate) fn module_init(_cfg: LocalConfig) -> anyhow::Result<LocalState> {
    let (gui, _task) = VeldMapToolsGui::new();
    let runtime = veldsdk::iced::create_runtime(
        gui, 
        iced_core::Font::with_name("VeldMap"),
        vec![
            ("DejaVuSans", common::DEJAVU_FONT_DATA),
            ("NotoColorEmoji", common::EMOJI_FONT_DATA),
        ]
    );
    
    Ok(LocalState(runtime))
}

pub(crate) fn handle_ui_event(state: &LocalState, event_proto: UiEvent) -> anyhow::Result<RpcResponse> {
    let runtime = &state.0;
    if let Some(ev) = event_proto.event {
        match ev {
            veldsdk::rpc::ui::ui_event::Event::Resize(r) => { 
                runtime.update_size(r.width, r.height, r.scale_factor);
            }
            veldsdk::rpc::ui::ui_event::Event::Click(c) => {
                runtime.update_cursor(c.x, c.y);
                let button = match c.button {
                    1 => mouse::Button::Left,
                    2 => mouse::Button::Right,
                    3 => mouse::Button::Middle,
                    _ => mouse::Button::Left,
                };
                let pos = runtime.cursor_position();
                runtime.push_event(iced_core::Event::Mouse(mouse::Event::CursorMoved { position: pos }));
                runtime.push_event(iced_core::Event::Mouse(mouse::Event::ButtonPressed(button)));
                runtime.push_event(iced_core::Event::Mouse(mouse::Event::ButtonReleased(button)));
            }
            veldsdk::rpc::ui::ui_event::Event::Key(k) => {
                let key = if k.key_code == 13 {
                    keyboard::Key::Named(keyboard::key::Named::Enter)
                } else if k.key_code == 8 {
                    keyboard::Key::Named(keyboard::key::Named::Backspace)
                } else {
                    keyboard::Key::Unidentified
                };

                if key != keyboard::Key::Unidentified {
                    let physical_key = keyboard::key::Physical::Unidentified(keyboard::key::NativeCode::Unidentified);
                    if k.pressed {
                        runtime.push_event(iced_core::Event::Keyboard(keyboard::Event::KeyPressed {
                            key: key.clone(),
                            modifiers: keyboard::Modifiers::default(),
                            location: keyboard::Location::Standard,
                            text: None,
                            modified_key: key,
                            physical_key,
                            repeat: false,
                        }));
                    } else {
                        runtime.push_event(iced_core::Event::Keyboard(keyboard::Event::KeyReleased {
                            key: key.clone(),
                            modifiers: keyboard::Modifiers::default(),
                            location: keyboard::Location::Standard,
                            modified_key: key,
                            physical_key,
                        }));
                    }
                }
            }
            _ => {}
        }
    }
    
    Ok(RpcResponse::default())
}

pub(crate) fn handle_render(state: &LocalState, _req: Empty) -> anyhow::Result<RpcResponse> {
    state.0.render()?;
    Ok(RpcResponse::default())
}
