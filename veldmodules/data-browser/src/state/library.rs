use crate::proto::data_library::{LibraryEntry, LibraryStatus};

#[derive(Default)]
pub struct LibraryState {
    pub entries: Vec<LibraryEntry>,
}

impl LibraryState {
    pub fn by_identifier(&self, identifier: &str) -> Option<&LibraryEntry> {
        if identifier.is_empty() { return None; }
        self.entries.iter().find(|e| e.identifier == identifier)
    }

    pub fn local_name(&self, identifier: &str) -> Option<&str> {
        self.by_identifier(identifier)
            .filter(|entry| status_of(entry) == LibraryStatus::LibComplete)
            .map(|entry| entry.name.as_str())
    }

    pub fn under(&self, prefix: &str) -> Under {
        if prefix.is_empty() {
            return Under::default();
        }
        self.entries
            .iter()
            .filter(|entry| entry.identifier.starts_with(prefix))
            .fold(Under::default(), |under, entry| Under {
                files: under.files + 1,
                bytes: under.bytes + entry.done,
                active: under.active
                    || status_of(entry) == LibraryStatus::LibDownloading,
            })
    }

    pub fn snapshot(&self, product: &str) -> (usize, u32) {
        if product.is_empty() { return (0, 0); }
        self.entries
            .iter()
            .filter(|entry| entry.product == product)
            .fold((0, 0), |(done, files), entry| {
                let complete = status_of(entry) == LibraryStatus::LibComplete;
                (done + complete as usize, files.max(entry.siblings))
            })
    }

    pub fn whole(&self, product: &str) -> bool {
        let (done, files) = self.snapshot(product);
        files > 0 && done as u32 == files
    }

    pub fn stored(&self) -> u64 {
        self.entries.iter().map(|entry| entry.done).sum()
    }

    pub fn downloading(&self) -> (usize, u64, u64) {
        self.entries
            .iter()
            .filter(|entry| status_of(entry) == LibraryStatus::LibDownloading)
            .fold((0, 0, 0), |(count, done, total), entry| {
                (count + 1, done + entry.done, total + entry.total)
            })
    }
}

#[derive(Default, Clone, Copy)]
pub struct Under {
    pub files: usize,
    pub bytes: u64,
    pub active: bool,
}

pub fn status_of(entry: &LibraryEntry) -> LibraryStatus {
    LibraryStatus::try_from(entry.status).unwrap_or(LibraryStatus::LibPaused)
}
