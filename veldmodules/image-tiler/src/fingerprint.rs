//! Content fingerprint — the identity of a source for the tile cache.
//!
//! Derived from the bytes themselves: length, first and last 64 KiB. So a
//! file on disk and a remote object with the same bytes share one fingerprint
//! — the cache survives the "watched over the network → downloaded"
//! transition — while no identity agreement between modules exists: there is
//! nothing to forge and nothing to disagree on. An incompletely downloaded
//! `.part` honestly yields a different fingerprint: it has a different length.
//!
//! Head and tail are chosen by cost, not by robustness: the decoder almost
//! always needs them anyway (headers, TIFF IFDs live at both ends), and after
//! the first read they sit in the network block cache. A collision requires
//! matching length, head, and tail with a different middle — for a preview
//! cache this is enough, and it is a deliberate boundary.
//!
//! The suffix encodes the layout and the production rules
//! (`-t<tile side>q<decode revision>`): changing either changes the key, and
//! old cache directories simply age until eviction — no versions, no
//! migrations. The named variable of a many-variable file enters the fold:
//! same bytes, different variable — different tiles.

use std::io::{Read, Seek, SeekFrom};

use super::pyramid::TILE;

/// How many bytes are taken from each edge.
const SAMPLE: u64 = 64 * 1024;

/// Revision of the decode rules. Raised by an edit that changes tile content
/// for the same source bytes — stretching of wide formats, "no data" keying,
/// weighting of the reduction — otherwise old tiles of the same picture would
/// surface from the cache next to the new ones.
const DECODE_REV: u32 = 12;

/// Resource fingerprint: `<fnv64 hex>-t<TILE>q<DECODE_REV>`. `variable` is the
/// named variable of a many-variable file; empty means the tiler's choice.
pub fn fingerprint(resource_id: u64, len: u64, variable: &str) -> Result<String, String> {
    let mut reader = veldsdk::ResourceReader::new(resource_id, len);
    let (head, tail) = edges(len);
    let head = read_exact_at(&mut reader, head.0, head.1)?;
    let tail = match tail {
        Some((from, size)) => read_exact_at(&mut reader, from, size)?,
        None => Vec::new(),
    };
    Ok(hash_of(len, &head, &tail, variable))
}

/// The "length ∥ head ∥ tail ∥ variable" fold over ready-made slices — one
/// for resource reads and for tests alike, which have no resource.
///
/// The variable goes into the fold, not into the key text: a cache name
/// allows only `[a-z0-9-]`, while variable paths carry slashes and capitals.
/// An empty one (the tiler's choice) does not enter the fold: a file shown by
/// the tiler's choice keeps its tiles in the cache, and `DECODE_REV` needs no
/// bump.
fn hash_of(len: u64, head: &[u8], tail: &[u8], variable: &str) -> String {
    let mut hash = Fnv::new();
    hash.update(&len.to_le_bytes());
    hash.update(head);
    hash.update(tail);
    if !variable.is_empty() {
        hash.update(variable.as_bytes());
    }
    format!("{:016x}-t{}q{}", hash.finish(), TILE, DECODE_REV)
}

/// Which pieces of the file enter the fingerprint: the head and the tail it
/// does not cover.
///
/// The tail is a separate answer, not a second piece on par with the head: a
/// short file has no second piece at all, rather than "the same piece once
/// more". There must be no overlap — not for speed but for meaning: with an
/// overlap the fingerprint would hash the shared bytes twice and stop
/// matching the declared "length ∥ head ∥ tail" rule. Silently: files of
/// 64–128 KiB would change fingerprint, and the whole tile cache for them
/// would miss forever.
fn edges(len: u64) -> ((u64, u64), Option<(u64, u64)>) {
    let head = (0, SAMPLE.min(len));
    let tail = (len > SAMPLE).then(|| {
        let from = (len - SAMPLE).max(SAMPLE);
        (from, len - from)
    });
    (head, tail)
}

fn read_exact_at(reader: &mut veldsdk::ResourceReader, from: u64, size: u64) -> Result<Vec<u8>, String> {
    reader.seek(SeekFrom::Start(from)).map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; size as usize];
    reader
        .read_exact(&mut buf)
        .map_err(|e| format!("fingerprint: reading {} bytes at offset {}: {}", size, from, e))?;
    Ok(buf)
}

/// FNV-1a, 64 bits. Hand-rolled rather than a crate: twenty lines against a
/// dependency, and the fingerprint needs no cryptographic strength by
/// construction (see above).
struct Fnv(u64);

impl Fnv {
    fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }

    fn update(&mut self, data: &[u8]) {
        for &byte in data {
            self.0 ^= u64::from(byte);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }

    fn finish(&self) -> u64 {
        self.0
    }
}
