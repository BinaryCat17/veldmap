use crate::proto::data_provider::DataProduct;

use super::listing::choice;

choice! {
    /// The mission the query is narrowed by.
    Mission {
        Any       = "any",        "All", "all";
        Sentinel1 = "sentinel-1", "Sentinel-1";
        Sentinel2 = "sentinel-2", "Sentinel-2";
        Sentinel3 = "sentinel-3", "Sentinel-3";
    }
}

impl Mission {
    /// The collection name as the catalog knows it; empty — do not narrow. Names
    /// are literal and go into the query untranslated.
    pub fn collection(self) -> &'static str {
        match self {
            Mission::Any => "",
            Mission::Sentinel1 => "SENTINEL-1",
            Mission::Sentinel2 => "SENTINEL-2",
            Mission::Sentinel3 => "SENTINEL-3",
        }
    }

    /// Whether snapshots of this mission carry cloud cover.
    ///
    /// Radar has none at all, and a condition on it cuts not part of the results
    /// but all of them: the catalog selects snapshots that carry the attribute and
    /// are below the threshold, while Sentinel-1 never has it (verified — the
    /// results are empty). "All missions" does not count either: radar is among
    /// them too.
    pub fn clouded(self) -> bool {
        matches!(self, Mission::Sentinel2 | Mission::Sentinel3)
    }
}

choice! {
    /// The acquisition window: how far back to look.
    ///
    /// By itself the window barely changes the results: the catalog gives the most
    /// recent first, and the fifty most recent were shot minutes ago — they fall
    /// into any window. Its meaning appears together with the spatial narrowing:
    /// over one tile, passes of the past week and of the past year are different
    /// lists.
    Period {
        Any   = "any",   "All time",    "all time";
        Week  = "week",  "Past week",   "past week";
        Month = "month", "Past month",  "past month";
        Year  = "year",  "Past year",   "past year";
        /// Both edges are given as dates. A separate variant, not a pair of fields
        /// next to the presets: "past week" and "August 1 to 5" are two different
        /// ways to say one and the same thing, and keeping both at once would
        /// raise the question "which one is primary".
        Custom = "custom", "Custom range", "custom range";
    }
}

impl Period {
    /// The start of the window in unix seconds; 0 — do not limit.
    pub fn since(self, now: i64) -> i64 {
        let days = match self {
            // The custom interval is computed not here but from the typed dates
            // (see `SearchState::window`).
            Period::Any | Period::Custom => return 0,
            Period::Week => 7,
            Period::Month => 30,
            Period::Year => 365,
        };
        now - days * 24 * 60 * 60
    }
}

choice! {
    /// The cloud cover ceiling. Asked only of optical missions — see
    /// [`Mission::clouded`].
    Cloud {
        Any    = "any", "Any",       "any";
        Upto10 = "10",  "Up to 10%", "up to 10%";
        Upto30 = "30",  "Up to 30%", "up to 30%";
        Upto60 = "60",  "Up to 60%", "up to 60%";
    }
}

impl Cloud {
    /// The threshold in percent; `None` — do not limit.
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
    /// The query to the provider — not the same as the name filter in `listing`:
    /// that one filters what is already found, this one decides what to search.
    pub query: String,
    pub mission: Mission,
    pub period: Period,
    /// The edges of the custom interval — as typed, not as parsed. Text, because a
    /// half-typed value ("2026-08-") is not a number yet, and erasing the
    /// unfinished part from the field would mean taking input away from it.
    pub from: String,
    pub to: String,
    /// The cloud cover ceiling. Kept even when it is not asked (the mission
    /// switched to radar), but then it does not go into the query: see
    /// `handlers::search::run`.
    pub cloud: Cloud,
    pub results: Vec<DataProduct>,
    /// Contents of expanded snapshots: a product that lies as a folder expands
    /// into its files right in the results — the same listing as in the catalog.
    pub children: super::browse::Children,
    /// The last search error from data-provider (empty on a successful request)
    pub error: Option<String>,
    /// Waiting for an answer to data-provider/on_search: only the last one is
    /// current — the result of a past request under the present query would be
    /// misleading.
    pub request: veldsdk::Latest,
    /// How to show what is found — filtering, order, page.
    pub listing: super::listing::ListingState,
}

impl SearchState {
    /// The acquisition window for the query: a pair of unix seconds, 0 — the edge
    /// is not set.
    ///
    /// One place for both edges: a preset has no upper edge at all ("past week"
    /// means "through today"), while the custom interval derives both from what
    /// was typed — and which of the two ways is in force must be decided by one
    /// place.
    ///
    /// The upper edge is the end of the named day, not its midnight: "through
    /// August 13" would otherwise not include the thirteenth itself, and the
    /// snapshot the date was typed for would not make it into the results.
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
