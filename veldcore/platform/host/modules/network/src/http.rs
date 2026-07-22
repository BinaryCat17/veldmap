//! Разовый HTTP-запрос (топик network/http): ответ — событием http_result,
//! жизненный цикл и отмена — через фасад Tasks (см. module.rs).

use super::State;
use veldmap_host_util::bindings::network as bus;
use veldmap_host_util::bindings::proto::network::{HttpTaskRequest, HttpTaskResponse};

pub fn on_input_http(state: &State, req: HttpTaskRequest, requestor_id: u32) {
    let ctx = state.ctx.clone();
    let label = format!("{} {}", req.method, req.url);

    log::info!(target: "host", "Received HTTP request: {}", label);

    let spawned = state.tasks.spawn(&req.correlation_id, requestor_id, "http", &label, |correlation_id| async move {
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

        match builder.send().await {
            Ok(res) => {
                let status = res.status().as_u16() as u32;
                let body = res.bytes().await.unwrap_or_default().to_vec();
                log::info!(target: "host", "HTTP request {} finished with status {}", correlation_id, status);
                bus::emit::http_result(&*ctx.dispatcher, &HttpTaskResponse { status, body, correlation_id });
                Ok(())
            }
            Err(e) => {
                log::warn!(target: "host", "HTTP request {} failed: {}", correlation_id, e);
                let error = e.to_string();
                bus::emit::http_result(&*ctx.dispatcher, &HttpTaskResponse { status: 0, body: Vec::new(), correlation_id });
                Err(error)
            }
        }
    });

    if let Err(dup) = spawned {
        bus::emit::http_result(&*state.ctx.dispatcher, &HttpTaskResponse {
            status: 0,
            body: Vec::new(),
            correlation_id: dup.0,
        });
    }
}
