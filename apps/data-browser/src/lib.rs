use extism_pdk::*;
use veldmap_rust_rpc::services::{RpcRequest, RpcResponse};
use veldmap_rust_rpc::ui::{DrawFrame, UiDisplayCommand, ui_display_command};
use veldmap_rust_rpc::host::call_service;
use prost::Message;

#[plugin_fn]
pub fn handle_rpc(input: Vec<u8>) -> FnResult<Vec<u8>> {
    let request = RpcRequest::decode(&input[..])
        .map_err(|e| anyhow::anyhow!("Failed to decode RpcRequest: {}", e))?;
    
    let (payload, error) = match request.method.as_str() {
        "init" => {
            // Логируем через системный сервис
            let _ = call_service("system", "log", "Data Browser initialized!".as_bytes().to_vec());
            
            // Запрашиваем отрисовку первого кадра
            let mut rgba_data = vec![0u8; 100 * 100 * 4];
            for chunk in rgba_data.chunks_exact_mut(4) {
                chunk[0] = 255; // Red
                chunk[3] = 255; // Alpha
            }
            let frame = DrawFrame {
                rgba_data,
                width: 100,
                height: 100,
            };
            let cmd = UiDisplayCommand {
                command: Some(ui_display_command::Command::DrawFrame(frame)),
            };
            let _ = call_service("app", "display", cmd.encode_to_vec());
            
            (Vec::new(), String::new())
        }
        "render" => {
            // Здесь логика обновления кадра
            (Vec::new(), String::new())
        }
        _ => (Vec::new(), format!("Method {} not found in data-browser", request.method)),
    };

    let response = RpcResponse { payload, error, sync: None };
    Ok(response.encode_to_vec())
}