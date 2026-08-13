//! Текстуры тайлов наложений в видеопамяти, с вытеснением по бюджету.
//!
//! Тот же уговор, что у канвы превью (image-view/tiles.rs): хранилище одно на
//! модуль, ключ — отпечаток источника и адрес в пирамиде, вытеснение — давно
//! не рисовавшиеся, и вытеснение — это забывание: тайл остаётся на диске у
//! tile-cache и вернётся оттуда за миллисекунды.

use std::collections::HashMap;

use veldsdk::graphics::{self as gfx, BindGroupId, TextureViewId};
use veldsdk::proto::core::ResourceHandle;

use super::gpu::Device;

/// Адрес тайла в пирамиде: уровень, колонка, ряд.
pub type Addr = (u32, u32, u32);

pub struct Stored {
    /// Держит текстуру живой; bind group и view — производные от неё.
    _texture: veldsdk::OwnedResource,
    _view: TextureViewId,
    pub bind: BindGroupId,
    pub width: u32,
    pub height: u32,
    bytes: u64,
    touched: u64,
}

pub struct TileStore {
    /// Отпечаток источника → тайлы. Двухэтажная карта, чтобы не клонировать
    /// отпечаток в ключ каждого тайла.
    sources: HashMap<String, HashMap<Addr, Stored>>,
    budget: u64,
    bytes: u64,
    /// Логические часы обращений — по ним выбирается жертва.
    tick: u64,
    /// Растёт на каждом добавлении и вытеснении: кадр, собранный при другом
    /// поколении, устарел.
    pub generation: u64,
}

impl TileStore {
    pub fn new(budget: u64) -> Self {
        Self { sources: HashMap::new(), budget, bytes: 0, tick: 0, generation: 0 }
    }

    /// Пустое хранилище с тем же бюджетом — на пересборку устройства: bind
    /// group'ы собраны под layout прежнего и с новым несовместимы.
    pub fn new_like(other: &TileStore) -> Self {
        Self::new(other.budget)
    }

    /// Сколько тайлов помещается в бюджет — потолок аппетита одного уровня
    /// (см. overlay::desired_level).
    pub fn capacity_tiles(&self, tile_bytes: u64) -> u64 {
        self.budget / tile_bytes.max(1)
    }

    /// Принимает тайл во владение: view и bind group создаются здесь же, и
    /// с ними тайл готов к рисованию без единого вызова в кадре. Отказ
    /// графики — тайл выброшен (Drop), промах останется промахом.
    pub fn insert(
        &mut self,
        device: &Device,
        fingerprint: &str,
        addr: Addr,
        texture: ResourceHandle,
        width: u32,
        height: u32,
    ) -> Result<(), String> {
        let texture = veldsdk::OwnedResource::new(texture);
        let view = gfx::create_texture_view(texture.id()).map_err(|e| e.to_string())?;
        let bind = device.overlay_bind_group(&view).map_err(|e| e.to_string())?;

        let bytes = u64::from(width) * u64::from(height) * 4;
        self.tick += 1;
        let stored = Stored {
            _texture: texture,
            _view: view,
            bind,
            width,
            height,
            bytes,
            touched: self.tick,
        };

        let tiles = self.sources.entry(fingerprint.to_string()).or_default();
        // Повтор (кэш и производитель могли прислать один тайл наперегонки)
        // замещается: старая текстура освобождается своим Drop.
        if let Some(old) = tiles.insert(addr, stored) {
            self.bytes -= old.bytes;
        }
        self.bytes += bytes;
        self.generation += 1;
        self.evict_to_budget();
        Ok(())
    }

    /// Тайл к рисованию: обращение продлевает ему жизнь.
    pub fn touch(&mut self, fingerprint: &str, addr: Addr) -> Option<&Stored> {
        self.tick += 1;
        let stored = self.sources.get_mut(fingerprint)?.get_mut(&addr)?;
        stored.touched = self.tick;
        Some(stored)
    }

    pub fn contains(&self, fingerprint: &str, addr: Addr) -> bool {
        self.sources.get(fingerprint).is_some_and(|tiles| tiles.contains_key(&addr))
    }

    /// Забыть источник целиком — наложение сняли, его тайлам больше не с чего
    /// рисоваться.
    pub fn forget(&mut self, fingerprint: &str) {
        if let Some(tiles) = self.sources.remove(fingerprint) {
            for stored in tiles.values() {
                self.bytes -= stored.bytes;
            }
            self.generation += 1;
        }
    }

    /// Старейшие — вон, пока не влезем. Линейный поиск жертвы честен: бюджет
    /// в четверть гигабайта — это порядка двух сотен тайлов.
    fn evict_to_budget(&mut self) {
        while self.bytes > self.budget {
            let victim = self
                .sources
                .iter()
                .flat_map(|(key, tiles)| {
                    tiles.iter().map(move |(addr, stored)| (stored.touched, key, addr))
                })
                .min_by_key(|(touched, ..)| *touched)
                .map(|(_, key, addr)| (key.clone(), *addr));
            let Some((key, addr)) = victim else { return };

            if let Some(tiles) = self.sources.get_mut(&key) {
                if let Some(old) = tiles.remove(&addr) {
                    self.bytes -= old.bytes;
                }
                if tiles.is_empty() {
                    self.sources.remove(&key);
                }
            }
            self.generation += 1;
        }
    }
}
