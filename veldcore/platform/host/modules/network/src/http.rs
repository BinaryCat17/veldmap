//! Разовый HTTP-запрос (топик network/http): ответ — событием http_result,
//! жизненный цикл и отмена — через фасад Tasks (см. module.rs).
//!
//! Здесь же общий транспорт: клиент, заголовки и Range-запрос — одна
//! реализация на всех потребителей (разовый запрос, потоковая докачка
//! в download.rs, оконное чтение удалённого ресурса в range.rs).

use super::State;
use veldmap_host_util::Caller;
use veldmap_host_util::bindings::network as bus;
use veldmap_host_util::bindings::proto::network::{HttpTaskRequest, HttpTaskResponse};
use std::collections::HashMap;

/// Сколько ждать установления соединения и следующей порции байт.
///
/// Таймауты обязательны, а не «на всякий случай»: оконное чтение удалённого
/// ресурса — синхронный для гостя ABI-вызов, поэтому зависшее соединение
/// останавливает не запрос, а весь модуль-читатель (его актор ждёт возврата
/// из handle_event). Отмена задачи такому чтению не поможет — оно опрашивает
/// её только между порциями. Ограничение по времени — единственное, что
/// делает зависание конечным. Пишется на весь ответ намеренно не берётся:
/// скачивание снимка легально идёт долго, а вот молчание сервера — нет.
const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);
const READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// Общий клиент процесса: держит пул соединений, поэтому оконное чтение не
/// переустанавливает TLS-сессию на каждый Range-запрос.
pub fn client() -> reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| {
        // Первым клиента может запросить и оконное чтение — а оно идёт с
        // blocking-пула, где контекст рантайма надо назвать явно, иначе
        // клиенту не к чему привязать таймеры и резолвер.
        let _guard = tokio::runtime::Handle::current().enter();
        reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .read_timeout(READ_TIMEOUT)
            .build()
            .unwrap_or_default()
    }).clone()
}

/// GET с заголовками вызывающего и (необязательно) диапазоном байт.
/// `range` — полуинтервал [from, to); to == 0 означает «до конца файла»,
/// то есть ровно то, что нужно докачке хвоста.
pub fn get(url: &str, headers: &HashMap<String, String>, range: Option<(u64, u64)>) -> reqwest::RequestBuilder {
    let mut builder = client().get(url);
    for (key, value) in headers {
        builder = builder.header(key, value);
    }
    if let Some((from, to)) = range {
        builder = builder.header("Range", match to {
            0 => format!("bytes={}-", from),
            to => format!("bytes={}-{}", from, to.saturating_sub(1)),
        });
    }
    builder
}

pub fn on_http(state: &State, req: HttpTaskRequest, caller: Caller) {
    let ctx = state.ctx.clone();
    let label = format!("{} {}", req.method, req.url);

    log::info!(target: "network", "Received HTTP request: {}", label);

    // Задача именуется корреляцией запроса: заказчик отменяет её тем же id,
    // которым опознаёт ответ.
    let spawned = state.tasks.spawn(&caller.correlation, caller.instance, "http", &label, |correlation_id| async move {
        log::info!(target: "network", "Executing HTTP request {}...", correlation_id);
        let method = match req.method.to_uppercase().as_str() {
            "POST" => reqwest::Method::POST,
            "PUT" => reqwest::Method::PUT,
            "DELETE" => reqwest::Method::DELETE,
            _ => reqwest::Method::GET,
        };

        let mut builder = client().request(method, &req.url);
        for (k, v) in req.headers { builder = builder.header(k, v); }
        if !req.body.is_empty() { builder = builder.body(req.body); }

        match builder.send().await {
            Ok(res) => {
                let status = res.status().as_u16() as u32;
                let body = res.bytes().await.unwrap_or_default().to_vec();
                log::info!(target: "network", "HTTP request {} finished with status {}", correlation_id, status);
                bus::emit::on_http_result(&*ctx.dispatcher, &HttpTaskResponse { status, body }, &correlation_id);
                Ok(())
            }
            Err(e) => {
                log::warn!(target: "network", "HTTP request {} failed: {}", correlation_id, e);
                let error = e.to_string();
                bus::emit::on_http_result(&*ctx.dispatcher, &HttpTaskResponse { status: 0, body: Vec::new() }, &correlation_id);
                Err(error)
            }
        }
    });

    if let Err(dup) = spawned {
        bus::emit::on_http_result(&*state.ctx.dispatcher, &HttpTaskResponse {
            status: 0,
            body: Vec::new(),
        }, &dup.0);
    }
}
