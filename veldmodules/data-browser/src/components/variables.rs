use crate::module::components::{format, menu};
use crate::module::Msg;

pub const LISTED: usize = 16;

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
