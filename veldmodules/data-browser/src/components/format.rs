//! components/format.rs — numbers and time in words.
//!
//! One place for the whole application: a size in a list row, in a subtitle,
//! and in the status bar is one and the same size, and it must be spelled the
//! same way everywhere.

/// Units in ascending order; the step is 1024.
const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];

/// "58,7 MB", "42 KB", "4,2 GB". The decimal comma, as in the rest of the
/// interface; the fractional part only where it tells things apart: on large
/// numbers it is noise, on bytes it is absent altogether.
pub fn bytes(value: u64) -> String {
    let (scaled, unit) = scale(value);
    if unit == 0 || scaled >= 10.0 {
        format!("{:.0} {}", scaled, UNITS[unit])
    } else {
        format!("{:.1} {}", scaled, UNITS[unit]).replace('.', ",")
    }
}

/// "51/59 MB", "4,2/9,0 GB" — done out of total. One unit for both numbers:
/// they speak of the same file, and different units side by side read as
/// different quantities. The fractional part follows the same rule as
/// [`bytes`]: on numbers above ten it is noise, and in a running download
/// counter it is flickering noise.
pub fn progress(done: u64, total: u64) -> String {
    let (_, unit) = scale(total);
    let show = |value: u64| {
        let scaled = value as f64 / 1024f64.powi(unit as i32);
        if unit == 0 || scaled >= 10.0 {
            format!("{:.0}", scaled)
        } else {
            format!("{:.1}", scaled).replace('.', ",")
        }
    };
    format!("{}/{} {}", show(done), show(total), UNITS[unit])
}

fn scale(value: u64) -> (f64, usize) {
    let mut scaled = value as f64;
    let mut unit = 0;
    while scaled >= 1024.0 && unit < UNITS.len() - 1 {
        scaled /= 1024.0;
        unit += 1;
    }
    (scaled, unit)
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];
const DAY: i64 = 86_400;

/// "today, 10:41", "yesterday, 14:08", "4 Aug, 09:12", "1 May 1991".
///
/// The further away the event, the coarser the label: the time of a file from
/// today tells it apart from its neighbors, the time of a file from 1991 does
/// not.
///
/// The time is universal, not local: `wasm32-wasip1` has no time zone —
/// neither an offset nor a rule database — and the module has nowhere to take
/// them from.
pub fn date(when: i64, now: i64) -> String {
    if when <= 0 {
        return String::new();
    }
    let (year, month, day) = veldsdk::time::civil_from_unix(when);
    let clock = format!("{:02}:{:02}", when.div_euclid(3600).rem_euclid(24), when.div_euclid(60).rem_euclid(60));

    let today = now.div_euclid(DAY);
    match today - when.div_euclid(DAY) {
        0 => format!("today, {}", clock),
        1 => format!("yesterday, {}", clock),
        _ if year == veldsdk::time::civil_from_unix(now).0 => format!("{} {}, {}", day, MONTHS[month as usize - 1], clock),
        _ => format!("{} {} {}", day, MONTHS[month as usize - 1], year),
    }
}

/// A date typed by hand — the other side of [`date`]: `2026-08-13`, and also
/// `13.08.2026`, because that is typed too. Returns the midnight of that day
/// in unix seconds; `None` — what is typed is not a number yet.
///
/// Parsing lives here, next to the writing: they are one pair, and they have
/// nowhere to diverge.
pub fn parse_date(text: &str) -> Option<i64> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let parts: Vec<&str> = text.split(['-', '.', '/']).collect();
    let [first, month, last] = parts[..] else { return None };
    let (year, day) = match first.len() {
        4 => (first, last),
        _ => (last, first),
    };
    let year: i64 = year.parse().ok()?;
    let month: i64 = month.parse().ok()?;
    let day: i64 = day.parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || !(1970..=9999).contains(&year) {
        return None;
    }
    Some(veldsdk::time::days_from_civil(year, month, day) * DAY)
}

/// Cuts the middle out: a name that shows neither its start nor its tail
/// cannot be recognized at all, while the middle is usually what the neighbors
/// share anyway.
///
/// Cuts blindly: what exactly the neighbors share is known to the list, not to
/// the string (see [`shared`]).
pub fn ellipsize(text: &str, limit: usize) -> String {
    let length = text.chars().count();
    if length <= limit || limit < 4 {
        return text.to_string();
    }
    let head = limit.div_ceil(2) - 1;
    let tail = limit - head - 1;
    let chars: Vec<char> = text.chars().collect();
    format!(
        "{}…{}",
        chars[..head].iter().collect::<String>(),
        chars[length - tail..].iter().collect::<String>(),
    )
}

/// Cuts the tail: a trouble phrase is read from the start — its point is up
/// front and the tail spells out the rest — and a middle cut like on a name
/// would leave two fragments of a verdict. Like [`ellipsize`], it does not cut
/// below four characters.
pub fn head(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit || limit < 4 {
        return text.to_string();
    }
    let mut kept: String = text.chars().take(limit - 1).collect();
    kept.truncate(kept.trim_end().len());
    format!("{}…", kept)
}

/// The name of a variable in a many-variable file — the last segment of its
/// path in the file: `/PRODUCT/carbonmonoxide_total_column` →
/// `carbonmonoxide_total_column`. The groups before it are the file's layout,
/// not its name.
pub fn variable_name(path: &str) -> &str {
    path.rsplit_once('/').map_or(path, |(_, name)| name)
}

/// A variable in words: name, units, and how the file named it — whatever of
/// that the file named: "carbonmonoxide_total_column, mol m-2 — Vertically
/// integrated CO column". The name first: it is shorter and it is how the
/// variable is recognized; units right behind it, because file words can run a
/// line and a half and are cut from the tail ([`head`]) — units behind them
/// would vanish first. Dimensionless CF writes the unit as "1", and
/// "qa_value, 1" would read as a number — such a unit is not spelled out.
pub fn variable(path: &str, said: &str, units: &str) -> String {
    let mut text = variable_name(path).to_string();
    if !units.is_empty() && units.trim() != "1" {
        text = format!("{}, {}", text, units);
    }
    if !said.is_empty() {
        text = format!("{} — {}", text, said);
    }
    text
}

/// Divides `room` characters among parts of lengths `lengths`: the short ones
/// take theirs in full, the long ones split the remainder evenly. Short ones
/// go first: a part whose share is enough hands its unused space to the long
/// ones, and nothing is cut while the sum fits. This is how a tooltip of file
/// name, variable, and trouble is assembled: one line for the three, and each
/// is recognizable in it.
pub fn share(lengths: &[usize], room: usize) -> Vec<usize> {
    let mut order: Vec<usize> = (0..lengths.len()).collect();
    order.sort_by_key(|&at| lengths[at]);
    let mut shares = vec![0; lengths.len()];
    let mut left = room;
    for (rank, &at) in order.iter().enumerate() {
        shares[at] = lengths[at].min(left / (lengths.len() - rank));
        left -= shares[at];
    }
    shares
}

/// What all names on a page share: how many characters match at the start and
/// how many at the tail.
///
/// The shared part distinguishes nothing yet takes space — and takes it right
/// where people look. Copernicus names share exactly the edges: mission with
/// product type at the start, processing baseline with extension at the end,
/// while the middle — sensing time, orbit, tile — is what tells them apart. Cut
/// in the middle, a catalogue page turns into two dozen rows like
/// "S3A_SL_2_A…R_003.SEN3", none of which can be told apart.
///
/// Counted over what is shown, not the whole list: the page is compared by
/// eye. One answer for the whole of it — otherwise neighboring rows would be
/// cut in different places and different things would have to be compared.
///
/// Empty when there is nothing to cut for (the longest name fits anyway) or
/// nothing to cut by (names differ from the first character). The shared part
/// does not swallow a name whole: identical rows have nothing to cut, and then
/// nothing is cut.
pub fn shared(names: &[&str], limit: usize) -> (usize, usize) {
    let Some(longest) = names.iter().map(|name| name.chars().count()).max() else {
        return (0, 0);
    };
    if names.len() < 2 || longest <= limit {
        return (0, 0);
    }
    let shortest = names.iter().map(|name| name.chars().count()).min().unwrap_or(0);
    let chars: Vec<Vec<char>> = names.iter().map(|name| name.chars().collect()).collect();
    let same = |at: &dyn Fn(&Vec<char>) -> Option<char>| -> bool {
        let first = at(&chars[0]);
        first.is_some() && chars.iter().all(|name| at(name) == first)
    };

    let mut head = 0;
    while head < shortest && same(&|name: &Vec<char>| name.get(head).copied()) {
        head += 1;
    }
    let mut tail = 0;
    // The tail must not run into the head: for names that matched whole they
    // would meet in the middle and subtract the same thing twice.
    while head + tail < shortest && same(&|name: &Vec<char>| name.get(name.len() - 1 - tail).copied())
    {
        tail += 1;
    }
    // At least one character must remain of the shortest name: a string of
    // nothing but ellipses is not a name.
    while head + tail >= shortest && tail > 0 {
        tail -= 1;
    }
    if head + tail >= shortest {
        return (0, 0);
    }
    // A cut is marked with an ellipsis, and the character under it is taken
    // from the same place. So an edge shorter than two characters is not worth
    // cutting: it frees nothing and the name starts reading one character
    // worse. That is exactly what comes out on a page of different missions
    // sharing the single letter "S".
    (if head > 1 { head } else { 0 }, if tail > 1 { tail } else { 0 })
}

/// A list row's name: within the allotted number of characters, cutting first
/// what the neighbors share (see [`shared`]).
///
/// Not all of the shared part is cut, only as much as needed: the space freed
/// beyond the distinguishing part returns to the start of the name — that is
/// how the name is recognized. On a Sentinel-2 tile page, three characters of
/// the tile code differ, and a name cut to them is distinguishable but not
/// recognizable; with the start returned it comes out
/// "S2A_MSIL2A_20260…RWV…" — both at once.
///
/// A cut at the edges is marked with an ellipsis just like a cut in the
/// middle: otherwise a shortened name cannot be told from a full one.
pub fn distinct(text: &str, limit: usize, shared: (usize, usize)) -> String {
    let (head, tail) = shared;
    let chars: Vec<char> = text.chars().collect();
    if (head == 0 && tail == 0) || head + tail >= chars.len() || limit < 4 {
        return ellipsize(text, limit);
    }
    let body: Vec<char> = chars[head..chars.len() - tail].to_vec();
    let trail = usize::from(tail > 0);

    // Keep as much of the start as the distinguishing part leaves room for;
    // the leading ellipsis needs a character of its own too.
    let room = limit.saturating_sub(trail);
    let keep = room.saturating_sub(body.len() + 1).min(head);
    let lead = usize::from(keep < head);
    let body: String = body.into_iter().collect();

    format!(
        "{}{}{}{}",
        chars[..keep].iter().collect::<String>(),
        if lead > 0 { "…" } else { "" },
        ellipsize(&body, limit.saturating_sub(keep + lead + trail)),
        if trail > 0 { "…" } else { "" },
    )
}

/// Now, in unix seconds. Needed only for the time label: "today" and
/// "yesterday" do not exist without a reference point.
pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs() as i64)
        .unwrap_or(0)
}

/// "1 file", "2 files", "5 files" — otherwise the counter reads like a stub.
/// The forms come in the order "one", "two", "five".
pub fn plural(count: usize, forms: [&'static str; 3]) -> &'static str {
    let tail = count % 100;
    match (tail, tail % 10) {
        (11..=14, _) => forms[2],
        (_, 1) => forms[0],
        (_, 2..=4) => forms[1],
        _ => forms[2],
    }
}

/// "3 snapshots, 2 files" — the subtitle counters joined with commas.
///
/// One place for all lists: each counts its own (snapshots and folders, layers
/// and outlines), but these numbers are labeled the same way — otherwise
/// neighboring tabs would speak of one and the same thing in different words.
///
/// No empty words in the label: what is not there is not spoken of — so zero
/// counters stay silent, and all zeros make an empty string. Only the view
/// itself can put it into words: an empty folder and an empty "Downloaded" go
/// silent for different reasons.
pub fn counted(parts: &[(usize, [&'static str; 3])]) -> String {
    parts
        .iter()
        .filter(|(count, _)| *count > 0)
        .map(|(count, forms)| format!("{} {}", count, plural(*count, *forms)))
        .collect::<Vec<String>>()
        .join(", ")
}

/// How many monospace characters fit into the allotted width. The character
/// width is derived from the size: for a monospace font it is one for all
/// characters, and this is the only text that can be measured without asking
/// the renderer.
pub fn mono_fit(width: f32, size: f32) -> usize {
    const ADVANCE: f32 = 0.6;
    (width / (size * ADVANCE)).max(0.0) as usize
}

/// How much space a label in the regular font takes — an upper-bound estimate.
///
/// Only the renderer knows the exact width: the font and its metrics live in
/// ui-service, while the module assembles the layout measuring nothing. The
/// estimate is enough where the answer is a threshold — whether the control
/// bar fits into a row or not (see `controls::bar`).
///
/// An upper bound specifically: the main font's Cyrillic takes about 0.46 of
/// the size per character, Latin with digits takes less, and half the size
/// covers both. An error upward turns the bar onto a row a little before it
/// gets tight; an error downward leaves the label cut — exactly what the width
/// is computed for.
pub fn text_width(text: &str, size: f32) -> f32 {
    const ADVANCE: f32 = 0.5;
    text.chars().count() as f32 * size * ADVANCE
}
