//! Реестр ресурсов — единственный источник истины о существовании ресурса,
//! его носителе (payload) и правах доступа (lease).
//!
//! Запись одна на ресурс: lease и payload вместе, поэтому регистрация и
//! освобождение атомарны относительно карты. Разносить носители по
//! отдельным картам нельзя — согласовывать их пришлось бы вручную, аллокация
//! стала бы двухшаговой без отката, а освобождение — перебором карт.
//!
//! Акцессоры `payload`/`payload_mut` выполняют замыкание под guard'ом
//! DashMap. Отсюда два правила: нельзя звать их, уже держа guard той же
//! карты (дедлок шарда), и нельзя звать из замыкания обратно в реестр
//! (`register`/`unregister`/`payload*`) — guard берётся рекурсивно. Долгие
//! операции (блокирующее чтение носителя) выполняют, вынеся Arc из
//! замыкания и отпустив guard — так делает `MemoryManager::read`.

use dashmap::DashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::memory::DataBacking;

pub type ResourceId = u64;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Access {
    Read,
    Write,
}

pub struct Lease {
    pub owner_id: u32,
    pub readers: Vec<u32>,
    /// Кому владелец выдал право писать — так владелец окна назначает
    /// рендерера его целевой текстуры. Право писать включает и чтение.
    pub writers: Vec<u32>,
}

impl Lease {
    pub fn new(owner_id: u32) -> Self {
        Self { owner_id, readers: Vec::new(), writers: Vec::new() }
    }

    pub fn can_read(&self, module_id: u32) -> bool {
        self.owner_id == module_id
            || module_id == 0
            || self.readers.contains(&module_id)
            || self.writers.contains(&module_id)
    }

    /// Владелец и хост (id 0) пишут всегда — это и есть владение. Остальные —
    /// только по выданному гранту.
    ///
    /// Срока у права нет: аренды с TTL в платформе не существует. По этому же
    /// праву пускает `veld_resource_free` (см. abi.rs), поэтому любое условие,
    /// временно закрывающее запись, закрыло бы владельцу и освобождение.
    pub fn can_write(&self, module_id: u32) -> bool {
        self.owner_id == module_id || module_id == 0 || self.writers.contains(&module_id)
    }

    pub fn add_reader(&mut self, module_id: u32) {
        if !self.readers.contains(&module_id) && module_id != self.owner_id {
            self.readers.push(module_id);
        }
    }

    pub fn add_writer(&mut self, module_id: u32) {
        if !self.writers.contains(&module_id) && module_id != self.owner_id {
            self.writers.push(module_id);
        }
    }

}

/// Непрозрачный GPU-объект: адресуется по id, но байтового диапазона за ним
/// нет — читать со смещением нечего.
///
/// Текстура здесь, а не среди байтовых носителей, именно поэтому: прочитать
/// её по смещению нельзя (копия GPU→CPU остановила бы конвейер), а можно
/// только залить в неё изображение целиком. Байтовый ресурс — тот, у которого
/// работает `read(offset, size)`; по этой границе enum и поделён.
#[derive(Clone)]
pub enum GpuObject {
    Texture { texture: Arc<wgpu::Texture>, width: u32, height: u32, format: i32 },
    /// Размеры и формат — исходной текстуры: кадровый цикл клампит по размерам
    /// viewport и scissor (знать размеры окна для этого ему не нужно), а по
    /// формату отличает цветной аттачмент от буфера глубины.
    TextureView { view: Arc<wgpu::TextureView>, width: u32, height: u32, format: i32 },
    Sampler(Arc<wgpu::Sampler>),
    BindGroupLayout(Arc<wgpu::BindGroupLayout>),
    RenderPipeline(Arc<wgpu::RenderPipeline>),
    BindGroup(Arc<wgpu::BindGroup>),
    ShaderModule(Arc<wgpu::ShaderModule>),
}

/// Носитель ресурса: байты, адресуемые смещением, либо непрозрачный
/// GPU-объект. Ресурс один — id, владение (lease) и освобождение у обоих
/// вариантов общие.
pub enum ResourcePayload {
    Data(DataBacking),
    Gpu(GpuObject),
}

pub struct ResourceEntry {
    pub lease: Lease,
    pub payload: ResourcePayload,
}

pub struct ResourceRegistry {
    next_id: AtomicU64,
    entries: DashMap<ResourceId, ResourceEntry>,
}

impl ResourceRegistry {
    pub fn new() -> Self {
        Self {
            // Id 0 обозначает хост (суперпользователь в lease-проверках)
            next_id: AtomicU64::new(1),
            entries: DashMap::new(),
        }
    }

    pub fn register(&self, payload: ResourcePayload, owner_id: u32) -> ResourceId {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let entry = ResourceEntry {
            lease: Lease::new(owner_id),
            payload,
        };
        self.entries.insert(id, entry);
        id
    }

    pub fn unregister(&self, id: ResourceId) -> bool {
        self.entries.remove(&id).is_some()
    }

    /// Освобождает всё, чем владеет этот инстанс, и возвращает, сколько
    /// освободила.
    ///
    /// Это деструкторы убитого, исполненные хостом. Убийство происходит
    /// посреди любой работы и ничего не разматывает — модуль мог остаться
    /// владельцем наполовину собранной текстуры, и вернуть её может только
    /// тот, у кого лежит таблица владения. Ровно так же система забирает
    /// дескрипторы у процесса, которому выключили питание.
    ///
    /// Выданные этому инстансу чужие гранты не трогаем: они принадлежат не
    /// ему, а владельцам своих ресурсов, и те распорядятся ими сами.
    pub fn free_owned_by(&self, owner_id: u32) -> usize {
        let doomed: Vec<ResourceId> = self.entries.iter()
            .filter(|e| e.lease.owner_id == owner_id)
            .map(|e| *e.key())
            .collect();
        doomed.iter().filter(|id| self.unregister(**id)).count()
    }

    pub fn check_access(&self, id: ResourceId, requestor_id: u32, access: Access) -> bool {
        if let Some(entry) = self.entries.get(&id) {
            match access {
                Access::Read => entry.lease.can_read(requestor_id),
                Access::Write => entry.lease.can_write(requestor_id),
            }
        } else {
            false
        }
    }

    pub fn update_lease<F>(&self, id: ResourceId, f: F) -> bool
    where
        F: FnOnce(&mut Lease),
    {
        if let Some(mut entry) = self.entries.get_mut(&id) {
            f(&mut entry.lease);
            true
        } else {
            false
        }
    }

    /// Доступ к носителю под shared-guard'ом карты. Guard живёт до конца
    /// замыкания: возвращать из него можно только клоны/копии, а блокирующие
    /// операции выполнять уже после выхода (см. комментарий к модулю).
    pub fn payload<R>(&self, id: ResourceId, f: impl FnOnce(&ResourcePayload) -> R) -> Option<R> {
        self.entries.get(&id).map(|e| f(&e.payload))
    }

    /// То же по mutable-guard'у — для операций, пишущих в носитель на месте
    /// (CPU-буфер, mapped-диапазон, очередь wgpu).
    pub fn payload_mut<R>(&self, id: ResourceId, f: impl FnOnce(&mut ResourcePayload) -> R) -> Option<R> {
        self.entries.get_mut(&id).map(|mut e| f(&mut e.payload))
    }
}

impl Default for ResourceRegistry {
    fn default() -> Self {
        Self::new()
    }
}
