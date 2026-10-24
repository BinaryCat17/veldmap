const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];

pub fn bytes(value: u64) -> String {
    let (scaled, unit) = scale(value);
    if unit == 0 || scaled >= 10.0 {
        format!("{:.0} {}", scaled, UNITS[unit])
    } else {
        format!("{:.1} {}", scaled, UNITS[unit]).replace('.', ",")
    }
}

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

pub fn head(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit || limit < 4 {
        return text.to_string();
    }
    let mut kept: String = text.chars().take(limit - 1).collect();
    kept.truncate(kept.trim_end().len());
    format!("{}…", kept)
}

pub fn variable_name(path: &str) -> &str {
    path.rsplit_once('/').map_or(path, |(_, name)| name)
}

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
    while head + tail < shortest && same(&|name: &Vec<char>| name.get(name.len() - 1 - tail).copied())
    {
        tail += 1;
    }
    while head + tail >= shortest && tail > 0 {
        tail -= 1;
    }
    if head + tail >= shortest {
        return (0, 0);
    }
    (if head > 1 { head } else { 0 }, if tail > 1 { tail } else { 0 })
}

pub fn distinct(text: &str, limit: usize, shared: (usize, usize)) -> String {
    let (head, tail) = shared;
    let chars: Vec<char> = text.chars().collect();
    if (head == 0 && tail == 0) || head + tail >= chars.len() || limit < 4 {
        return ellipsize(text, limit);
    }
    let body: Vec<char> = chars[head..chars.len() - tail].to_vec();
    let trail = usize::from(tail > 0);

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

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs() as i64)
        .unwrap_or(0)
}

pub fn plural(count: usize, forms: [&'static str; 3]) -> &'static str {
    let tail = count % 100;
    match (tail, tail % 10) {
        (11..=14, _) => forms[2],
        (_, 1) => forms[0],
        (_, 2..=4) => forms[1],
        _ => forms[2],
    }
}

pub fn counted(parts: &[(usize, [&'static str; 3])]) -> String {
    parts
        .iter()
        .filter(|(count, _)| *count > 0)
        .map(|(count, forms)| format!("{} {}", count, plural(*count, *forms)))
        .collect::<Vec<String>>()
        .join(", ")
}

pub fn mono_fit(width: f32, size: f32) -> usize {
    const ADVANCE: f32 = 0.6;
    (width / (size * ADVANCE)).max(0.0) as usize
}

pub fn text_width(text: &str, size: f32) -> f32 {
    const ADVANCE: f32 = 0.5;
    text.chars().count() as f32 * size * ADVANCE
}
