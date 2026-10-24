use std::cell::Cell;
use std::time::{Duration, Instant};

#[derive(Default)]
pub struct Toll {
    pub seen: Cell<u64>,
    pub visible: Cell<u64>,
}

impl Toll {
    pub fn level(&self, seen: u64, visible: u64) {
        self.seen.set(self.seen.get() + seen);
        self.visible.set(self.visible.get() + visible);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Pass {
    Camera,
    Fetch,
    Set,
    Patches,
    Answer,
}

impl Pass {
    const ALL: [Pass; 5] = [Pass::Camera, Pass::Fetch, Pass::Set, Pass::Patches, Pass::Answer];

    fn named(self) -> &'static str {
        match self {
            Pass::Camera => "camera",
            Pass::Fetch => "fetch",
            Pass::Set => "set",
            Pass::Patches => "patches",
            Pass::Answer => "answer",
        }
    }

    fn at(self) -> usize {
        Pass::ALL.iter().position(|kind| *kind == self).expect("reason from its own enumeration")
    }
}

const QUIET_FRAMES: u32 = 12;

const LONGEST: Duration = Duration::from_secs(5);

#[derive(Default)]
struct Burst {
    began: Option<Instant>,
    until: Option<Instant>,
    frames: u32,
    quiet: u32,
    passes: [u32; Pass::ALL.len()],
    seen: u64,
    visible: u64,
    spent: Duration,
    frame_spent: Duration,
    frame_passes: u32,
    worst: Duration,
    rebuilds: u32,
    vertices: u64,
    rebuild_spent: Duration,
}

#[derive(Default)]
pub struct Meter {
    burst: Burst,
}

impl Meter {
    pub fn pass(&mut self, from: Pass, toll: &Toll, spent: Duration, now: Instant) {
        let seen = toll.seen.get();
        if seen == 0 {
            return;
        }
        let burst = &mut self.burst;
        burst.began.get_or_insert(now);
        burst.until = Some(now);
        burst.passes[from.at()] += 1;
        burst.seen += seen;
        burst.visible += toll.visible.get();
        burst.spent += spent;
        burst.frame_spent += spent;
        burst.frame_passes += 1;
    }

    pub fn rebuilt(&mut self, vertices: usize, spent: Duration) {
        let burst = &mut self.burst;
        if burst.began.is_none() || vertices == 0 {
            return;
        }
        burst.rebuilds += 1;
        burst.vertices += vertices as u64;
        burst.rebuild_spent += spent;
        burst.frame_spent += spent;
    }

    pub fn frame(&mut self, now: Instant) -> Option<String> {
        let burst = &mut self.burst;
        burst.worst = burst.worst.max(burst.frame_spent);
        burst.frame_spent = Duration::ZERO;
        match burst.frame_passes {
            0 => burst.quiet = burst.quiet.saturating_add(1),
            _ => {
                burst.frames += 1;
                burst.quiet = 0;
            }
        }
        burst.frame_passes = 0;

        let began = burst.began?;
        let quieted = burst.quiet > QUIET_FRAMES;
        if !quieted && now.duration_since(began) < LONGEST {
            return None;
        }

        let lasted = burst.until.unwrap_or(now).duration_since(began);
        let carried = burst.report(lasted);
        let empty = burst.passes.iter().sum::<u32>() == 0;
        self.burst = Burst { began: (!quieted).then_some(now), ..Burst::default() };
        match empty {
            true => None,
            false => Some(carried),
        }
    }
}

impl Burst {
    fn report(&self, lasted: Duration) -> String {
        let rebuilt = match self.rebuilds {
            0 => String::new(),
            _ => format!(
                "; rebuilds {}, vertices {} avg., {:.1} ms per rebuild",
                self.rebuilds,
                self.vertices / u64::from(self.rebuilds),
                self.rebuild_spent.as_secs_f32() * 1000.0 / self.rebuilds as f32,
            ),
        };
        let named: Vec<String> = Pass::ALL
            .iter()
            .filter(|kind| self.passes[kind.at()] > 0)
            .map(|kind| format!("{} {}", kind.named(), self.passes[kind.at()]))
            .collect();
        let total: u32 = self.passes.iter().sum();
        format!(
            "grid walk: {:.2} s, {} frames, {} walks ({}), cells {}, of them visible {}{}; \
             per frame {:.1} ms, worst {:.1}",
            lasted.as_secs_f32(),
            self.frames,
            total,
            named.join(", "),
            self.seen,
            self.visible,
            rebuilt,
            self.spent.as_secs_f32() * 1000.0 / self.frames.max(1) as f32,
            self.worst.as_secs_f32() * 1000.0,
        )
    }
}
