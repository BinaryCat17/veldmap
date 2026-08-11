//! Счётчик кадров.
//!
//! Меряет по собственным часам, а не по `dt`, которые присылает хост. Сумма
//! присланных `dt` — это время хоста, и делённое на число кадров она даёт темп
//! хоста, как бы медленно мы их ни разбирали: события кадров рано или поздно
//! обрабатываются все, поэтому такая «частота» совпадала бы с оконной всегда и
//! показать ничего другого не могла. Собственные часы показывают наш темп, и
//! разошедшись с оконным, они говорят ровно то, что нужно знать: события
//! приходят быстрее, чем мы их разбираем.
//!
//! Кроме среднего — самый короткий и самый долгий промежуток между кадрами:
//! средним хорошо видно нагрузку, но не видно заминок, а заминка на кадр в
//! четверть секунды заметна глазу и в среднем за пять секунд не проступает.

use std::time::{Duration, Instant};

/// Как часто отчитываться.
const WINDOW: Duration = Duration::from_secs(5);

pub struct FrameMeter {
    window_started: Instant,
    /// Прошлый кадр; переживает границу окна, иначе первый промежуток нового
    /// окна оставался бы неизмеренным.
    previous: Option<Instant>,
    frames: u32,
    shortest: Duration,
    longest: Duration,
}

impl FrameMeter {
    pub fn new() -> Self {
        Self {
            window_started: Instant::now(),
            previous: None,
            frames: 0,
            shortest: Duration::MAX,
            longest: Duration::ZERO,
        }
    }

    /// Отмечает кадр и, когда окно закрылось, отдаёт строку отчёта.
    pub fn tick(&mut self) -> Option<String> {
        let now = Instant::now();
        if let Some(previous) = self.previous {
            let gap = now.duration_since(previous);
            self.shortest = self.shortest.min(gap);
            self.longest = self.longest.max(gap);
        }
        self.previous = Some(now);
        self.frames += 1;

        let elapsed = now.duration_since(self.window_started);
        if elapsed < WINDOW {
            return None;
        }

        let report = format!(
            "{:.1} FPS в среднем за {:.1}с, по кадрам от {:.0} до {:.0}",
            self.frames as f32 / elapsed.as_secs_f32().max(f32::EPSILON),
            elapsed.as_secs_f32(),
            hz(self.longest),
            hz(self.shortest),
        );

        self.window_started = now;
        self.frames = 0;
        self.shortest = Duration::MAX;
        self.longest = Duration::ZERO;
        Some(report)
    }
}

/// Промежуток между кадрами в частоту. Долгий промежуток даёт нижнюю границу,
/// короткий — верхнюю, поэтому «минимум» и «максимум» здесь меняются местами.
fn hz(gap: Duration) -> f32 {
    let seconds = gap.as_secs_f32();
    if seconds > 0.0 { 1.0 / seconds } else { 0.0 }
}
