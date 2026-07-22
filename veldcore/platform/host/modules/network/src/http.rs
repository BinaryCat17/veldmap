//! Разовый HTTP-запрос (топик network/http): ответ — событием http_result,
//! отмена — через реестр задач в State.

use super::State;
use veldmap_host_util::bindings::network as bus;
use veldmap_host_util::bindings::proto::network::{HttpTaskRequest, HttpTaskResponse};

pub fn on_input_http(state: &State, req: HttpTaskRequest, _requestor_id: u32) {
    let correlation_id = if req.correlation_id.is_empty() {
        uuid::Uuid::new_v4().to_string()
    } else {
        req.correlation_id.clone()
    };
    let ctx = state.ctx.clone();
    let cancel_key = correlation_id.clone();

    log::info!(target: "host", "Received HTTP request: {} {} (correlation_id: {})", req.method, req.url, correlation_id);

    let join_handle = tokio::spawn(async move {
        log::info!(target: "host", "Executing HTTP request {}...", correlation_id);
        let client = reqwest::Client::new();
        let method = match req.method.to_uppercase().as_str() {
            "POST" => reqwest::Method::POST,
            "PUT" => reqwest::Method::PUT,
            "DELETE" => reqwest::Method::DELETE,
            _ => reqwest::Method::GET,
        };

        let mut builder = client.request(method, &req.url);
        for (k, v) in req.headers { builder = builder.header(k, v); }
        if !req.body.is_empty() { builder = builder.body(req.body); }

        let result = match builder.send().await {
            Ok(res) => {
                let status = res.status().as_u16() as u32;
                let body = res.bytes().await.unwrap_or_default().to_vec();
                Ok((status, body))
            }
            Err(e) => Err(e.to_string()),
        };

        match result {
            Ok((status, body)) => {
                log::info!(target: "host", "HTTP request {} finished with status {}", correlation_id, status);
                bus::emit::http_result(&*ctx.dispatcher, &HttpTaskResponse { status, body, correlation_id: correlation_id.clone() });
            }
            Err(e) => {
                log::warn!(target: "host", "HTTP request {} failed: {}", correlation_id, e);
                bus::emit::http_result(&*ctx.dispatcher, &HttpTaskResponse { status: 0, body: Vec::new(), correlation_id: correlation_id.clone() });
            }
        }
    });

    state.local_tasks.lock().unwrap().insert(cancel_key, join_handle.abort_handle());
}
