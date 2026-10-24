pub const INSTANCE: u64 = 1024 * 1024 * 1024;

pub const RESERVE: u64 = 96 * 1024 * 1024;

pub const fn free() -> u64 {
    INSTANCE - RESERVE
}

pub const CHUNK_CACHE: u64 = free() / 8;

#[derive(Clone, Debug, Default)]
pub struct Peak {
    terms: Vec<(&'static str, u64)>,
}

impl Peak {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with(mut self, what: &'static str, bytes: u64) -> Self {
        self.terms.push((what, bytes));
        self
    }

    pub fn total(&self) -> u64 {
        self.terms.iter().map(|(_, bytes)| *bytes).fold(0, u64::saturating_add)
    }

    pub fn fits(&self) -> bool {
        self.total() <= free()
    }

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

    pub fn note(&self) -> String {
        self.terms
            .iter()
            .map(|(what, bytes)| format!("{} {} MB", what, mb(*bytes)))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

pub fn mb(bytes: u64) -> u64 {
    bytes / (1024 * 1024)
}
