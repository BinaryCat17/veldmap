//! Iced integration for VeldMap WASM plugins.

use iced_core::Font;
use std::pin::Pin;
use std::future::Future;

pub mod runtime;

/// Settings for initializing the Iced runtime.
pub struct IcedSettings {
    pub default_font: Font,
    pub fonts: Vec<(&'static str, &'static [u8])>,
}

/// A command that describes a side effect to be performed.
pub struct Command<M>(pub(crate) Vec<BoxedFuture<M>>);

impl<M> Command<M> {
    /// Creates an empty command.
    pub fn none() -> Self {
        Self(Vec::new())
    }

    /// Creates a command from a future that returns a result, wrapping it in a message.
    pub fn perform<F, T, G>(future: F, msg_wrap: G) -> Self 
    where 
        F: Future<Output = T> + Send + 'static,
        G: FnOnce(T) -> M + Send + 'static,
        T: 'static,
        M: 'static 
    {
        Self(vec![Box::pin(async move { Some(msg_wrap(future.await)) })])
    }

    /// Creates a command from a raw future that returns an option of message.
    pub fn perform_raw<F>(future: F) -> Self 
    where 
        F: Future<Output = Option<M>> + Send + 'static,
        M: 'static 
    {
        Self(vec![Box::pin(future)])
    }

    /// Creates a command from a future that doesn't return anything.
    pub fn perform_action<F>(future: F) -> Self 
    where 
        F: Future<Output = ()> + Send + 'static,
        M: 'static 
    {
        Self(vec![Box::pin(async move { future.await; None })])
    }

    pub fn batch(commands: impl IntoIterator<Item = Self>) -> Self {
        let mut futures = Vec::new();
        for cmd in commands {
            futures.extend(cmd.0);
        }
        Self(futures)
    }
}

/// Internal trait used by the macro to drive the UI.
#[doc(hidden)]
pub trait RawIcedRuntime: Send + Sync {
    fn handle_event(&self, event: crate::rpc::ui::UiEvent) -> anyhow::Result<()>;
    fn render(&self) -> anyhow::Result<()>;
    fn tick(&self) -> anyhow::Result<()>;
}

pub type BoxedFuture<M> = Pin<Box<dyn Future<Output = Option<M>> + Send + 'static>>;

#[macro_export]
macro_rules! rpc_call {
    ($service:expr, $method:expr, $payload:expr, $resp_type:ty) => {
        async move {
            $crate::core::yield_now().await;
            let res = $crate::rpc::host::call_service($service, $method, $payload);
            res.and_then(|bytes| {
                <$resp_type as ::prost::Message>::decode(&bytes[..])
                    .map_err(|e| ::anyhow::anyhow!("Decode error: {}", e))
            }).map_err(|e| e.to_string())
        }
    };
}

#[macro_export]
macro_rules! rpc_command {
    ($service:expr, $method:expr, $payload:expr, $resp_type:ty, $processor:expr) => {
        $crate::iced::Command::perform(
            $crate::rpc_call!($service, $method, $payload, $resp_type),
            $processor
        )
    };
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
        #[extism_pdk::plugin_fn]
        pub fn init() -> extism_pdk::FnResult<()> {
            let _ = $crate::core::init();
            let config_json = extism_pdk::config::get("config")
                .map_err(|e| extism_pdk::Error::msg(format!("Failed to get config: {}", e)))?
                .ok_or_else(|| extism_pdk::Error::msg("Config not found"))?;
            
            let config: $config_type = serde_json::from_str(&config_json)
                .map_err(|e| extism_pdk::Error::msg(format!("Failed to parse config: {}", e)))?;

            let result = $init_func(config);
            let boxed_state: anyhow::Result<Box<dyn std::any::Any + Send + Sync>> = match result {
                Ok((state, settings)) => {
                    fn internal_update(state: &mut $state_type, message: $message_type) -> $crate::iced::Command<$message_type> {
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
                 return Err(extism_pdk::Error::msg("Module state already initialized").into());
            }
            Ok(())
        }

        #[extism_pdk::plugin_fn]
        pub fn handle_rpc(input: Vec<u8>) -> extism_pdk::FnResult<Vec<u8>> {
            use prost::Message;
            use $crate::rpc::services::{RpcRequest, RpcResponse};
            use $crate::iced::RawIcedRuntime;

            let request = RpcRequest::decode(&input[..])
                .map_err(|e| anyhow::anyhow!("Failed to decode RpcRequest: {}", e))?;
            
            let state_any = match $crate::rpc::MODULE_STATE.get() {
                Some(Ok(s)) => s,
                Some(Err(e)) => return Err(anyhow::anyhow!("Module initialization failed: {}", e).into()),
                None => return Err(anyhow::anyhow!("Module not initialized").into()),
            };
            
            let runtime = state_any.downcast_ref::<Box<dyn RawIcedRuntime>>()
                .expect("Failed to downcast state to RawIcedRuntime");

            match request.method.as_str() {
                "handle_ui_event" => {
                    let event = $crate::rpc::ui::UiEvent::decode(&request.payload[..])
                        .map_err(|e| anyhow::anyhow!("Failed to decode UiEvent: {}", e))?;
                    runtime.handle_event(event)?;
                    runtime.tick()?;
                    let response = RpcResponse { payload: Vec::new(), error: String::new(), sync: None };
                    Ok(response.encode_to_vec())
                }
                "render" => {
                    runtime.tick()?; 
                    runtime.render()?;
                    let response = RpcResponse { payload: Vec::new(), error: String::new(), sync: None };
                    Ok(response.encode_to_vec())
                }
                _ => Err(anyhow::anyhow!("Method '{}' not found in Iced module", request.method).into()),
            }
        }
    };
}