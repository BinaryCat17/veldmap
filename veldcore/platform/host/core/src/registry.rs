use dashmap::DashMap;
use std::sync::atomic::{AtomicU64, Ordering};

pub type ResourceId = u64;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Access {
    Read,
    Write,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ResourceBackend {
    Memory,
    GpuState,
    // Network, // For future use
}

pub struct Lease {
    pub owner_id: u32,
    pub readers: Vec<u32>,
    /// Modules granted write access by the owner (e.g. a renderer writing into
    /// a window target texture owned by the window's module). Writers can read.
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
    /// Срока у права нет намеренно: аренды с TTL в платформе не существует, а
    /// поле `expires_at` тут было единственным её следом. Выставлял его только
    /// `revoke_all`, причём моментом «сейчас», и проверка стояла ПОСЛЕ проверки
    /// владельца — то есть отзыв чужих грантов запрещал запись и самому
    /// владельцу, и хосту. А так как `veld_memory_free` пускает по этому же
    /// праву (см. abi.rs), владелец после отзыва не мог и освободить свой
    /// регион: тот утекал до конца процесса, молча.
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

    pub fn remove_reader(&mut self, module_id: u32) {
        self.readers.retain(|&r| r != module_id);
    }

    /// Снимает все выданные гранты. Владения не касается: владелец продолжает
    /// читать, писать и освобождать свой регион — отозвать у себя собственный
    /// ресурс нельзя, для этого есть `free`.
    pub fn revoke_all(&mut self) {
        self.readers.clear();
        self.writers.clear();
    }
}

pub struct ResourceEntry {
    pub backend: ResourceBackend,
    pub lease: Lease,
    pub name: Option<String>,
}

pub struct ResourceRegistry {
    next_id: AtomicU64,
    entries: DashMap<ResourceId, ResourceEntry>,
    named_resources: DashMap<String, ResourceId>,
}

impl ResourceRegistry {
    pub fn new() -> Self {
        Self {
            // Id 0 обозначает хост (суперпользователь в lease-проверках)
            next_id: AtomicU64::new(1),
            entries: DashMap::new(),
            named_resources: DashMap::new(),
        }
    }

    pub fn register(&self, backend: ResourceBackend, owner_id: u32, name: Option<String>) -> ResourceId {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let entry = ResourceEntry {
            backend,
            lease: Lease::new(owner_id),
            name: name.clone(),
        };
        self.entries.insert(id, entry);
        if let Some(n) = name {
            self.named_resources.insert(n, id);
        }
        id
    }

    pub fn register_with_id(&self, id: ResourceId, backend: ResourceBackend, owner_id: u32, name: Option<String>) {
        let entry = ResourceEntry {
            backend,
            lease: Lease::new(owner_id),
            name: name.clone(),
        };
        self.entries.insert(id, entry);
        if let Some(n) = name {
            self.named_resources.insert(n, id);
        }
    }

    pub fn unregister(&self, id: ResourceId) -> bool {
        if let Some((_, entry)) = self.entries.remove(&id) {
            if let Some(name) = entry.name {
                self.named_resources.remove(&name);
            }
            true
        } else {
            false
        }
    }

    pub fn get_named_id(&self, name: &str) -> Option<ResourceId> {
        self.named_resources.get(name).map(|v| *v)
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

    pub fn get_backend(&self, id: ResourceId) -> Option<ResourceBackend> {
        self.entries.get(&id).map(|e| e.backend)
    }

    pub fn get_owner(&self, id: ResourceId) -> Option<u32> {
        self.entries.get(&id).map(|e| e.lease.owner_id)
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
}

impl Default for ResourceRegistry {
    fn default() -> Self {
        Self::new()
    }
}
