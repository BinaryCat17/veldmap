//! Reusable UI pieces. One file per component; whole screens live in `view/`,
//! not here.
//!
//! `row` is not rendering but a model: what state a record is in and what is
//! known about it. It sits next to `table` because that one matches on
//! `RowStatus` exhaustively — it is a single thing and they must not diverge.
//! `arrange` sits between them: it decides what to show and in what order, and
//! knows no widget at all.

pub mod arrange;
pub mod controls;
pub mod format;
pub mod list_screen;
pub mod menu;
pub mod row;
pub mod rows;
pub mod table;
pub mod variables;

pub use list_screen::Screen;
pub use row::{
    folder_of, folder_path, last_segment, preview_of, OnGlobe, OnOutline, Row, RowKind,
    RowStatus,
};
