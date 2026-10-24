//! What recomputing the wanted set costs.
//!
//! What must be measured is not "how much on average" but "how much at the moment it
//! happens". The grid walk comes not as a steady background but in bursts: a gesture lasts
//! just over a second, and between gestures the gate (`module::build_patches`) cuts the
//! count off before anything expensive. An hour-long average cannot see such a burst at
//! all, yet the burst is the only thing that drops a frame.
//!
//! Hence the report fires at the end of a burst. It is counted over the frames in which a
//! walk happened, not over all frames in a row — and that alone saves the average from
//! being diluted by stillness. The burst serves the rest: the work length, the worst frame,
//! and the breakdown by reason. Gathered in an hour-long window, they would blend a wheel
//! gesture, the aiming flight, and tile arrivals into one line where none of the causes
//! shows.
//!
//! The report lives on the frame tick, so a burst that has not closed before the ticks run
//! out is never printed: the closed window and the reclaimed slot carry away the last one.
//! The module has no timer of its own and nowhere to get one.
//!
//! The clock arrives as an argument rather than being taken inside: this way the type is
//! checked by a test without waiting for real seconds.

use std::cell::Cell;
use std::time::{Duration, Instant};

/// The cost of an answer: how many cells were checked and how many of them turned out
/// visible.
///
/// Two quantities, not one, because the question put to them is single: is it worth
/// stopping the whole-grid walk. The tree descent saves exactly on the rejected — if
/// almost everything checked is visible, there is nothing to save, however long the walk
/// takes.
///
/// `Cell`, not `&mut`: the walk is called from a closure that `tiles::want` accepts as
/// `Fn`.
#[derive(Default)]
pub struct Toll {
    pub seen: Cell<u64>,
    pub visible: Cell<u64>,
}

impl Toll {
    /// Record a level walked: this many cells checked, this many visible.
    pub fn level(&self, seen: u64, visible: u64) {
        self.seen.set(self.seen.get() + seen);
        self.visible.set(self.visible.get() + visible);
    }
}

/// The reason the wanted set was computed.
///
/// Five reasons, not one, because a burst from an arriving tile and a burst from a gesture
/// weigh the same but mean different things: the first is unavoidable, the second is the
/// price of a hand movement. Merged into one number, they would not let one be told from
/// the other.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Pass {
    /// The camera moved — a gesture or the aiming flight.
    Camera,
    /// The fetch course shifted: the cache closed an order, the producer pass ended. An
    /// arriving tile does not go here — it advances the store generation, and
    /// [`Pass::Patches`] does the walk for it from the next frame.
    Fetch,
    /// The set changed: the place under the frame, the overlay set, the raster
    /// description.
    Set,
    /// A patch rebuild on the frame tick.
    Patches,
    /// Checking the cache answer: what of the delivered is still needed.
    Answer,
}

impl Pass {
    /// All the reasons in printing order.
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

/// How many frames in a row without a single walk end the burst.
///
/// Not zero: the host coalesces mouse motion to one event per frame, and a skipped frame
/// in the middle of a drag is routine. A burst broken on it would give two reports instead
/// of one, both with a wrong denominator.
///
/// A dozen is a margin, not a derived number: a check pins down only that a frame without
/// a walk does not end the gesture.
const QUIET_FRAMES: u32 = 12;

/// The cap on burst length.
///
/// A burst lasting longer is no longer a gesture but continuous work, and a report at its
/// end would come too late to be tied to a cause. Then the report is printed at the cap,
/// and the burst carries on.
const LONGEST: Duration = Duration::from_secs(5);

#[derive(Default)]
struct Burst {
    began: Option<Instant>,
    /// The last frame in which a walk happened. The burst length is measured by it: the
    /// tail of silence by which the end is recognized does not belong to the work and
    /// would stretch it by a third of a second.
    until: Option<Instant>,
    /// Frames with walks. Quiet ones do not go here: "so much per frame" is about a frame
    /// of motion, and divided by frames of rest it understates as many times as they
    /// happened.
    frames: u32,
    quiet: u32,
    passes: [u32; Pass::ALL.len()],
    seen: u64,
    visible: u64,
    spent: Duration,
    /// Accumulates from the start of the frame and folds into `worst` at its end: a frame
    /// is dropped by a pair of walks, not by the longest of them alone.
    frame_spent: Duration,
    frame_passes: u32,
    worst: Duration,
    /// Patch buffer rebuilds: how many there were, how many vertices were gathered in all,
    /// and what it cost. Kept apart from the walks, because this is different work at a
    /// different order of cost: a walk counts cells, a rebuild counts vertices, and summed
    /// into one number they would not let an expensive gesture be told from an expensive
    /// grid.
    rebuilds: u32,
    vertices: u64,
    rebuild_spent: Duration,
}

#[derive(Default)]
pub struct Meter {
    burst: Burst,
}

impl Meter {
    /// Record a walk and its cost.
    ///
    /// A pass that walked no cells does not count as a pass. Such happen and cost almost
    /// nothing: a gesture over an empty globe calls a recompute on every camera event, and
    /// there is nothing to walk — the set is empty, hidden, or not yet described. Recorded,
    /// they would start a burst with nothing to tell and dilute the average with zeros.
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

    /// Record a patch buffer rebuild: how many vertices were gathered and what it cost.
    ///
    /// On its own it starts no burst and is lost outside one: a walk precedes a rebuild and
    /// has already opened the burst, while a burst opened by the rebuild alone would measure
    /// its length from the rebuild to the silence — that is not a gesture.
    ///
    /// A rebuild that gathered zero vertices does not count as a rebuild. There is exactly
    /// one such — the one that removes the last layer — and it costs nothing, while halving
    /// the per-vertex average.
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

    /// Record a frame and, when the burst has ended, hand back the report line.
    ///
    /// The frame closes the count of its own time — before it the frame's walks lie apart,
    /// and the largest of them would say nothing about a dropped frame.
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
        // Silence ends the burst; the length cap only reports on it: in that case the work
        // goes on, and the next report must measure its own time rather than count it from
        // a long-past beginning.
        let quieted = burst.quiet > QUIET_FRAMES;
        if !quieted && now.duration_since(began) < LONGEST {
            return None;
        }

        let lasted = burst.until.unwrap_or(now).duration_since(began);
        let carried = burst.report(lasted);
        // A burst without a single walk is not reported. It can only arise from the
        // length-cap extension — the work ended exactly at it, the start for a new one is
        // already set, — and the silence after would print a line of zeros: zero walks,
        // zero cells, zero milliseconds. Such a line says nothing and reads as a fault.
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
        // The rebuild is named only if there was one: a gesture over a ready set has none
        // at all, and zeros in the line would read as a fault.
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
            // The denominator is the burst's frames: a share of its duration would say
            // "how much time is taken", while a frame is dropped not by a share but by the
            // milliseconds in it.
            self.spent.as_secs_f32() * 1000.0 / self.frames.max(1) as f32,
            self.worst.as_secs_f32() * 1000.0,
        )
    }
}
