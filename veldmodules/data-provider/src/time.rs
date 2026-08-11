//! Время в обе стороны: ISO 8601 ↔ unix-секунды.
//!
//! Общее для хранилища и каталога: первое отдаёт время в XML, второй в JSON, но
//! пишут они его одинаково, и одинаково же оно уходит наружу — числом (см.
//! `ListEntry.modified` и `DataProduct.acquired` в types.proto).
//!
//! Алгоритм Хиннанта в обе стороны: сдвиг эры на 1 марта делает год без
//! високосного дня непрерывным, и день эпохи считается без таблиц и без цикла
//! по годам.

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

    let year = year - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;

    days * 86_400 + hour * 3_600 + minute * 60 + second
}

/// unix-секунды → `2024-05-04T08:23:58.000Z`.
///
/// Доли секунды всегда нулевые: наружу время уходит целыми секундами, а такой
/// вид каталог требует буквально — время без миллисекунд он отвергает как
/// негодный запрос.
pub fn format(seconds: i64) -> String {
    let days = seconds.div_euclid(86_400);
    let time = seconds.rem_euclid(86_400);

    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    // Номер месяца в сдвинутом году: 0 — март, 11 — февраль.
    let shifted = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted + 2) / 5 + 1;
    let month = shifted + if shifted < 10 { 3 } else { -9 };
    let year = year_of_era + era * 400 + i64::from(month <= 2);

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.000Z",
        year, month, day, time / 3_600, time / 60 % 60, time % 60,
    )
}
