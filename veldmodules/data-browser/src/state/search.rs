use crate::proto::data_provider::DataProduct;

use super::listing::choice;

choice! {
    Mission {
        Any       = "any",        "All", "all";
        Sentinel1 = "sentinel-1", "Sentinel-1";
        Sentinel2 = "sentinel-2", "Sentinel-2";
        Sentinel3 = "sentinel-3", "Sentinel-3";
    }
}

impl Mission {
    pub fn collection(self) -> &'static str {
        match self {
            Mission::Any => "",
            Mission::Sentinel1 => "SENTINEL-1",
            Mission::Sentinel2 => "SENTINEL-2",
            Mission::Sentinel3 => "SENTINEL-3",
        }
    }

    pub fn clouded(self) -> bool {
        matches!(self, Mission::Sentinel2 | Mission::Sentinel3)
    }
}

choice! {
    Period {
        Any   = "any",   "All time",    "all time";
        Week  = "week",  "Past week",   "past week";
        Month = "month", "Past month",  "past month";
        Year  = "year",  "Past year",   "past year";
        Custom = "custom", "Custom range", "custom range";
    }
}

impl Period {
    pub fn since(self, now: i64) -> i64 {
        let days = match self {
            Period::Any | Period::Custom => return 0,
            Period::Week => 7,
            Period::Month => 30,
            Period::Year => 365,
        };
        now - days * 24 * 60 * 60
    }
}

choice! {
    Cloud {
        Any    = "any", "Any",       "any";
        Upto10 = "10",  "Up to 10%", "up to 10%";
        Upto30 = "30",  "Up to 30%", "up to 30%";
        Upto60 = "60",  "Up to 60%", "up to 60%";
    }
}

impl Cloud {
    pub fn max(self) -> Option<f64> {
        match self {
            Cloud::Any => None,
            Cloud::Upto10 => Some(10.0),
            Cloud::Upto30 => Some(30.0),
            Cloud::Upto60 => Some(60.0),
        }
    }
}

#[derive(Default)]
pub struct SearchState {
    pub query: String,
    pub mission: Mission,
    pub period: Period,
    pub from: String,
    pub to: String,
    pub cloud: Cloud,
    pub results: Vec<DataProduct>,
    pub children: super::browse::Children,
    pub error: Option<String>,
    pub request: veldsdk::Latest,
    pub listing: super::listing::ListingState,
}

impl SearchState {
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
