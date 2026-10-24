//! Instance memory: how much the host grants, how much is set aside as
//! uncountable, and how a job checks its peak against that. One account for
//! every path: the peak is summed from named terms ([`Peak`]) — band, cascade,
//! chunk, cache, region, frame, plane — and checked against [`free`] in
//! [`Peak::admit`]; a refusal names the terms. The spender knows them:
//! [`crate::cascade::bytes`], `Grid`, `netcdf`. The level table
//! (`adapters::table`) computes the same peak up front — the "fits" column.
//! Two shares are assigned from here: the chunk cache ([`CHUNK_CACHE`]) and,
//! through it, the per-tile region cap (`grid::REGION_CAP`).

/// How much linear memory the host grants one instance.
///
/// A copy of `INSTANCE_MEMORY_LIMIT` from the host core, and a copy by
/// necessity: the numbers lie on opposite sides of the wire, and the host
/// constant is invisible to the module. The equality is held by the pair
/// table `buildgen/tests/test_wire_pairs.py` — the compiler cannot link such
/// a pair, while the runtime links it with a trap.
pub const INSTANCE: u64 = 1024 * 1024 * 1024;

/// Always occupied and counted by nobody.
///
/// It covers the module code with its statics, the stack, the HDF5 header
/// cache (`netcdf::METADATA_CACHE`, eight megabytes), and allocator slack:
/// released large blocks return to the allocator, not to the host, and the
/// next large request must land in the freed hole. If it does not land as one
/// contiguous piece, `memory.grow` traps while hundreds of megabytes are
/// formally free.
///
/// The number is assigned, not derived — there is nothing to derive it from:
/// the uncountable is uncountable by definition. It is pinned by tests from
/// both sides instead, and every attempt to spend more runs into them rather
/// than into the instance limit.
pub const RESERVE: u64 = 96 * 1024 * 1024;

/// How much is left for the work on the source.
pub const fn free() -> u64 {
    INSTANCE - RESERVE
}

/// Decoded chunk cache for pointwise reads — a share of the free memory.
///
/// Neighboring tiles of a level sit on the very same source chunks, and
/// without the cache each would be unpacked four times; in bytes, not in
/// items, because chunks come as 256² and as 4096². One eighth — because the
/// peak of a pointwise read sums three such shares at once (the cache itself,
/// the tile region — it lives in the same cache, and the fresh chunk before
/// unpacking), while the remainder goes to parsing the neighboring source and
/// to allocator slack. This number is cache policy, not an invariant: move it,
/// and only the window boundary shifts ([`Peak`] recomputes itself).
pub const CHUNK_CACHE: u64 = free() / 8;

/// Peak memory of one job — as named terms.
///
/// The terms are named so that a refusal says exactly what did not fit and
/// the level table shows what the cost is made of; nobody writes the sum by
/// hand.
#[derive(Clone, Debug, Default)]
pub struct Peak {
    terms: Vec<(&'static str, u64)>,
}

impl Peak {
    pub fn new() -> Self {
        Self::default()
    }

    /// One more term.
    pub fn with(mut self, what: &'static str, bytes: u64) -> Self {
        self.terms.push((what, bytes));
        self
    }

    pub fn total(&self) -> u64 {
        self.terms.iter().map(|(_, bytes)| *bytes).fold(0, u64::saturating_add)
    }

    /// Whether the peak fits in the free memory.
    pub fn fits(&self) -> bool {
        self.total() <= free()
    }

    /// A refusal when it does not fit — with the terms, so that it is visible
    /// which one asks for the most.
    pub fn admit(&self) -> Result<(), String> {
        if self.fits() {
            return Ok(());
        }
        Err(format!(
            "need {} MB with {} MB free ({})",
            mb(self.total()),
            mb(free()),
            self.note()
        ))
    }

    /// The terms in words: "band 200 MB, cascade 56 MB".
    pub fn note(&self) -> String {
        self.terms
            .iter()
            .map(|(what, bytes)| format!("{} {} MB", what, mb(*bytes)))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// Bytes to megabytes, for talking to a human.
pub fn mb(bytes: u64) -> u64 {
    bytes / (1024 * 1024)
}
