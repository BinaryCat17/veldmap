//! Календарь: unix-секунды ↔ гражданская дата (год, месяц, день).
//!
//! Алгоритм Хиннанта: сдвиг эры на 1 марта делает год без високосного дня
//! непрерывным, и день эпохи считается без таблиц и без цикла по годам.
//! Написан один раз и здесь: провайдер переводит им времена каталога,
//! интерфейс подписывает файлы, а два экземпляра такой арифметики расходятся
//! молча — сверить их друг о друга нечем.
//!
//! Часового пояса нет намеренно: у wasm32-wasip1 его не существует — ни
//! смещения, ни базы правил, — поэтому всё время в модулях всемирное.

/// Unix-секунды → (год, месяц 1..=12, день 1..=31).
///
/// Корректно и до 1970 года: отрицательные секунды делятся с округлением к
/// минус бесконечности (`div_euclid`), а не к нулю.
pub fn civil_from_unix(seconds: i64) -> (i64, u32, u32) {
    let days = seconds.div_euclid(86_400) + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    // Номер месяца в сдвинутом году: 0 — март, 11 — февраль.
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * shifted_month + 2) / 5 + 1) as u32;
    let month = if shifted_month < 10 { shifted_month + 3 } else { shifted_month - 9 } as u32;
    (year + i64::from(month <= 2), month, day)
}

/// (год, месяц 1..=12, день 1..=31) → дни от эпохи unix.
///
/// Обратная к [`civil_from_unix`] по дням: секунды в сутках добавляет
/// вызывающий — они у него свои (разобранные из строки, полночь, ...).
pub fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_dates() {
        assert_eq!(civil_from_unix(0), (1970, 1, 1));
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        // Секунда до эпохи — ещё 1969 год: деление к минус бесконечности.
        assert_eq!(civil_from_unix(-1), (1969, 12, 31));
        // Високосный день.
        assert_eq!(civil_from_unix(days_from_civil(2024, 2, 29) * 86_400), (2024, 2, 29));
    }

    /// Прямая и обратная сходятся на каждом дне широкого диапазона — включая
    /// границы веков (правило 100/400) и даты до эпохи.
    #[test]
    fn roundtrip() {
        for day in (-200_000..200_000).step_by(97) {
            let (year, month, dom) = civil_from_unix(day * 86_400);
            assert_eq!(days_from_civil(year, month as i64, dom as i64), day, "{}-{}-{}", year, month, dom);
        }
    }
}
