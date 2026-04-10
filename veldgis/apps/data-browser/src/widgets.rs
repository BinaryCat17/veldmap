//! widgets.rs — общие виджеты приложения

use veld_ui::{column, row, text, container, progress_bar, scrollable, Element, Length, Alignment};
use crate::{AppMessage, styles, task_manager::TaskManager};

/// Панель активных задач для отображения справа
pub fn task_panel(task_manager: &TaskManager) -> Element<AppMessage> {
    let active_tasks = task_manager.active();
    
    if active_tasks.is_empty() {
        return container(
            column![
                text("No active tasks").size(12.0).color(styles::COLOR_TEXT_DIM),
            ]
            .spacing(5.0)
        )
        .padding(10.0)
        .width(Length::Fixed(200.0))
        .into();
    }
    
    let task_items: Vec<Element<AppMessage>> = active_tasks
        .iter()
        .map(|task| {
            let title = task.kind.title();
            let progress_text = format!("{:.0}%", task.progress * 100.0);
            
            column![
                text(&title).size(12.0),
                row![
                    progress_bar(0.0..=1.0, task.progress)
                        .width(Length::Fixed(120.0)),
                    text(&progress_text).size(10.0).color(styles::COLOR_TEXT_DIM),
                ]
                .spacing(5.0)
                .align_items(Alignment::Center),
            ]
            .spacing(3.0)
            .into()
        })
        .collect();
    
    container(
        column![
            row![
                text("Tasks").size(14.0),
                text(format!("({})", active_tasks.len())).size(12.0).color(styles::COLOR_TEXT_DIM),
            ]
            .spacing(5.0)
            .align_items(Alignment::Center),
            scrollable(
                column(task_items).spacing(10.0)
            )
            .height(Length::Fill),
        ]
        .spacing(10.0)
        .width(Length::Fill)
    )
    .padding(10.0)
    .width(Length::Fixed(200.0))
    .height(Length::Fill)
    .into()
}

/// Компактная панель задач для размещения в layout
pub fn task_sidebar(task_manager: &TaskManager) -> Element<AppMessage> {
    container(task_panel(task_manager))
        .into()
}
