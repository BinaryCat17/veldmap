//! Удалённый файл как ресурс (топик network/open): читается Range-запросами,
//! целиком не скачивается.
//!
//! Для читателя такой ресурс неотличим от файла — тот же `arena_read(id,
//! offset, size)`. Поэтому декодер, умеющий работать окнами, снимает превью
//! со снимка на гигабайты, вытянув заголовок и несколько тайлов: остальное
//! по проводу не идёт. Условия — сервер отвечает на Range (иначе открытие
//! завершится ошибкой сразу, а не посреди чтения) и формат допускает
//! произвольный доступ.

use super::State;
use veldmap_host_util::bindings::network as bus;
use veldmap_host_util::bindings::proto::network::{RemoteOpenRequest, RemoteOpenResult};
use veldmap_host_util::core::ResourceHandle;
use veldmap_host_util::{blocking, RangeSource};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Размер блока кэша. Крупнее окна читателя: у HTTP заметная цена запроса,
/// и последовательный проход по файлу выгоднее делать реже и большими кусками.
const BLOCK: u64 = 4 * 1024 * 1024;

/// Потолок кэша на один ресурс. Проход по гигабайтному снимку не должен
/// превращаться в его копию в памяти — что не влезло, перечитается.
const CACHE_LIMIT: u64 = 64 * 1024 * 1024;

pub fn on_open(state: &State, req: RemoteOpenRequest, requestor_id: u32) {
    let correlation_id = req.correlation_id.clone();

    // Пробный запрос уходит в сеть, поэтому не в async-обработчике.
    blocking(&state.ctx, move |ctx| {
        let result = match HttpRange::open(&req.url, req.headers) {
            Ok(source) => {
                let len = source.len();
                let id = ctx.memory.alloc_remote(Arc::new(source), requestor_id);
                log::info!(target: "host", "Opened remote resource {} ({} bytes): {}", id, len, req.url);
                RemoteOpenResult {
                    handle: Some(ResourceHandle { id, size: len }),
                    error: String::new(),
                    correlation_id,
                }
            }
            Err(e) => RemoteOpenResult { handle: None, error: e.to_string(), correlation_id },
        };
        bus::emit::on_open_result(&*ctx.dispatcher, &result);
    });
}

/// Носитель поверх HTTP: блоки тянутся по требованию и кэшируются.
struct HttpRange {
    url: String,
    headers: HashMap<String, String>,
    len: u64,
    /// Хендл рантайма: read_at вызывается хостом с blocking-пула, где
    /// асинхронный запрос надо кому-то отдать.
    runtime: tokio::runtime::Handle,
    cache: Mutex<Cache>,
}

#[derive(Default)]
struct Cache {
    blocks: HashMap<u64, Vec<u8>>,
    /// Порядок появления — им же и вытесняем: у последовательного прохода
    /// (а это основной сценарий) самый старый блок и есть самый ненужный.
    order: std::collections::VecDeque<u64>,
    bytes: u64,
}

impl HttpRange {
    /// Пробный запрос первого байта: заодно проверяет, что сервер понимает
    /// Range, и узнаёт полный размер из Content-Range.
    fn open(url: &str, headers: HashMap<String, String>) -> anyhow::Result<Self> {
        let runtime = tokio::runtime::Handle::current();
        let response = runtime.block_on(super::http::get(url, &headers, Some((0, 1))).send())?;

        if response.status() != reqwest::StatusCode::PARTIAL_CONTENT {
            anyhow::bail!("сервер не поддерживает Range (HTTP {})", response.status());
        }
        let len = response
            .headers()
            .get(reqwest::header::CONTENT_RANGE)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.rsplit('/').next()?.parse::<u64>().ok())
            .ok_or_else(|| anyhow::anyhow!("сервер не сообщил размер файла (Content-Range)"))?;

        Ok(Self { url: url.to_string(), headers, len, runtime, cache: Mutex::new(Cache::default()) })
    }

    /// Блок из кэша или из сети.
    fn block(&self, index: u64) -> anyhow::Result<Vec<u8>> {
        if let Some(data) = self.cache.lock().unwrap().blocks.get(&index) {
            return Ok(data.clone());
        }

        let from = index * BLOCK;
        let to = (from + BLOCK).min(self.len);
        let response = self.runtime.block_on(
            super::http::get(&self.url, &self.headers, Some((from, to))).send(),
        )?;
        if !response.status().is_success() {
            anyhow::bail!("HTTP {}", response.status());
        }
        let data = self.runtime.block_on(response.bytes())?.to_vec();

        let mut cache = self.cache.lock().unwrap();
        while cache.bytes + data.len() as u64 > CACHE_LIMIT {
            let Some(oldest) = cache.order.pop_front() else { break };
            if let Some(dropped) = cache.blocks.remove(&oldest) {
                cache.bytes -= dropped.len() as u64;
            }
        }
        cache.bytes += data.len() as u64;
        cache.order.push_back(index);
        cache.blocks.insert(index, data.clone());
        Ok(data)
    }
}

impl RangeSource for HttpRange {
    fn len(&self) -> u64 {
        self.len
    }

    fn read_at(&self, offset: u64, size: u64) -> anyhow::Result<Vec<u8>> {
        if offset >= self.len { return Ok(Vec::new()); }
        let size = size.min(self.len - offset);
        let mut out = Vec::with_capacity(size as usize);

        let mut position = offset;
        while (out.len() as u64) < size {
            let block = self.block(position / BLOCK)?;
            let start = (position % BLOCK) as usize;
            if start >= block.len() { break; }
            let take = ((size - out.len() as u64) as usize).min(block.len() - start);
            out.extend_from_slice(&block[start..start + take]);
            position += take as u64;
        }
        Ok(out)
    }
}
