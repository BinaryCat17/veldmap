//! Time in both directions: ISO 8601 ↔ unix seconds.
//!
//! Shared by the storage and the catalogue: the first serves time in XML, the
//! second in JSON, but they write it the same way, and it goes outward the
//! same way too — as a number (see `ListEntry.modified` and
//! `DataProduct.acquired` in types.proto).
//!
//! Only the textual form lives here: calendar arithmetic is `veldsdk::time`,
//! shared by all of its consumers.

/// `2024-05-04T08:23:58.000Z` → unix seconds.
///
/// Responses never carry offsets: time is always universal, so only digits in
/// their fixed places are parsed. An unrecognizable string is zero, that is
/// "time unknown": the file gets no signature, but the listing must not
/// disappear because of it.
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

/// unix seconds → `2024-05-04T08:23:58.000Z`.
///
/// The fractional seconds are always zero: time goes outward as whole
/// seconds, and the catalogue demands this form literally — it rejects time
/// without milliseconds as a malformed request.
pub fn format(seconds: i64) -> String {
    let (year, month, day) = veldsdk::time::civil_from_unix(seconds);
    let time = seconds.rem_euclid(86_400);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.000Z",
        year, month, day, time / 3_600, time / 60 % 60, time % 60,
    )
}
