//! Library state cache — for rendering only.
//!
//! There is no catalogue, no suffixes, no sidecar here and there cannot be:
//! data-library knows all that, and what arrives here is already a derived list
//! of entries. Our business is to keep the last sent list and find an entry in
//! it.

use crate::proto::data_library::{LibraryEntry, LibraryStatus};

#[derive(Default)]
pub struct LibraryState {
    /// The last sent state (data-library/on_state). Replaced wholesale, not
    /// patched: the library broadcasts it on every change, and keeping our own
    /// version of the truth here would be a second source of truth.
    pub entries: Vec<LibraryEntry>,
}

impl LibraryState {
    /// The entry by the provider key — for the Browse/Search screens, where the
    /// row comes from the provider catalogue and knows nothing about the disk.
    pub fn by_identifier(&self, identifier: &str) -> Option<&LibraryEntry> {
        if identifier.is_empty() { return None; }
        self.entries.iter().find(|e| e.identifier == identifier)
    }

    /// The entry name if this key already lies on the disk in full — that is,
    /// what to open with: downloaded content is opened by the library, the rest
    /// by the provider.
    ///
    /// One rule for all open paths (viewing a row, an overlay raster): the file
    /// is at hand, and going over the network for it means waiting for what is
    /// already there. A partially downloaded one does not get here — a `.part`
    /// has neither an end nor a header, and the tile pipeline needs the whole
    /// file.
    pub fn local_name(&self, identifier: &str) -> Option<&str> {
        self.by_identifier(identifier)
            .filter(|entry| status_of(entry) == LibraryStatus::LibComplete)
            .map(|entry| entry.name.as_str())
    }

    /// What lies under this catalogue path and whether anything is being done
    /// there. That is how a network-catalogue folder learns everything about
    /// itself that can be learned at all: the catalogue has no answer to this,
    /// but the library does — the key of every entry carries the path it was
    /// downloaded from.
    ///
    /// One pass and one answer, because they are asked together: a folder row
    /// says either "so much on disk" or "downloading", and the choice between
    /// these two words is the whole question.
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

    /// Snapshot completeness: how many of its files are done and how many there
    /// are in the snapshot in total. Zero for the second number — the snapshot
    /// has never been traversed, and there is nothing to call it whole by: what
    /// is done is "everything that was downloaded", and what was downloaded is
    /// not necessarily the whole snapshot.
    ///
    /// The maximum over the entries, not an arbitrary one: the number comes from
    /// outside and reaches disk-backed entries through the files' sidecars, so at
    /// a moment between the two some entries do not know it yet (0), while the
    /// one that knows it knows it correctly.
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

    /// The snapshot lies on the disk in full.
    ///
    /// In full, not "with anything at all": this is asked where the answer
    /// decides whether to offer a raster that can only be read by a full pass over
    /// the file (see `ImageryRequest.downloaded`). A half-downloaded snapshot may
    /// well lack exactly that file on disk, and the promise "from the disk it
    /// takes seconds" would turn into a quarter of an hour over the wire.
    ///
    /// "How many files there should be" is known only to the sidecar
    /// (`siblings`), and without it every snapshot whose downloaded part is
    /// complete would look whole.
    pub fn whole(&self, product: &str) -> bool {
        let (done, files) = self.snapshot(product);
        files > 0 && done as u32 == files
    }

    /// How many bytes lie on the disk — counting the partially downloaded ones,
    /// they take up space too.
    pub fn stored(&self) -> u64 {
        self.entries.iter().map(|entry| entry.done).sum()
    }

    /// Running downloads: how many, how much done, and how much in total. One
    /// pass, because they are shown together too — as one status row.
    pub fn downloading(&self) -> (usize, u64, u64) {
        self.entries
            .iter()
            .filter(|entry| status_of(entry) == LibraryStatus::LibDownloading)
            .fold((0, 0, 0), |(count, done, total), entry| {
                (count + 1, done + entry.done, total + entry.total)
            })
    }
}

/// What the library knows about the content of one catalogue folder.
#[derive(Default, Clone, Copy)]
pub struct Under {
    /// How many of its entries are registered — counting the partially
    /// downloaded ones: they already take up space.
    pub files: usize,
    /// How many bytes of those lie on the disk.
    pub bytes: u64,
    /// Whether at least one file is transferring right now. While that is so,
    /// the whole snapshot is transferring — by the same rule by which the
    /// collapsed "Downloaded" row speaks about itself (see
    /// `components::row::snapshot`): a sum over parts would be true about the
    /// files, not about the snapshot.
    pub active: bool,
}

/// The entry status as an enum, not as a raw i32 from protobuf.
pub fn status_of(entry: &LibraryEntry) -> LibraryStatus {
    LibraryStatus::try_from(entry.status).unwrap_or(LibraryStatus::LibPaused)
}
