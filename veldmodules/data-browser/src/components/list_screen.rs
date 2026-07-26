//! components/list_screen.rs — каркас экрана со списком.
//!
//! К списку файлов отношения не имеет (жил в browser_list по историческим
//! причинам): это обёртка целой страницы, одинаковая на Browse/Search/
//! Downloaded — заголовочные строки сверху, скроллируемое тело снизу.
//! Отличается у экранов только содержимое, не геометрия.

use veld_ui_service_wrap::column;
use crate::proto::ui_service::{container, scrollable, Element, Length, Padding};

/// Колонку заголовка собирает сама (а не принимает готовый `Element`) —
/// каждый вложенный `column![]` без явного `.width(Fill)` схлопывается по
/// ширине до содержимого, а это тот же самый класс бага что и Length::Shrink
/// по умолчанию у самих виджетов (кнопок/текста/иконок) — там он осознанный
/// и его трогать не надо, но конкретно на экранном контейнере он неуместен.
/// Раз этот вызов — единственное место, где вложенная колонка вообще
/// создаётся, промахнуться мимо Fill здесь больше негде.
pub fn list_screen(header_rows: Vec<Element<()>>, body: Element<()>) -> Element<()> {
    // Правый паддинг у тела, а не у всей колонки — резервирует место под
    // полосу прокрутки: у `scrollable` в этом фреймворке нет своего API для
    // отступа скроллбара (в отличие от iced, где это Properties::margin), он
    // рисуется поверх правого края content-зоны — без запаса задевает
    // последнюю кнопку в каждой строке.
    let padded_body = container(body)
        .width(Length::Fill)
        .padding(Padding { top: 0.0, right: 14.0, bottom: 0.0, left: 0.0 });
    column![
        column(header_rows).spacing(15.0).width(Length::Fill),
        scrollable(padded_body).width(Length::Fill).height(Length::Fill)
    ]
    .spacing(15.0)
    .padding(Padding::new(10.0))
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}
