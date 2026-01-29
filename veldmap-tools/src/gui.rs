use iced::widget::{column, container, text, text_input, button, row, scrollable};
use iced::{Alignment, Element, Length, Task};
use crate::copernicus::CopernicusSource;
use std::sync::Arc;

pub struct VeldMapToolsGui {
    search_query: String,
    status_message: String,
    results: Vec<(String, String)>,
    selected_product: Option<String>,
    product_files: Vec<String>,
    source: Option<Arc<CopernicusSource>>,
}

impl Default for VeldMapToolsGui {
    fn default() -> Self {
        Self {
            search_query: String::new(),
            status_message: "Initializing...".to_string(),
            results: Vec::new(),
            selected_product: None,
            product_files: Vec::new(),
            source: None,
        }
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    SearchInputChanged(String),
    SearchPressed,
    SourceInitialized(Result<Arc<CopernicusSource>, String>),
    SearchResultsReceived(Result<Vec<(String, String)>, String>),
    ProductSelected(String, String), // Name, S3Path
    FilesReceived(Result<Vec<String>, String>),
    BackToSearch,
}

impl VeldMapToolsGui {
    pub fn new() -> (Self, Task<Message>) {
        (
            Self::default(),
            Task::perform(async {
                CopernicusSource::new().await
                    .map(Arc::new)
                    .map_err(|e| e.to_string())
            }, Message::SourceInitialized),
        )
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::SearchInputChanged(query) => {
                self.search_query = query;
                Task::none()
            }
            Message::SearchPressed => {
                if let Some(source) = &self.source {
                    self.status_message = format!("Searching for {}...", self.search_query);
                    let source = source.clone();
                    let query = self.search_query.clone();
                    self.selected_product = None;
                    Task::perform(async move {
                        source.search_grid_id(&query).await
                            .map_err(|e| e.to_string())
                    }, Message::SearchResultsReceived)
                } else {
                    Task::none()
                }
            }
            Message::SourceInitialized(result) => {
                match result {
                    Ok(source) => {
                        self.source = Some(source);
                        self.status_message = "Ready".to_string();
                    }
                    Err(e) => {
                        self.status_message = format!("Error initializing source: {}", e);
                    }
                }
                Task::none()
            }
            Message::SearchResultsReceived(result) => {
                match result {
                    Ok(results) => {
                        self.results = results;
                        self.status_message = format!("Found {} products", self.results.len());
                    }
                    Err(e) => {
                        self.status_message = format!("Search error: {}", e);
                    }
                }
                Task::none()
            }
            Message::ProductSelected(name, path) => {
                if let Some(source) = &self.source {
                    self.selected_product = Some(name);
                    self.status_message = format!("Loading files for {}...", path);
                    let source = source.clone();
                    Task::perform(async move {
                        source.list_product_files(&path).await
                            .map_err(|e| e.to_string())
                    }, Message::FilesReceived)
                } else {
                    Task::none()
                }
            }
            Message::FilesReceived(result) => {
                match result {
                    Ok(files) => {
                        self.product_files = files;
                        self.status_message = format!("Showing {} files", self.product_files.len());
                    }
                    Err(e) => {
                        self.status_message = format!("Error loading files: {}", e);
                    }
                }
                Task::none()
            }
            Message::BackToSearch => {
                self.selected_product = None;
                self.product_files.clear();
                self.status_message = format!("Found {} products", self.results.len());
                Task::none()
            }
        }
    }

    pub fn view(&self) -> Element<'_, Message> {
        let title = text("Copernicus Data Explorer").size(30);

        let content = if let Some(product_name) = &self.selected_product {
            // View files inside product
            column![
                button("← Back to Results").on_press(Message::BackToSearch),
                text(format!("Product: {}", product_name)).size(20),
                text(&self.status_message).size(14),
                scrollable(
                    column(
                        self.product_files.iter().map(|file| {
                            let is_tif = file.to_lowercase().ends_with(".tif");
                            row![
                                text(file).size(14),
                                if is_tif {
                                    text(" [GeoTIFF]").color([0.0, 0.5, 0.0])
                                } else {
                                    text("")
                                }
                            ].into()
                        }).collect::<Vec<Element<Message>>>()
                    )
                    .spacing(5)
                    .padding(10)
                )
                .height(Length::Fill),
            ]
            .spacing(20)
        } else {
            // Main search view
            column![
                row![
                    text_input("Enter gridId (e.g. N55_E037)...", &self.search_query)
                        .on_input(Message::SearchInputChanged)
                        .on_submit(Message::SearchPressed)
                        .padding(10),
                    button("Search").on_press(Message::SearchPressed).padding(10),
                ]
                .spacing(10)
                .align_y(Alignment::Center),
                text(&self.status_message).size(14),
                scrollable(
                    column(
                        self.results.iter().map(|(name, path)| {
                            button(
                                column![
                                    text(name).size(16),
                                    text(path).size(12).color([0.5, 0.5, 0.5]),
                                ].spacing(5)
                            )
                            .on_press(Message::ProductSelected(name.clone(), path.clone()))
                            .width(Length::Fill)
                            .padding(10)
                            .into()
                        }).collect::<Vec<Element<Message>>>()
                    )
                    .spacing(10)
                    .padding(5)
                )
                .height(Length::Fill),
            ]
            .spacing(20)
        };

        container(column![title, content].spacing(20).padding(20))
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .into()
    }
}