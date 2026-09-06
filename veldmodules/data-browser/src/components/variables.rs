//! components/variables.rs — список величин файла многих величин: под кадром
//! канвы и в строке слоя. Один на оба места: пункты, галочка у показанной и
//! обрезка длинного списка устроены одинаково, а откуда величины и что шлёт
//! пункт — решает вызывающий.

use crate::module::components::{format, menu};
use crate::module::Msg;

/// Сколько величин перечисляет раскрытый список; остальные — числом. У
/// гранулы Sentinel-5P годных три десятка, и панель длиннее окна не раскрыть.
/// Семнадцать строк списка — около 520 pt по метрикам `menu::line`: в окно в
/// 768 pt встают; в панель деления в половину окна — нет, там всплывашка
/// упирается в край и накрывает кнопку, оставаясь закрываемой щелчком мимо.
pub const LISTED: usize = 16;

/// Величина, как её называют: путь в файле, слова файла и единицы. Одно на
/// две копии сообщения (`image_view::Variable`, `globe::Variable`) — список
/// собирается из любой.
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

/// Пункты списка: все величины в порядке тайлера, показанная (`shown`) — с
/// галочкой; пункт шлёт `pick` со своим путём. Длинный список обрезается
/// числом ([`LISTED`]), но показанной место гарантировано: обрезанная, она
/// уходила бы в «и ещё N», и список из одних не показанных читался бы как
/// «показана ни одна из них». Пустая `shown` — галочки нет ни у кого: на
/// экране нет ни одной (названной отказано).
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
        items.push(menu::Item::note(format!("и ещё {}", variables.len() - LISTED)));
    }
    items
}

#[cfg(test)]
mod tests {
    use super::{items, Named, LISTED};
    use crate::module::Msg;

    fn named(path: &'static str, units: &'static str) -> Named<'static> {
        Named { path, said: "", units }
    }

    /// Список называет все величины в порядке тайлера, отмечает показанную и
    /// обрезает длинный хвост числом; показанная за обрезкой встаёт последней
    /// из перечисленных.
    #[test]
    fn the_list_names_every_variable_and_marks_the_shown_one() {
        let pick = |path: String| Msg::OverlayVariable("k".into(), path);
        let few = [named("/PRODUCT/a", "K"), named("/PRODUCT/b", "")];
        let listed = items(&few, "/PRODUCT/b", pick);
        assert_eq!(listed.iter().map(|item| item.named()).collect::<Vec<_>>(), ["a, K", "b"]);
        assert_eq!(listed.iter().map(|item| item.marked()).collect::<Vec<_>>(), [false, true]);
        assert!(items(&few, "", pick).iter().all(|item| !item.marked()), "без показанной галочка лишняя");

        let paths: Vec<String> = (0..LISTED + 5).map(|at| format!("/v{at}")).collect();
        let many: Vec<Named<'_>> = paths.iter().map(|path| Named { path, said: "", units: "" }).collect();
        let listed = items(&many, "/v0", pick);
        assert_eq!(listed.len(), LISTED + 1);
        assert_eq!(listed.last().map(|item| item.named()), Some("и ещё 5"));
        assert!(listed[0].marked() && !listed[1].marked());

        let listed = items(&many, "/v20", pick);
        assert_eq!(listed.len(), LISTED + 1);
        assert_eq!(listed[LISTED - 1].named(), "v20");
        assert!(listed[LISTED - 1].marked() && !listed[0].marked());
        assert_eq!(listed.last().map(|item| item.named()), Some("и ещё 5"));
    }
}
