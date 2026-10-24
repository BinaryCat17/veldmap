//! components/variables.rs — the list of variables of a multi-variable file:
//! under the canvas frame and in the layer row. One list for both places:
//! items, the tick by the shown one, and truncation of a long list are built
//! the same way; where the variables come from and what an item sends is up to
//! the caller.

use crate::module::components::{format, menu};
use crate::module::Msg;

/// How many variables the expanded list enumerates; the rest are a number. A
/// Sentinel-5P granule has three dozen usable ones, and a panel taller than
/// the window cannot be expanded. Seventeen list rows are about 520 pt by
/// `menu::line` metrics: they fit a 768 pt window; they do not fit a half-
/// window split panel — there the pop-up hits the edge and covers the button,
/// while staying closable by a click outside.
pub const LISTED: usize = 16;

/// A variable as it is named: path in the file, the file's wording, and units.
/// One struct for both message copies (`image_view::Variable`,
/// `globe::Variable`) — the list is built from either.
pub struct Named<'a> {
    pub path: &'a str,
    pub said: &'a str,
    pub units: &'a str,
}

impl<'a> From<&'a crate::proto::image_view::Variable> for Named<'a> {
    fn from(variable: &'a crate::proto::image_view::Variable) -> Self {
        Named { path: &variable.path, said: &variable.said, units: &variable.units }
    }
}

impl<'a> From<&'a crate::proto::globe::Variable> for Named<'a> {
    fn from(variable: &'a crate::proto::globe::Variable) -> Self {
        Named { path: &variable.path, said: &variable.said, units: &variable.units }
    }
}

/// List items: all variables in tiler order, the shown one (`shown`) carries
/// the tick; an item sends `pick` with its path. A long list is truncated with
/// a number ([`LISTED`]), but the shown one is guaranteed a spot: truncated,
/// it would go into "and N more", and a list of only not-shown ones would read
/// as "none of them is shown". Empty `shown` — nobody carries a tick: there is
/// none on screen (the named one was refused).
pub fn items(variables: &[Named<'_>], shown: &str, pick: impl Fn(String) -> Msg) -> Vec<menu::Item> {
    let item = |variable: &Named<'_>| {
        menu::Item::new(format::variable(variable.path, variable.said, variable.units), pick(variable.path.to_string()))
            .selected(!shown.is_empty() && variable.path == shown)
    };
    let mut items: Vec<menu::Item> = variables.iter().take(LISTED).map(item).collect();
    if let Some(at) = variables.iter().position(|variable| variable.path == shown).filter(|&at| at >= LISTED) {
        items.truncate(LISTED - 1);
        items.push(item(&variables[at]));
    }
    if variables.len() > LISTED {
        items.push(menu::Item::note(format!("and {} more", variables.len() - LISTED)));
    }
    items
}
