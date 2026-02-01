use iced_widget::{
    button, column, container, row, text, progress_bar, scrollable, Space
};
use iced_core::image::Handle;
use iced_core::{Alignment, Element, Length, Color, Theme};
use iced_tiny_skia::Renderer;
use iced_runtime::Task;
use veldmap_rust_rpc::dataprovider::{SearchRequest, SearchResponse, ListPathRequest, ListPathResponse, DownloadRequest, DownloadResponse, DataProduct, SearchFilter};
use prost::Message as ProstMessage;
use crate::search;
use crate::downloaded;
use crate::preview;
use crate::browse;
use crate::common::{self, BrowserItem, is_previewable, icon_text};

impl veldmap_iced_wasm_runtime::Application<Message> for VeldMapToolsGui {
    fn update(&mut self, message: Message) {
        let _ = self.update(message);
    }

    fn view(&self) -> Element<'_, Message, Theme, Renderer> {
        self.view()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ViewMode {
    Search,
    Browse,
    Downloaded,
}

#[derive(Debug, Clone)]
pub enum Message {
    SwitchMode(ViewMode),
    SearchInputChanged(String),
    SearchFilterTypeChanged(search::SearchFilterType),
    SearchPressed,
    ClearError,
    ProductSelected(DataProduct),
    BackToList,
    BrowsePath(String),
    BrowseUp,
    LocalSearchChanged(String),
    LocalFilterChanged(downloaded::FileFilter),
    DownloadFile(String),
    DeleteLocalFile(String),
    ViewFile(String),
    ClosePreview,
}

pub struct VeldMapToolsGui {
    pub view_mode: ViewMode,
    pub status_message: String,
    pub error_message: Option<String>,
    pub search_state: search::SearchState,
    pub search_results: Vec<DataProduct>,
    pub download_progress: Option<f32>,
    pub current_image: Option<Handle>,
    pub downloaded_state: downloaded::DownloadedState,
    pub token_stack: Vec<String>,
    pub current_token: Option<String>,
    pub next_token: Option<String>,
    pub current_browse_path: String,
    pub selected_product: Option<String>,
    pub product_files: Vec<BrowserItem>,
    pub browse_items: Vec<BrowserItem>,
    pub local_files: Vec<BrowserItem>,
}

impl VeldMapToolsGui {
    pub fn new() -> (Self, Task<Message>) {
        (
            Self {
                view_mode: ViewMode::Search,
                status_message: "VeldMap Data Browser Ready".to_string(),
                error_message: None,
                search_state: search::SearchState::default(),
                search_results: Vec::new(),
                download_progress: None,
                current_image: None,
                downloaded_state: downloaded::DownloadedState::default(),
                token_stack: Vec::new(),
                current_token: None,
                next_token: None,
                current_browse_path: String::new(),
                selected_product: None,
                product_files: Vec::new(),
                browse_items: Vec::new(),
                local_files: Vec::new(),
            },
            Task::none(),
        )
    }

    fn perform_browse(&mut self, path: String) {
        self.status_message = format!("Listing /{}...", path);
        let req = ListPathRequest { path: path.clone(), token: String::new() };
        match veldmap_rust_rpc::host::call_service("data-provider", "list_path", req.encode_to_vec()) {
            Ok(res_bytes) => {
                if let Ok(response) = ListPathResponse::decode(&res_bytes[..]) {
                    if !response.error.is_empty() {
                        self.error_message = Some(format!("Browse API Error: {}", response.error));
                        self.status_message = "Browse failed".to_string();
                    } else {
                        self.browse_items = response.items.into_iter().map(|s3_key| {
                            let is_folder = s3_key.ends_with('/');
                            let name = s3_key.trim_end_matches('/').split('/').last().unwrap_or(&s3_key).to_string();
                            BrowserItem { s3_key, name, is_folder, exists_locally: false }
                        }).collect();
                        self.current_browse_path = path;
                        self.next_token = if response.next_token.is_empty() { None } else { Some(response.next_token) };
                        self.status_message = format!("Loaded {} items", self.browse_items.len());
                    }
                } else {
                    self.error_message = Some("Failed to decode ListPathResponse".into());
                }
            }
            Err(e) => { 
                self.error_message = Some(format!("Browse RPC Error: {}", e)); 
                self.status_message = "Browse failed".to_string();
            }
        }
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::SwitchMode(mode) => {
                self.view_mode = mode;
                self.current_image = None;
                self.download_progress = None;
                self.selected_product = None;
                if self.view_mode == ViewMode::Browse && self.browse_items.is_empty() {
                    self.perform_browse(String::new());
                }
                Task::none()
            }
            Message::SearchInputChanged(q) => { self.search_state.query = q; Task::none() }
            Message::SearchFilterTypeChanged(ft) => { self.search_state.filter_type = ft; Task::none() }
            Message::SearchPressed => {
                self.status_message = "Searching CDSE...".to_string();
                let mut filters = Vec::new();
                match self.search_state.filter_type {
                    search::SearchFilterType::GridId => filters.push(SearchFilter { name: "gridId".into(), value: self.search_state.query.clone() }),
                    search::SearchFilterType::Collection => filters.push(SearchFilter { name: "Collection".into(), value: self.search_state.query.clone() }),
                    _ => {}
                }
                let q = if self.search_state.filter_type == search::SearchFilterType::General { self.search_state.query.clone() } else { String::new() };
                
                let req = SearchRequest { query: q, filters };
                match veldmap_rust_rpc::host::call_service("data-provider", "search", req.encode_to_vec()) {
                    Ok(res_bytes) => {
                        if let Ok(response) = SearchResponse::decode(&res_bytes[..]) {
                            if !response.error.is_empty() {
                                self.error_message = Some(format!("Search API Error: {}", response.error));
                            } else {
                                self.search_results = response.products;
                                self.status_message = format!("Found {} results", self.search_results.len());
                            }
                        }
                    }
                    Err(e) => { self.error_message = Some(format!("Search Error: {}", e)); }
                }
                Task::none()
            }
            Message::ProductSelected(prod) => {
                self.status_message = format!("Loading files for {}...", prod.name);
                let req = ListPathRequest { path: prod.path.clone(), token: String::new() };
                match veldmap_rust_rpc::host::call_service("data-provider", "list_path", req.encode_to_vec()) {
                    Ok(res_bytes) => {
                        if let Ok(response) = ListPathResponse::decode(&res_bytes[..]) {
                            self.product_files = response.items.into_iter().map(|s3_key| {
                                let name = s3_key.split('/').last().unwrap_or(&s3_key).to_string();
                                let is_folder = s3_key.ends_with('/');
                                BrowserItem { s3_key, name, is_folder, exists_locally: false }
                            }).collect();
                            self.selected_product = Some(prod.name.clone());
                            self.status_message = format!("Loaded {} items", self.product_files.len());
                        }
                    }
                    Err(e) => { self.error_message = Some(format!("List Error: {}", e)); }
                }
                Task::none()
            }
            Message::BackToList => { self.selected_product = None; Task::none() }
            Message::BrowsePath(path) => {
                self.perform_browse(path);
                Task::none()
            }
            Message::BrowseUp => {
                let current = self.current_browse_path.trim_end_matches('/');
                if let Some(idx) = current.rfind('/') {
                    let parent = current[..=idx].to_string();
                    self.perform_browse(parent);
                } else {
                    self.perform_browse(String::new());
                }
                Task::none()
            }
            Message::DownloadFile(s3_key) => {
                let filename = s3_key.split('/').last().unwrap_or("file");
                let dest = format!("data/dem/source/{}", filename);
                let req = DownloadRequest { identifier: s3_key, destination: dest };
                match veldmap_rust_rpc::host::call_service("data-provider", "download", req.encode_to_vec()) {
                    Ok(res_bytes) => {
                        if let Ok(response) = DownloadResponse::decode(&res_bytes[..]) {
                            if response.success {
                                self.status_message = "Download started".into();
                            } else {
                                self.error_message = Some(format!("Download failed: {}", response.error));
                            }
                        }
                    }
                    Err(e) => { self.error_message = Some(format!("Download Error: {}", e)); }
                }
                Task::none()
            }
            Message::DeleteLocalFile(_name) => {
                Task::none()
            }
            Message::ViewFile(s3_key) => {
                let _ = veldmap_rust_rpc::host::call_service("system", "log", format!("WASM: Viewing file {}", s3_key).as_bytes().to_vec());
                Task::none()
            }
            Message::ClearError => { self.error_message = None; Task::none() }
            Message::LocalSearchChanged(q) => { self.downloaded_state.search_query = q; Task::none() }
            Message::LocalFilterChanged(f) => { self.downloaded_state.filter = f; Task::none() }
            Message::ClosePreview => { self.current_image = None; Task::none() }
        }
    }

    pub fn view(&self) -> Element<'_, Message, Theme, Renderer> {
        let title_bar = column![
            text("VeldMap Tools").font(crate::common::APP_FONT).size(32).color(common::COLOR_TEXT),
            row![
                button(text("Search").font(crate::common::APP_FONT))
                    .on_press(Message::SwitchMode(ViewMode::Search))
                    .style(if self.view_mode == ViewMode::Search { common::primary_button_style } else { common::ghost_button_style })
                    .padding(12),
                button(text("Browse").font(crate::common::APP_FONT))
                    .on_press(Message::SwitchMode(ViewMode::Browse))
                    .style(if self.view_mode == ViewMode::Browse { common::primary_button_style } else { common::ghost_button_style })
                    .padding(12),
                button(text("Downloaded").font(crate::common::APP_FONT))
                    .on_press(Message::SwitchMode(ViewMode::Downloaded))
                    .style(if self.view_mode == ViewMode::Downloaded { common::primary_button_style } else { common::ghost_button_style })
                    .padding(12),
            ].spacing(15),
        ].spacing(20);

        let error_view: Element<Message, Theme, Renderer> = if let Some(err) = &self.error_message {
            container(row![
                button(text("X").font(crate::common::APP_FONT)).on_press(Message::ClearError).padding(5),
                text(err).font(crate::common::APP_FONT).size(14).color(Color::from_rgb(1.0, 0.4, 0.4)).width(Length::Fill),
            ].spacing(15).align_y(Alignment::Center))
            .padding(12)
            .style(|_| container::Style::default().background(Color::from_rgb(0.25, 0.1, 0.1)))
            .into()
        } else { column![].into() };

        let status_view = text(&self.status_message).font(crate::common::APP_FONT).size(14).color(common::COLOR_TEXT_DIM);

        let progress_view: Element<Message, Theme, Renderer> = if let Some(p) = self.download_progress {
            column![text(format!("Processing... {:.0}%", p * 100.0)).font(crate::common::APP_FONT).size(14), progress_bar(0.0..=1.0, p)].spacing(8).into()
        } else { column![].into() };

        let main_content: Element<Message, Theme, Renderer> = if let Some(handle) = &self.current_image {
            preview::view(handle)
        } else if let Some(product_name) = &self.selected_product {
            column![
                button(text("← Back").font(crate::common::APP_FONT)).on_press(Message::BackToList).padding(8).style(common::ghost_button_style),
                text(format!("Product: {}", product_name)).font(crate::common::APP_FONT).size(20),
                scrollable(column(self.product_files.iter().map(|item| {
                    let previewable = is_previewable(&item.name);
                    let label_color = if item.exists_locally { Color::from_rgb(0.3, 0.8, 0.3) } else { common::COLOR_TEXT };
                    
                    let controls: Element<Message, Theme, Renderer> = if previewable {
                         row![
                            button(text("Download").font(crate::common::APP_FONT)).on_press(Message::DownloadFile(item.s3_key.clone())).padding(8).style(common::primary_button_style),
                            button(text("View").font(crate::common::APP_FONT)).on_press(Message::ViewFile(item.s3_key.clone())).padding(8).style(common::primary_button_style)
                         ].spacing(8).into()
                    } else {
                        button(text("Download").font(crate::common::APP_FONT)).on_press(Message::DownloadFile(item.s3_key.clone())).padding(8).style(common::primary_button_style).into()
                    };

                    container(row![
                        icon_text(if item.exists_locally { "✅" } else { "📄" }, &item.name, label_color),
                        Space::new().width(Length::Fill),
                        controls
                    ].spacing(25).align_y(Alignment::Center))
                    .padding(12)
                    .style(common::surface_container_style)
                    .into()
                }).collect::<Vec<Element<Message, Theme, Renderer>>>()).spacing(10)).height(Length::Fill)
            ].spacing(20).into()
        } else {
            match self.view_mode {
                ViewMode::Search => search::view(&self.search_state, &self.search_results),
                ViewMode::Browse => browse::view(&self.current_browse_path, &self.browse_items, &self.status_message, !self.token_stack.is_empty(), self.next_token.is_some()),
                ViewMode::Downloaded => downloaded::view(&self.downloaded_state, &self.local_files),
            }
        };

        container(column![title_bar, status_view, error_view, progress_view, main_content].spacing(20).padding(25))
            .width(Length::Fill).height(Length::Fill)
            .style(common::main_container_style)
            .into()
    }
}
