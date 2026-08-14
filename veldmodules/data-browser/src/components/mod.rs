//! Переиспользуемые куски UI. Один файл — один компонент; экраны целиком
//! лежат в `view/`, а не здесь.
//!
//! `row` — не рендер, а модель: в каком состоянии запись и что о ней известно.
//! Лежит рядом с `table`, потому что тот матчится на `RowStatus` исчерпывающе —
//! это одна вещь, разъезжаться им нельзя. `arrange` между ними: он решает, что
//! и в каком порядке показывать, и не знает ни одного виджета.

pub mod arrange;
pub mod controls;
pub mod format;
pub mod list_screen;
pub mod menu;
pub mod row;
pub mod table;

pub use list_screen::Screen;
pub use row::{downloaded_rows, folder_of, Row, RowKind, RowStatus};
