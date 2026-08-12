//! Время в обе стороны: ISO 8601 ↔ unix-секунды.
//!
//! Общее для хранилища и каталога: первое отдаёт время в XML, второй в JSON, но
//! пишут они его одинаково, и одинаково же оно уходит наружу — числом (см.
//! `ListEntry.modified` и `DataProduct.acquired` в types.proto).
//!
//! Здесь только текстовый вид: календарная арифметика — `veldsdk::time`,
//! одна на всех её потребителей.

/// `2024-05-04T08:23:58.000Z` → unix-секунды.
///
/// Смещения в ответах не бывает: время всегда всемирное, поэтому разбираются
/// только цифры на своих местах. Непонятная строка — ноль, то есть «время
/// неизвестно»: подписи у файла не будет, но листинг из-за этого пропадать не
/// должен.
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

/// unix-секунды → `2024-05-04T08:23:58.000Z`.
///
/// Доли секунды всегда нулевые: наружу время уходит целыми секундами, а такой
/// вид каталог требует буквально — время без миллисекунд он отвергает как
/// негодный запрос.
pub fn format(seconds: i64) -> String {
    let (year, month, day) = veldsdk::time::civil_from_unix(seconds);
    let time = seconds.rem_euclid(86_400);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.000Z",
        year, month, day, time / 3_600, time / 60 % 60, time % 60,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_format_are_inverse() {
        for text in ["1970-01-01T00:00:00.000Z", "2024-05-04T08:23:58.000Z", "1999-12-31T23:59:59.000Z"] {
            assert_eq!(format(parse(text)), text);
        }
    }

    /// Непонятная строка — «время неизвестно», а не паника и не мусорная дата.
    #[test]
    fn garbage_reads_as_unknown() {
        for text in ["", "вчера", "2024-05-04", "2024-05-04Tкаша"] {
            assert_eq!(parse(text), 0, "«{}»", text);
        }
    }
}
