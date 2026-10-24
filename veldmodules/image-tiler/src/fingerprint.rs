use std::io::{Read, Seek, SeekFrom};

use super::pyramid::TILE;

const SAMPLE: u64 = 64 * 1024;

const DECODE_REV: u32 = 12;

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
