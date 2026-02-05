use veld_ui::proto::*;
use crate::handlers::*;
use veldsdk::prost::Message;
use veldsdk::rpc::core::{RpcRequest, RpcResponse};

mod state;
mod handlers;
mod renderer;
mod converter;

#[derive(serde::Deserialize, Clone)]
pub struct LocalConfig {}

#[no_mangle]
pub extern "C" fn init() -> i32 {
    let _ = veldsdk::core::init();
    0
}

#[no_mangle]
pub extern "C" fn handle_rpc() -> i32 {
    let input = veldsdk::rpc::host::load_input();
    let request = match RpcRequest::decode(&input[..]) {
        Ok(r) => r,
        Err(_) => return 1,
    };

    let result = match request.method.as_str() {
        "set_view" => {
            let req = SetViewRequest::decode(&request.payload[..]).unwrap();
            handle_set_view(req).map(|r| r.encode_to_vec())
        }
        "handle_ui_event" => {
            let req = HandleUiEventRequest::decode(&request.payload[..]).unwrap();
            handle_ui_event(req).map(|r| r.encode_to_vec())
        }
        "render" => {
            let req = RenderRequest::decode(&request.payload[..]).unwrap();
            handle_render(req).map(|r| r.encode_to_vec())
        }
        _ => Err(anyhow::anyhow!("Method not found")),
    };

    match result {
        Ok(payload) => {
            let res = RpcResponse { payload, error: String::new(), sync: None };
            veldsdk::rpc::host::store_output(res.encode_to_vec());
            0
        }
        Err(e) => {
            let res = RpcResponse { payload: Vec::new(), error: e.to_string(), sync: None };
            veldsdk::rpc::host::store_output(res.encode_to_vec());
            0
        }
    }
}
