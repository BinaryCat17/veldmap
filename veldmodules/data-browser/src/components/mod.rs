//! Переиспользуемые куски UI. Один файл — один компонент; экраны целиком
//! лежат в `view/`, а не здесь.
//!
//! `row` — не рендер, а модель: в каком состоянии файл и что с ним можно
//! сделать. Лежит рядом с `file_list`, потому что тот матчится на `RowStatus`
//! исчерпывающе — это одна вещь, разъезжаться им нельзя.

pub mod row;
pub mod file_list;
pub mod list_screen;

pub use row::{Row, RowStatus, downloaded_rows};
pub use file_list::{render_list, items_or_message, ItemActions};
pub use list_screen::list_screen;
