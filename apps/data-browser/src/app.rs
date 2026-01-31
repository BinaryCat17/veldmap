use iced_widget::{
    button, column, container, row, text, progress_bar,
};
use iced_core::image::Handle;
use iced_core::{Alignment, Element, Length, Color, Theme};
use iced_tiny_skia::Renderer;
use iced_runtime::Task;
use veldmap_rust_rpc::services::{SearchRequest, SearchResponse};
use veldmap_rust_rpc::common::{DataProduct, SearchFilter};
use prost::Message as ProstMessage;
use crate::search;
use crate::downloaded;
use crate::preview;
use crate::browse;

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
    SearchResultsReceived(Result<Vec<DataProduct>, String>),
    ClearError,
    ProductSelected(DataProduct),
    BrowsePath(String),
    BrowseUp,
    ViewFile(String),
    DeleteLocalFile(String),
    PrevPage,
    NextPage,
    LocalSearchChanged(String),
    LocalFilterChanged(downloaded::FileFilter),
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
    pub product_files: Vec<crate::common::BrowserItem>,
    pub browse_items: Vec<crate::common::BrowserItem>,
    pub local_files: Vec<crate::common::BrowserItem>,
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
                product_files: Vec::new(),
                browse_items: Vec::new(),
                local_files: Vec::new(),
            },
            Task::none(),
        )
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::SwitchMode(mode) => {
                self.view_mode = mode;
                self.current_image = None;
                self.download_progress = None;
                Task::none()
            }
            Message::SearchInputChanged(q) => { self.search_state.query = q; Task::none() }
            Message::SearchFilterTypeChanged(ft) => { self.search_state.filter_type = ft; Task::none() }
            Message::SearchPressed => {
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
                            self.search_results = response.products;
                            self.status_message = format!("Found {} results", self.search_results.len());
                        }
                    }
                    Err(e) => { self.error_message = Some(format!("Search Error: {}", e)); }
                }
                Task::none()
            }
            Message::SearchResultsReceived(res) => {
                match res {
                    Ok(items) => { self.search_results = items; self.status_message = format!("Found {} results", self.search_results.len()); }
                    Err(e) => { self.error_message = Some(format!("Search Error: {}", e)); }
                }
                Task::none()
            }
            Message::ClearError => { self.error_message = None; Task::none() }
            Message::LocalSearchChanged(q) => { self.downloaded_state.search_query = q; Task::none() }
            Message::LocalFilterChanged(f) => { self.downloaded_state.filter = f; Task::none() }
            Message::ClosePreview => { self.current_image = None; Task::none() }
            _ => Task::none(),
        }
    }

    pub fn view(&self) -> Element<'_, Message, Theme, Renderer> {
        let _ = veldmap_rust_rpc::host::call_service("system", "log", "App: view() called".as_bytes().to_vec());
        let title_bar = column![
            text("VeldMap Tools").font(crate::common::APP_FONT).size(24),
            row![
                button(text("Search").font(crate::common::APP_FONT)).on_press(Message::SwitchMode(ViewMode::Search)).padding(8),
                button(text("Browse").font(crate::common::APP_FONT)).on_press(Message::SwitchMode(ViewMode::Browse)).padding(8),
                button(text("Downloaded").font(crate::common::APP_FONT)).on_press(Message::SwitchMode(ViewMode::Downloaded)).padding(8),
            ].spacing(10),
        ].spacing(10);

        let error_view: Element<Message, Theme, Renderer> = if let Some(err) = &self.error_message {
            container(row![
                button(text("X").font(crate::common::APP_FONT)).on_press(Message::ClearError).padding(5),
                text(err).font(crate::common::APP_FONT).size(13).color(Color::from_rgb(1.0, 0.4, 0.4)).width(Length::Fill),
            ].spacing(15).align_y(Alignment::Center))
            .padding(10)
            .style(|_| container::Style::default().background(Color::from_rgb(0.25, 0.1, 0.1)))
            .into()
        } else { column![].into() };

        let progress_view: Element<Message, Theme, Renderer> = if let Some(p) = self.download_progress {
            column![text(format!("Processing... {:.0}%", p * 100.0)).font(crate::common::APP_FONT).size(12), progress_bar(0.0..=1.0, p)].spacing(5).into()
        } else { column![].into() };

        let main_content: Element<Message, Theme, Renderer> = if let Some(handle) = &self.current_image {
            preview::view(handle)
        } else {
            match self.view_mode {
                ViewMode::Search => search::view(&self.search_state, &self.search_results),
                ViewMode::Browse => browse::view(&self.current_browse_path, &self.browse_items, &self.status_message, !self.token_stack.is_empty(), self.next_token.is_some()),
                ViewMode::Downloaded => downloaded::view(&self.downloaded_state, &self.local_files),
            }
        };

        container(column![title_bar, error_view, progress_view, main_content].spacing(10).padding(20))
            .width(Length::Fill).height(Length::Fill).into()
    }
}