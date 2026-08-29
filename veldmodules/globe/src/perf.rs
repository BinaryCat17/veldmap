//! Чего стоит пересчёт желаемого.
//!
//! Мерить приходится не «сколько в среднем», а «сколько в тот миг, когда это
//! происходит». Обход сетки идёт не ровным фоном, а всплесками: жест длится
//! секунду с небольшим, а между жестами ворота (`module::build_patches`)
//! отсекают счёт до всего дорогого. Средним за час такой всплеск не виден
//! вовсе, а он и есть то единственное, что роняет кадр.
//!
//! Поэтому отчёт — по концу всплеска. Считается он по кадрам, в которых обход
//! был, а не по всем подряд, — и вот это единственное, что спасает среднее от
//! разбавления покоем. Всплеск нужен остальному: длине работы, худшему кадру и
//! разбивке по поводам. Собранные окном по часам, они смешали бы жест колесом,
//! перелёт наводки и приход тайлов в одну строку, из которой не видно, чем
//! вызвана ни одна из них.
//!
//! Отчёт живёт на кадровом тике, поэтому всплеск, не закрывшийся до того, как
//! тики кончились, не печатается вовсе: закрытое окно и отобранное место
//! уносят последний из них. Своего таймера у модуля нет, и взять его негде.
//!
//! Часы приходят доводом, а не берутся внутри: так тип проверяется тестом, не
//! дожидаясь настоящих секунд.

use std::cell::Cell;
use std::time::{Duration, Instant};

/// Цена ответа: сколько ячеек проверили и сколько из них оказалось видно.
///
/// Две величины, а не одна, потому что вопрос к ним один: имеет ли смысл
/// перестать обходить сетку целиком. Спуск по дереву экономит ровно на
/// отвергнутых — если видно почти всё проверенное, экономить нечего, сколько
/// бы времени обход ни занимал.
///
/// `Cell`, а не `&mut`: обход зовётся из замыкания, которое `tiles::want`
/// принимает как `Fn`.
#[derive(Default)]
pub struct Toll {
    pub seen: Cell<u64>,
    pub visible: Cell<u64>,
}

impl Toll {
    /// Отметить пройденный уровень: столько ячеек проверено, столько видно.
    pub fn level(&self, seen: u64, visible: u64) {
        self.seen.set(self.seen.get() + seen);
        self.visible.set(self.visible.get() + visible);
    }
}

/// Повод, по которому считали желаемое.
///
/// Поводов пять, а не один, потому что всплеск от пришедшего тайла и всплеск
/// от жеста стоят одинаково, а значат разное: первый неизбежен, второй —
/// цена движения руки. Слитые в одно число, они не дали бы отличить одно от
/// другого.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Pass {
    /// Двинулась камера — жест либо наводка.
    Camera,
    /// Сдвинулся ход добычи: кэш закрыл заказ, кончился проход производителя.
    /// Приехавший тайл сюда не идёт — он двигает поколение хранилища, и обход
    /// за него делает [`Pass::Patches`] со следующего кадра.
    Fetch,
    /// Сменился состав: место под кадр, набор наложений, описание растра.
    Set,
    /// Пересборка патчей на кадровом тике.
    Patches,
    /// Проверка ответа кэша: что из присланного ещё нужно.
    Answer,
}

impl Pass {
    /// Все поводы в порядке печати.
    const ALL: [Pass; 5] = [Pass::Camera, Pass::Fetch, Pass::Set, Pass::Patches, Pass::Answer];

    fn named(self) -> &'static str {
        match self {
            Pass::Camera => "камера",
            Pass::Fetch => "добыча",
            Pass::Set => "набор",
            Pass::Patches => "патчи",
            Pass::Answer => "ответ",
        }
    }

    fn at(self) -> usize {
        Pass::ALL.iter().position(|kind| *kind == self).expect("повод из своего же перечисления")
    }
}

/// Сколько кадров подряд без единого обхода считать концом всплеска.
///
/// Не ноль: движение мыши хост коалесцирует до одного события на кадр, и
/// пропущенный кадр посреди перетаскивания — обычное дело. Разорванный на нём
/// всплеск дал бы два отчёта вместо одного и оба с неверным знаменателем.
///
/// Дюжина — запас, а не выведенное число: проверкой закреплено лишь то, что
/// кадр без обхода жеста не кончает.
const QUIET_FRAMES: u32 = 12;

/// Предел длины всплеска.
///
/// Всплеск, идущий дольше, — уже не жест, а непрерывная работа, и отчёт по его
/// концу пришёл бы слишком поздно, чтобы связаться с причиной. Тогда отчёт
/// печатается по пределу, а всплеск продолжается дальше.
const LONGEST: Duration = Duration::from_secs(5);

#[derive(Default)]
struct Burst {
    began: Option<Instant>,
    /// Последний кадр, в котором обход был. Им и меряется длина всплеска:
    /// хвост тишины, по которому его конец опознан, к работе не относится и
    /// растянул бы её на треть секунды.
    until: Option<Instant>,
    /// Кадры с обходами. Тихие сюда не идут: «столько-то на кадр» — это про
    /// кадр движения, а поделённое на кадры покоя оно занижает во столько раз,
    /// сколько их случилось.
    frames: u32,
    quiet: u32,
    passes: [u32; Pass::ALL.len()],
    seen: u64,
    visible: u64,
    spent: Duration,
    /// Копится с начала кадра и складывается в `worst` на его конце: кадр
    /// роняет пара обходов, а не самый долгий из них поодиночке.
    frame_spent: Duration,
    frame_passes: u32,
    worst: Duration,
    /// Пересборки буфера патчей: сколько их было, сколько вершин собрано всего
    /// и чего это стоило. Отдельно от обходов, потому что это другая работа и
    /// другой порядок цены: обход считает ячейки, пересборка — вершины, и
    /// сложенные в одно число они не дали бы различить дорогой жест от дорогой
    /// сетки.
    rebuilds: u32,
    vertices: u64,
    rebuild_spent: Duration,
}

#[derive(Default)]
pub struct Meter {
    burst: Burst,
}

impl Meter {
    /// Отметить обход и его цену.
    ///
    /// Проход, не обошедший ни одной ячейки, проходом не считается. Такие
    /// бывают и стоят почти ничего: жест над пустым шаром зовёт пересчёт на
    /// каждое событие камеры, а обходить там нечего — набор пуст, скрыт или
    /// ещё не описан. Записанные, они завели бы всплеск, которому нечего
    /// рассказать, и разбавили бы среднее нулями.
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

    /// Отметить пересборку буфера патчей: столько вершин собрано, столько это
    /// стоило.
    ///
    /// Всплеска сама по себе не заводит и вне его теряется: пересборке
    /// предшествует обход, и он его уже открыл, а заведённый ею одной всплеск
    /// мерил бы длину от пересборки до тишины — это не жест.
    ///
    /// Пересборка, собравшая ноль вершин, пересборкой не считается. Такая
    /// бывает ровно одна — та, что снимает последний слой, — и стоит она
    /// ничего, а среднее по вершинам занижает вдвое.
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

    /// Отметить кадр и, когда всплеск кончился, отдать строку отчёта.
    ///
    /// Кадр закрывает счёт своего времени — до него обходы кадра лежат
    /// порознь, и наибольший из них не сказал бы о пропущенном кадре ничего.
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
        // Тишина кончает всплеск, предел длины — только отчёт по нему: работа
        // в этом случае идёт дальше, и следующий отчёт обязан мерить своё
        // время, а не считать его от давно прошедшего начала.
        let quieted = burst.quiet > QUIET_FRAMES;
        if !quieted && now.duration_since(began) < LONGEST {
            return None;
        }

        let lasted = burst.until.unwrap_or(now).duration_since(began);
        let carried = burst.report(lasted);
        // Всплеск без единого обхода не отчитывается. Завестись он может
        // только продлением по пределу — работа кончилась ровно на нём, начало
        // новому уже проставлено, — и тишина следом напечатала бы строку
        // нулей: ноль обходов, ноль ячеек, ноль миллисекунд. Такая строка не
        // говорит ни о чём и читается как поломка.
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
        // Пересборка называется, только если была: у жеста над готовым набором
        // её нет вовсе, и нули в строке читались бы как поломка.
        let rebuilt = match self.rebuilds {
            0 => String::new(),
            _ => format!(
                "; пересборок {}, вершин {} сред., на пересборку {:.1} мс",
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
            "обход сетки: {:.2} с, {} кадров, {} обходов ({}), ячеек {}, из них видно {}{}; \
             на кадр {:.1} мс, худший {:.1}",
            lasted.as_secs_f32(),
            self.frames,
            total,
            named.join(", "),
            self.seen,
            self.visible,
            rebuilt,
            // Знаменатель — кадры всплеска: доля от его длительности сказала
            // бы «сколько времени занято», а роняет кадр не доля, а миллисекунды
            // в нём.
            self.spent.as_secs_f32() * 1000.0 / self.frames.max(1) as f32,
            self.worst.as_secs_f32() * 1000.0,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Цена обхода, разложенная по двум уровням, — как её и приносит
    /// `tiles::want`: он зовёт обход и на целевом уровне, и на каждой ступени
    /// лестницы, и `Toll` обязан их сложить, а не заместить последним.
    fn toll(seen: u64, visible: u64) -> Toll {
        let toll = Toll::default();
        let (head, tail) = (seen / 4, visible / 4);
        toll.level(head, tail);
        toll.level(seen - head, visible - tail);
        toll
    }

    /// Кадры без обходов ничего не открывают, а обход открывает всплеск, и
    /// отчёт приходит ровно один раз — по тишине, а не на каждом следующем
    /// кадре.
    #[test]
    fn a_burst_reports_once_and_only_after_it_falls_quiet() {
        let start = Instant::now();
        let mut meter = Meter::default();
        assert!(meter.frame(start).is_none(), "без обходов отчитываться не о чем");

        meter.pass(Pass::Camera, &toll(100, 10), Duration::from_millis(3), start);
        let mut at = start;
        for step in 1..=QUIET_FRAMES + 1 {
            at += Duration::from_millis(16);
            assert!(meter.frame(at).is_none(), "всплеск ещё не кончился на {}-м тихом кадре", step);
        }
        at += Duration::from_millis(16);
        let report = meter.frame(at).expect("тишина закрыла всплеск");
        assert!(report.contains("камера 1"), "повод назван: {}", report);
        assert!(report.contains("ячеек 100"), "проверенное сосчитано: {}", report);
        assert!(report.contains("видно 10"), "видимое сосчитано: {}", report);

        at += Duration::from_millis(16);
        assert!(meter.frame(at).is_none(), "второй раз тот же всплеск не отчитывается");
    }

    /// Пустой шар молчит. Пересчёт при пустом наборе исполняется на каждом
    /// кадре и стоит почти ничего — записанный проходом, он открывал бы
    /// всплеск на неподвижном пустом шаре и печатал бы строку нулей каждые
    /// несколько секунд.
    #[test]
    fn a_pass_that_walked_nothing_is_not_a_pass() {
        let start = Instant::now();
        let mut meter = Meter::default();
        // Дольше предела длины всплеска: пустой обход идёт на каждом кадре, и
        // тишиной его молчание не объяснить — кончиться такой всплеск может
        // только по пределу, а кончиться он не должен вовсе.
        let frames = LONGEST.as_millis() as u64 / 16 + 60;
        for step in 0..frames {
            meter.pass(Pass::Patches, &toll(0, 0), Duration::from_micros(4), start);
            assert!(
                meter.frame(start + Duration::from_millis(16 * step)).is_none(),
                "пустой обход открыл всплеск на {}-м кадре из {}",
                step,
                frames
            );
        }
    }

    /// Поводы считаются порознь, и в строку идут только те, что были: набор
    /// нулей по всем пяти поводам сказал бы о причине всплеска меньше, чем
    /// один названный повод.
    #[test]
    fn only_the_reasons_that_happened_are_named() {
        let start = Instant::now();
        let mut meter = Meter::default();
        meter.pass(Pass::Camera, &toll(10, 1), Duration::from_millis(1), start);
        meter.pass(Pass::Camera, &toll(10, 1), Duration::from_millis(1), start);
        meter.pass(Pass::Patches, &toll(10, 1), Duration::from_millis(1), start);
        let report = quiet_out(&mut meter, start);
        assert!(report.contains("камера 2"), "повод сосчитан по разу на обход: {}", report);
        assert!(report.contains("патчи 1"), "второй повод назван: {}", report);
        assert!(!report.contains("добыча"), "небывший повод не назван: {}", report);
        assert!(report.contains("3 обходов"), "общий счёт — сумма по поводам: {}", report);
        assert!(report.contains("ячеек 30"), "проверенное копится по обходам: {}", report);
        assert!(report.contains("видно 3"), "видимое копится тоже: {}", report);
    }

    /// Пересборка буфера патчей названа отдельно от обходов: обход считает
    /// ячейки, пересборка — вершины, и стоит она на порядок больше. Слитые в
    /// одно число, дорогой жест и дорогая варп-сетка были бы неразличимы, а
    /// лечатся они разным.
    ///
    /// Пришедшая вне всплеска, она молчит: всплеск заводит обход, и мерка
    /// длины у него от первого обхода до тишины.
    #[test]
    fn a_rebuild_is_counted_apart_from_the_walks() {
        let start = Instant::now();
        let mut meter = Meter::default();

        meter.rebuilt(1_000, Duration::from_millis(9));
        for step in 0..=QUIET_FRAMES + 2 {
            let at = start + Duration::from_millis(16 * u64::from(step));
            assert!(meter.frame(at).is_none(), "пересборка завела всплеск сама на {}-м кадре", step);
        }

        let mut meter = Meter::default();
        meter.pass(Pass::Camera, &toll(10, 1), Duration::from_millis(1), start);
        meter.rebuilt(600_000, Duration::from_millis(20));
        meter.rebuilt(400_000, Duration::from_millis(10));
        // Снятие последнего слоя — тоже пересборка, но собравшая ноль вершин:
        // стои́т она ничего, а среднее по вершинам занижает вдвое.
        meter.rebuilt(0, Duration::from_micros(5));
        let report = quiet_out(&mut meter, start);
        assert!(report.contains("пересборок 2"), "пересборки сосчитаны: {}", report);
        assert!(report.contains("вершин 500000"), "вершины усреднены по ним же: {}", report);
        assert!(report.contains("на пересборку 15.0 мс"), "цена усреднена: {}", report);
        // И она же попадает в худший кадр: роняет его именно она.
        assert!(report.contains("худший 31.0"), "пересборка не вошла в кадр: {}", report);
    }

    /// Худшее — сумма за кадр, а не самый долгий обход. Кадр роняет пара
    /// обходов подряд, и названный по одному из них он выглядел бы уложившимся
    /// в бюджет.
    #[test]
    fn the_worst_frame_is_the_frame_and_not_the_longest_pass() {
        let start = Instant::now();
        let mut meter = Meter::default();
        // Кадр из пары: каждый обход короче одиночного из следующего кадра, а
        // вместе — длиннее.
        meter.pass(Pass::Camera, &toll(10, 1), Duration::from_millis(4), start);
        meter.pass(Pass::Patches, &toll(10, 1), Duration::from_millis(4), start);
        let at = start + Duration::from_millis(16);
        assert!(meter.frame(at).is_none());
        meter.pass(Pass::Camera, &toll(10, 1), Duration::from_millis(5), at);
        let report = quiet_out(&mut meter, at);
        assert!(report.contains("худший 8.0"), "пара кадра перевесила одиночку: {}", report);
    }

    /// Всплеск длиннее предела отчитывается и продолжается: работа-то идёт, и
    /// оборванный счёт потерял бы её остаток.
    #[test]
    fn a_burst_longer_than_the_cap_reports_and_carries_on() {
        let start = Instant::now();
        let mut meter = Meter::default();
        let mut at = start;
        let mut reports = Vec::new();
        // Обход на каждом кадре, дольше предела вдвое.
        for _ in 0..((LONGEST.as_millis() as u64 / 16) * 2 + 4) {
            meter.pass(Pass::Camera, &toll(10, 1), Duration::from_millis(1), at);
            at += Duration::from_millis(16);
            if let Some(report) = meter.frame(at) {
                reports.push(report);
            }
        }
        assert!(reports.len() >= 2, "непрерывная работа отчиталась не раз: {:?}", reports);
        for report in &reports {
            let lasted: f32 = report
                .split_once("обход сетки: ")
                .and_then(|(_, tail)| tail.split_once(' '))
                .and_then(|(head, _)| head.parse().ok())
                .expect("длительность в начале строки");
            assert!(
                lasted <= LONGEST.as_secs_f32() + 0.05,
                "отчёт по пределу мерит своё время, а не от начала работы: {}",
                report
            );
        }
    }

    /// Кадр без обхода посреди жеста всплеска не разрывает.
    ///
    /// Разорванный, он дал бы два отчёта вместо одного, и оба с неверным
    /// знаменателем: движение мыши хост коалесцирует до одного события на
    /// кадр, так что пропущенный кадр посреди перетаскивания — обычное дело, а
    /// не признак конца жеста.
    #[test]
    fn a_frame_without_a_pass_does_not_break_the_burst_in_two() {
        let start = Instant::now();
        let mut meter = Meter::default();
        let mut at = start;
        let mut reports = Vec::new();
        // Обход, ровно один кадр без него, снова обход — и только потом конец.
        // Единица здесь числом, а не через `QUIET_FRAMES`: выраженная
        // константой, она подстроилась бы под любое её значение, включая ноль,
        // и правило осталось бы незакреплённым.
        meter.pass(Pass::Camera, &toll(40, 4), Duration::from_millis(2), at);
        for _ in 0..2 {
            at += Duration::from_millis(16);
            if let Some(report) = meter.frame(at) {
                reports.push(report);
            }
        }
        meter.pass(Pass::Camera, &toll(40, 4), Duration::from_millis(2), at);
        reports.push(quiet_out(&mut meter, at));
        assert_eq!(reports.len(), 1, "тихий кадр разорвал всплеск: {:?}", reports);
        assert!(reports[0].contains("2 кадров"), "оба кадра с обходом сосчитаны: {}", reports[0]);
        assert!(reports[0].contains("ячеек 80"), "обходы сложились: {}", reports[0]);
    }

    /// Работа, кончившаяся ровно на пределе длины, второго отчёта не даёт.
    ///
    /// Отчёт по пределу продлевает всплеск — но продлевать его нечем, если
    /// обходы на этом же кадре и кончились. Продлённый вслепую, он дождался бы
    /// тишины и напечатал строку нулей: ноль обходов, ноль ячеек, ноль
    /// миллисекунд. Такая строка не говорит ни о чём и выглядит как поломка.
    #[test]
    fn work_that_stops_at_the_cap_does_not_leave_an_empty_burst_behind() {
        let start = Instant::now();
        let mut meter = Meter::default();
        let mut at = start;
        let mut reports = Vec::new();
        // Обход на каждом кадре, ровно до предела и ни кадром дольше.
        while at.duration_since(start) < LONGEST {
            meter.pass(Pass::Camera, &toll(10, 1), Duration::from_millis(1), at);
            at += Duration::from_millis(16);
            if let Some(report) = meter.frame(at) {
                reports.push(report);
            }
        }
        // Дальше только тишина.
        for _ in 0..(QUIET_FRAMES + 3) {
            at += Duration::from_millis(16);
            if let Some(report) = meter.frame(at) {
                reports.push(report);
            }
        }
        assert_eq!(reports.len(), 1, "лишний отчёт после конца работы: {:?}", reports);
        assert!(!reports[0].contains("0 обходов"), "строка нулей: {}", reports[0]);
    }

    /// Длина всплеска меряется по работе, а не по тишине, которой её опознали.
    /// Тишина эта — дюжина кадров, пятая доля секунды; приписанная к жесту в
    /// секунду, она занизила бы всё, что делится на длительность.
    #[test]
    fn a_burst_is_as_long_as_the_work_and_not_as_the_hush_that_ended_it() {
        let start = Instant::now();
        let mut meter = Meter::default();
        let mut at = start;
        // Работы — ровно сто миллисекунд.
        for _ in 0..6 {
            meter.pass(Pass::Camera, &toll(10, 1), Duration::from_millis(1), at);
            assert!(meter.frame(at).is_none());
            at += Duration::from_millis(20);
        }
        let report = quiet_out(&mut meter, at);
        assert!(report.contains("0.10 с"), "тишина в длину всплеска не вошла: {}", report);
    }

    /// Среднее делится на кадры всплеска. Поделённое на число обходов, оно
    /// назвало бы цену одного обхода — а кадр их держит два, и уложится ли он
    /// в бюджет, по цене одного не видно.
    #[test]
    fn the_average_is_per_frame_and_not_per_pass() {
        let start = Instant::now();
        let mut meter = Meter::default();
        let mut at = start;
        // Три кадра, в каждом по два обхода по 2 мс: на кадр 4 мс, на обход 2.
        for _ in 0..3 {
            meter.pass(Pass::Camera, &toll(10, 1), Duration::from_millis(2), at);
            meter.pass(Pass::Patches, &toll(10, 1), Duration::from_millis(2), at);
            at += Duration::from_millis(16);
            assert!(meter.frame(at).is_none());
        }
        let report = quiet_out(&mut meter, at);
        assert!(report.contains("на кадр 4.0 мс"), "среднее — по кадрам: {}", report);
    }

    /// Домолчать всплеск до отчёта.
    fn quiet_out(meter: &mut Meter, from: Instant) -> String {
        let mut at = from;
        for _ in 0..=QUIET_FRAMES + 2 {
            at += Duration::from_millis(16);
            if let Some(report) = meter.frame(at) {
                return report;
            }
        }
        panic!("всплеск не кончился за {} тихих кадров", QUIET_FRAMES + 3);
    }
}
