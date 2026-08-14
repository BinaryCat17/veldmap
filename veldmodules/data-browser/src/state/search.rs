use crate::proto::data_provider::DataProduct;

use super::listing::Choice;

/// Миссия, по которой сужается запрос.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum Mission {
    #[default]
    Any,
    Sentinel1,
    Sentinel2,
    Sentinel3,
}

impl Mission {
    /// Имя коллекции, как его знает каталог; пусто — не сужать. Имена
    /// буквальные и уходят в запрос без перевода.
    pub fn collection(self) -> &'static str {
        match self {
            Mission::Any => "",
            Mission::Sentinel1 => "SENTINEL-1",
            Mission::Sentinel2 => "SENTINEL-2",
            Mission::Sentinel3 => "SENTINEL-3",
        }
    }

    /// Есть ли у снимков этой миссии облачность.
    ///
    /// У радара её нет вовсе, и условие по ней отсекает не часть выдачи, а всю:
    /// каталог отбирает снимки, у которых атрибут есть и меньше порога, а у
    /// Sentinel-1 его не бывает (проверено — выдача пустая). «Все миссии» тоже
    /// не в счёт: радар входит и в них.
    pub fn clouded(self) -> bool {
        matches!(self, Mission::Sentinel2 | Mission::Sentinel3)
    }
}

impl Choice for Mission {
    const ALL: &'static [Self] =
        &[Mission::Any, Mission::Sentinel1, Mission::Sentinel2, Mission::Sentinel3];

    fn key(self) -> &'static str {
        match self {
            Mission::Any => "any",
            Mission::Sentinel1 => "sentinel-1",
            Mission::Sentinel2 => "sentinel-2",
            Mission::Sentinel3 => "sentinel-3",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Mission::Any => "Все",
            Mission::Sentinel1 => "Sentinel-1",
            Mission::Sentinel2 => "Sentinel-2",
            Mission::Sentinel3 => "Sentinel-3",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Mission::Any => "все",
            Mission::Sentinel1 => "Sentinel-1",
            Mission::Sentinel2 => "Sentinel-2",
            Mission::Sentinel3 => "Sentinel-3",
        }
    }
}

/// Окно съёмки: насколько глубоко в прошлое смотреть.
///
/// Само по себе окно выдачу почти не меняет: каталог отдаёт самое свежее
/// первым, и пятьдесят самых свежих сняты минуты назад — они попадают в любое
/// окно. Смысл у него появляется вместе с сужением по месту: у одной плитки
/// пролёты за неделю и за год — это разные списки.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum Period {
    #[default]
    Any,
    Week,
    Month,
    Year,
    /// Оба края названы датами. Отдельным вариантом, а не парой полей рядом с
    /// пресетом: «за неделю» и «с 1 по 5 августа» — это два разных способа
    /// сказать одно и то же, и держать их одновременно значит завести вопрос
    /// «какой из них главный».
    Custom,
}

impl Period {
    /// Начало окна в unix-секундах; 0 — не ограничивать.
    pub fn since(self, now: i64) -> i64 {
        let days = match self {
            // Свой интервал считается не отсюда, а из набранных дат
            // (см. `SearchState::window`).
            Period::Any | Period::Custom => return 0,
            Period::Week => 7,
            Period::Month => 30,
            Period::Year => 365,
        };
        now - days * 24 * 60 * 60
    }
}

impl Choice for Period {
    const ALL: &'static [Self] =
        &[Period::Any, Period::Week, Period::Month, Period::Year, Period::Custom];

    fn key(self) -> &'static str {
        match self {
            Period::Any => "any",
            Period::Week => "week",
            Period::Month => "month",
            Period::Year => "year",
            Period::Custom => "custom",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Period::Any => "За всё время",
            Period::Week => "За неделю",
            Period::Month => "За месяц",
            Period::Year => "За год",
            Period::Custom => "Свой интервал",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Period::Any => "всё время",
            Period::Week => "неделя",
            Period::Month => "месяц",
            Period::Year => "год",
            Period::Custom => "свой интервал",
        }
    }
}

/// Потолок облачности. Спрашивается только у оптических миссий — см.
/// [`Mission::clouded`].
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum Cloud {
    #[default]
    Any,
    Upto10,
    Upto30,
    Upto60,
}

impl Cloud {
    /// Порог в процентах; `None` — не ограничивать.
    pub fn max(self) -> Option<f64> {
        match self {
            Cloud::Any => None,
            Cloud::Upto10 => Some(10.0),
            Cloud::Upto30 => Some(30.0),
            Cloud::Upto60 => Some(60.0),
        }
    }
}

impl Choice for Cloud {
    const ALL: &'static [Self] = &[Cloud::Any, Cloud::Upto10, Cloud::Upto30, Cloud::Upto60];

    fn key(self) -> &'static str {
        match self {
            Cloud::Any => "any",
            Cloud::Upto10 => "10",
            Cloud::Upto30 => "30",
            Cloud::Upto60 => "60",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Cloud::Any => "Любая",
            Cloud::Upto10 => "До 10%",
            Cloud::Upto30 => "До 30%",
            Cloud::Upto60 => "До 60%",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Cloud::Any => "любая",
            Cloud::Upto10 => "до 10%",
            Cloud::Upto30 => "до 30%",
            Cloud::Upto60 => "до 60%",
        }
    }
}

#[derive(Default)]
pub struct SearchState {
    /// Запрос к провайдеру — не то же, что фильтр по имени в `listing`: тот
    /// отбирает уже найденное, этот решает, что искать.
    pub query: String,
    pub mission: Mission,
    pub period: Period,
    /// Края своего интервала — как их набрали, а не как разобрали. Текстом,
    /// потому что набранное наполовину («2026-08-») числом ещё не является, а
    /// стирать из поля недобранное значило бы отнимать у него ввод.
    pub from: String,
    pub to: String,
    /// Потолок облачности. Держится и когда его не спрашивают (миссия сменилась
    /// на радарную), но в запрос тогда не уходит: см. `handlers::search::run`.
    pub cloud: Cloud,
    pub results: Vec<DataProduct>,
    /// Последняя ошибка search от data-provider (пусто, если запрос успешен)
    pub error: Option<String>,
    /// Ожидание ответа на data-provider/on_search: актуален только последний —
    /// результат по прошлому запросу под нынешним запросом ввёл бы в заблуждение.
    pub request: veldsdk::Latest,
    /// Как показывать найденное — отбор, порядок, страница.
    pub listing: super::listing::ListingState,
}

impl SearchState {
    /// Окно съёмки для запроса: пара unix-секунд, 0 — край не задан.
    ///
    /// Одним местом на оба края: у пресета верхнего края нет вовсе («за
    /// неделю» значит «по сегодня»), у своего интервала оба выводятся из
    /// набранного, — и решать, какой из двух способов сейчас в силе, должен
    /// кто-то один.
    ///
    /// Верхний край — конец названного дня, а не его полночь: «по 13 августа»
    /// без этого не включало бы сам тринадцатое, и снимок, ради которого дату
    /// и набрали, в выдачу бы не попал.
    pub fn window(&self, now: i64) -> (i64, i64) {
        use crate::module::components::format::parse_date;
        match self.period {
            Period::Custom => (
                parse_date(&self.from).unwrap_or(0),
                parse_date(&self.to).map(|day| day + 24 * 60 * 60 - 1).unwrap_or(0),
            ),
            period => (period.since(now), 0),
        }
    }
}
