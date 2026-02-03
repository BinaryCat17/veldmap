//! Iced integration for VeldMap WASM plugins.

use iced_core::Font;

pub mod runtime;

/// Settings for initializing the Iced runtime.
pub struct IcedSettings {
    pub default_font: Font,
    pub fonts: Vec<(&'static str, &'static [u8])>,
}

/// Internal trait used by the macro to drive the UI.
#[doc(hidden)]
pub trait RawIcedRuntime: Send + Sync {
    fn handle_event(&self, event: crate::rpc::ui::UiEvent) -> anyhow::Result<()>;
    fn render(&self) -> anyhow::Result<()>;
    fn tick(&self) -> anyhow::Result<()>;
}

#[doc(hidden)]
pub struct SendPtr<T>(pub *mut T);
unsafe impl<T> Send for SendPtr<T> {}

impl<T> SendPtr<T> {
    pub unsafe fn as_mut(&self) -> &mut T {
        &mut *self.0
    }
}

#[cfg(all(feature = "pdk", feature = "iced"))]
#[macro_export]
macro_rules! define_iced_module {
    (
        config: $config_type:ty,
        state: $state_type:ty,
        message: $message_type:ty,
        init: $init_func:path,
        view: $view_func:path,
        handlers: {
            $($msg_variant:ident $( ( $($arg:ident),* ) )? => $handler_func:path);* $(;)?
        }
    ) => {
        #[no_mangle]
        pub extern "C" fn init() -> i32 {
            let _ = $crate::core::init();
            let config_json = match $crate::rpc::host::get_config("config") {
                Some(c) => c,
                None => return 1,
            };
            
            let config: $config_type = match $crate::serde_json::from_str(&config_json) {
                Ok(c) => c,
                Err(_) => return 2,
            };

            let result = $init_func(config);
            let boxed_state: $crate::anyhow::Result<Box<dyn std::any::Any + Send + Sync>> = match result {
                Ok((state, settings)) => {
                    fn internal_update(state: &mut $state_type, message: $message_type) -> $crate::core::Command<$message_type> {
                        #[allow(unused_imports)]
                        use $message_type::*;
                        match message {
                            $(
                                $msg_variant $( ( $($arg),* ) )? => {
                                    $handler_func(state, $($($arg),*)?)
                                }
                            )*
                        }
                    }

                    let runtime = $crate::iced::runtime::IcedRuntime::new(
                        state,
                        internal_update,
                        $view_func,
                        settings.default_font, 
                        settings.fonts
                    );
                    let boxed_runtime: Box<dyn $crate::iced::RawIcedRuntime> = Box::new(runtime);
                    Ok(Box::new(boxed_runtime))
                },
                Err(e) => Err(e),
            };

            if $crate::rpc::MODULE_STATE.set(boxed_state).is_err() {
                 return 3;
            }
            0
        }

        #[no_mangle]
        pub extern "C" fn handle_rpc() -> i32 {
            use $crate::prost::Message;
            use $crate::rpc::services::{RpcRequest, RpcResponse};
            use $crate::iced::RawIcedRuntime;

            let input = $crate::rpc::host::load_input();
            let request = match RpcRequest::decode(&input[..]) {
                Ok(r) => r,
                Err(_) => return 1,
            };
            
            let state_any = match $crate::rpc::MODULE_STATE.get() {
                Some(Ok(s)) => s,
                Some(Err(e)) => return 2,
                None => return 3,
            };
            
            let runtime = state_any.downcast_ref::<Box<dyn RawIcedRuntime>>()
                .expect("Failed to downcast state to RawIcedRuntime");

            match request.method.as_str() {
                "handle_ui_event" => {
                    let event = match $crate::rpc::ui::UiEvent::decode(&request.payload[..]) {
                        Ok(ev) => ev,
                        Err(_) => return 4,
                    };
                    if let Err(_) = runtime.handle_event(event) { return 5; }
                    let response = RpcResponse { payload: Vec::new(), error: String::new(), sync: None };
                    $crate::rpc::host::store_output(response.encode_to_vec());
                    0
                }
                "render" => {
                    if let Err(_) = runtime.tick() { return 6; }
                    if let Err(_) = runtime.render() { return 7; }
                    let response = RpcResponse { payload: Vec::new(), error: String::new(), sync: None };
                    $crate::rpc::host::store_output(response.encode_to_vec());
                    0
                }
                _ => 8,
            }
        }
    };
}