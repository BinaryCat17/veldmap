use dashmap::DashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

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
    pub expires_at: Option<Instant>,
}

impl Lease {
    pub fn new(owner_id: u32) -> Self {
        Self { owner_id, readers: Vec::new(), writers: Vec::new(), expires_at: None }
    }

    pub fn can_read(&self, module_id: u32) -> bool {
        self.owner_id == module_id
            || module_id == 0
            || self.readers.contains(&module_id)
            || self.writers.contains(&module_id)
    }

    pub fn can_write(&self, module_id: u32) -> bool {
        (self.owner_id == module_id || module_id == 0 || self.writers.contains(&module_id))
            && self.expires_at.map_or(true, |e| e > Instant::now())
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

    pub fn revoke_all(&mut self) {
        self.readers.clear();
        self.writers.clear();
        self.expires_at = Some(Instant::now());
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
