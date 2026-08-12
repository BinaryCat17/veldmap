//! Буфер, растущий под то, что в него пишут.
//!
//! Геометрия, собираемая каждый кадр, размера не имеет: у разметки он зависит
//! от того, сколько на экране строк и глифов, у контуров на глобусе — от того,
//! сколько нашлось снимков. Задать его числом нельзя не потому, что жалко
//! памяти, а потому, что упереться в заданное означало бы не «стало тесно», а
//! запись за край буфера — её ловит уже валидация wgpu, и ловит падением
//! процесса.

use crate::abi::{resource_alloc_buffer, resource_write};
use crate::proto::core::ResourceHandle;
use crate::OwnedResource;

use anyhow::anyhow;

pub struct GrowingBuffer {
    region: Option<OwnedResource>,
    usage: u32,
    what: &'static str,
    /// С какого размера начинать. Дальше буфер растёт сам, поэтому это не
    /// потолок, а «столько понадобится точно»: попав в него сразу, тот, кто
    /// пишет каждый кадр, не перевыделяет буфер ни разу.
    initial: u64,
    /// Сколько байт легло в буфер последней записью. Не то же, что его размер:
    /// рисовать нужно ровно записанное, а буфер обычно просторнее.
    filled: u64,
}

impl GrowingBuffer {
    pub const fn new(usage: u32, what: &'static str, initial: u64) -> Self {
        Self { region: None, usage, what, initial, filled: 0 }
    }

    /// Кладёт срез с начала буфера, перевыделив его, если не влезает.
    ///
    /// Пустой срез буфер не выделяет: рисовать по нему всё равно нечего, а
    /// первый непустой кадр выделит его сам.
    pub fn write<T: Copy>(&mut self, data: &[T]) -> anyhow::Result<()> {
        let bytes = super::bytes_of(data);
        self.filled = bytes.len() as u64;
        if bytes.is_empty() {
            return Ok(());
        }

        if self.region.as_ref().is_none_or(|region| region.size() < self.filled) {
            // Степенями двойки: иначе каждая добавленная строка перевыделяла бы
            // буфер, а перевыделение — это ещё и потерянный кадр отрисовки.
            let size = self.filled.next_power_of_two().max(self.initial);
            let id = resource_alloc_buffer(size, self.usage, false)
                .ok_or_else(|| anyhow!("не выделился буфер ({}, {} байт)", self.what, size))?;
            // Прежний освобождается своим Drop — присваиванием, а не отдельным
            // шагом.
            self.region = Some(OwnedResource::new(ResourceHandle { id, size, ..Default::default() }));
            crate::log::debug!(target: "graphics", "буфер {}: {} КиБ", self.what, size / 1024);
        }

        let region = self.region.as_ref().expect("буфер выделен выше");
        resource_write(region.id(), 0, bytes)
    }

    /// Id буфера; `None` — в него ещё ничего не писали.
    pub fn id(&self) -> Option<u64> {
        self.region.as_ref().map(|region| region.id())
    }

    /// Сколько байт записано последним `write`.
    pub fn filled(&self) -> u64 {
        self.filled
    }
}
