pub fn parse(text: &str) -> i64 {
    let number = |range: std::ops::Range<usize>| text.get(range).and_then(|part| part.parse::<i64>().ok());
    let (Some(year), Some(month), Some(day)) = (number(0..4), number(5..7), number(8..10)) else {
        return 0;
    };
    let (Some(hour), Some(minute), Some(second)) = (number(11..13), number(14..16), number(17..19)) else {
        return 0;
    };

    veldsdk::time::days_from_civil(year, month, day) * 86_400
        + hour * 3_600 + minute * 60 + second
}

pub fn format(seconds: i64) -> String {
    let (year, month, day) = veldsdk::time::civil_from_unix(seconds);
    let time = seconds.rem_euclid(86_400);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.000Z",
        year, month, day, time / 3_600, time / 60 % 60, time % 60,
    )
}
