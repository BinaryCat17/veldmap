use iced::widget::{
    button, column, container, horizontal_space, row, scrollable, text, vertical_space, progress_bar,
};
use iced::widget::image::Handle;
use iced::{Alignment, Element, Length, Task, Color, Theme};
use crate::common::{BrowserItem, icon_text, is_previewable};
use crate::utils::{generate_preview_with_progress, RawImage};
use veldmap_core::{RemoteDataSource, DataProduct, SearchFilter};
use std::sync::Arc;
use std::path::PathBuf;
use async_stream::stream;
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

pub struct VeldMapToolsGui {
    pub view_mode: ViewMode,
    pub status_message: String,
    pub error_message: Option<String>,
    pub search_state: search::SearchState,
    pub browse_items: Vec<BrowserItem>,
    pub downloaded_state: downloaded::DownloadedState,
    pub token_stack: Vec<String>,
    pub current_token: Option<String>,
    pub next_token: Option<String>,
    pub current_browse_path: String,
    pub search_results: Vec<DataProduct>,
    pub local_files: Vec<BrowserItem>,
    pub selected_product: Option<String>,
    pub product_files: Vec<BrowserItem>,
    pub download_progress: Option<f32>,
    pub current_image: Option<Handle>,
    pub source: Option<Arc<dyn RemoteDataSource>>,
}

#[derive(Clone)]
pub enum Message {
    SwitchMode(ViewMode),
    SearchInputChanged(String),
    SearchFilterTypeChanged(search::SearchFilterType),
    SearchPressed,
    SearchResultsReceived(Result<Vec<DataProduct>, String>),
    ProductSelected(DataProduct),
    FilesReceived(Result<Vec<String>, String>),
    BackToList,
    BrowseFetchPage(Option<String>),
    NextPage,
    PrevPage,
    BrowserItemsReceived(Result<(Vec<String>, Option<String>), String>),
    BrowsePath(String),
    BrowseUp,
    ScanLocalFiles,
    LocalFilesReceived(Vec<BrowserItem>),
    LocalSearchChanged(String),
    LocalFilterChanged(downloaded::FileFilter),
    DeleteLocalFile(String),
    SourceInitialized(Result<Arc<dyn RemoteDataSource>, String>),
    RetryConnection,
    DownloadFile(String),
    DownloadProgress(f32),
    DownloadCompleted(Result<PathBuf, String>),
    ViewFile(String),
    PreviewReady(Result<RawImage, String>),
    ClosePreview,
    ClearError,
}

impl std::fmt::Debug for Message {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Message")
    }
}

impl VeldMapToolsGui {
    pub fn new() -> (Self, Task<Message>) {
        (
            Self {
                view_mode: ViewMode::Search,
                status_message: "Initializing plugins...".to_string(),
                error_message: None,
                search_state: search::SearchState::default(),
                browse_items: Vec::new(),
                downloaded_state: downloaded::DownloadedState::default(),
                token_stack: Vec::new(),
                current_token: None,
                next_token: None,
                current_browse_path: String::new(),
                search_results: Vec::new(),
                local_files: Vec::new(),
                selected_product: None,
                product_files: Vec::new(),
                download_progress: None,
                current_image: None,
                source: None,
            },
            Task::perform(async move { 
                veldmap_core::create_data_provider().await.map_err(|e| e.to_string()) 
            }, Message::SourceInitialized),
        )
    }

    pub fn title(&self) -> String { "VeldMap Data Browser".into() }

    pub fn theme(&self) -> Theme { Theme::Dark }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::SwitchMode(mode) => {
                self.view_mode = mode;
                self.selected_product = None;
                self.current_image = None;
                self.download_progress = None;
                match self.view_mode {
                    ViewMode::Browse if self.browse_items.is_empty() && self.source.is_some() => return self.update(Message::BrowseFetchPage(None)),
                    ViewMode::Downloaded => return self.update(Message::ScanLocalFiles),
                    _ => {}
                }
                Task::none()
            }
            Message::SearchInputChanged(q) => { self.search_state.query = q; Task::none() }
            Message::SearchFilterTypeChanged(ft) => { self.search_state.filter_type = ft; Task::none() }
            Message::SearchPressed => {
                if let Some(source) = &self.source {
                    self.status_message = format!("Searching...");
                    let source = source.clone();
                    let query = self.search_state.query.clone();
                    let filter_type = self.search_state.filter_type;
                    self.selected_product = None;
                    Task::perform(async move {
                        let mut filters = Vec::new();
                        match filter_type {
                            search::SearchFilterType::GridId => filters.push(SearchFilter { name: "gridId".into(), value: query.clone() }),
                            search::SearchFilterType::Collection => filters.push(SearchFilter { name: "Collection".into(), value: query.clone() }),
                            _ => {}
                        }
                        let q = if filter_type == search::SearchFilterType::General { query } else { String::new() };
                        source.search(q, filters).await
                    }, Message::SearchResultsReceived)
                } else { Task::none() }
            }
            Message::SearchResultsReceived(res) => {
                match res {
                    Ok(items) => { self.search_results = items; self.status_message = format!("Found {} results", self.search_results.len()); }
                    Err(e) => { self.error_message = Some(format!("Search Error: {}", e)); }
                }
                Task::none()
            }
            Message::ProductSelected(prod) => {
                if let Some(source) = &self.source {
                    self.selected_product = Some(prod.name.clone());
                    let source = source.clone();
                    Task::perform(async move { source.list_path(prod.path, None).await.map(|res| res.items) }, Message::FilesReceived)
                } else { Task::none() }
            }
            Message::FilesReceived(res) => {
                match res {
                    Ok(files) => {
                        let base_path = PathBuf::from("data/dem/source");
                        self.product_files = files.into_iter().map(|identifier| {
                            let name = identifier.split('/').last().unwrap_or(&identifier).to_string();
                            let exists_locally = base_path.join(&name).exists();
                            BrowserItem { s3_key: identifier, name, is_folder: false, exists_locally }
                        }).collect();
                    }
                    Err(e) => { self.error_message = Some(format!("List Error: {}", e)); }
                }
                Task::none()
            }
            Message::BackToList => { self.selected_product = None; Task::none() }
            Message::BrowsePath(path) => {
                if path.is_empty() || path.ends_with('/') {
                    self.current_browse_path = path;
                    self.token_stack.clear();
                    self.current_token = None;
                    self.update(Message::BrowseFetchPage(None))
                } else { self.update(Message::DownloadFile(path)) }
            }
            Message::BrowseUp => {
                let current = self.current_browse_path.trim_end_matches('/');
                if let Some(idx) = current.rfind('/') {
                    self.current_browse_path = current[..=idx].to_string();
                } else { self.current_browse_path = String::new(); }
                self.token_stack.clear();
                self.current_token = None;
                self.update(Message::BrowseFetchPage(None))
            }
            Message::BrowseFetchPage(token) => {
                if let Some(source) = &self.source {
                    self.status_message = format!("Browsing...");
                    let source = source.clone();
                    self.current_token = token.clone();
                    let path = self.current_browse_path.clone();
                    Task::perform(async move { source.list_path(path, token).await.map(|res| (res.items, res.next_token)) }, Message::BrowserItemsReceived)
                } else { Task::none() }
            }
            Message::NextPage => {
                if let Some(token) = &self.next_token {
                    self.token_stack.push(self.current_token.clone().unwrap_or_else(|| "ROOT".to_string()));
                    return self.update(Message::BrowseFetchPage(Some(token.clone())));
                }
                Task::none()
            }
            Message::PrevPage => {
                if let Some(prev_token) = self.token_stack.pop() {
                    let t = if prev_token == "ROOT" { None } else { Some(prev_token) };
                    return self.update(Message::BrowseFetchPage(t));
                }
                Task::none()
            }
            Message::BrowserItemsReceived(res) => {
                match res {
                    Ok((items, next)) => {
                        let base_path = PathBuf::from("data/dem/source");
                        self.browse_items = items.into_iter().map(|identifier| {
                            let is_folder = identifier.ends_with('/');
                            let name = identifier.trim_end_matches('/').split('/').last().unwrap_or(&identifier).to_string();
                            let exists_locally = if is_folder { false } else { base_path.join(&name).exists() };
                            BrowserItem { s3_key: identifier, name, is_folder, exists_locally }
                        }).collect();
                        self.next_token = next;
                    }
                    Err(e) => { self.error_message = Some(format!("Browse Error: {}", e)); }
                }
                Task::none()
            }
            Message::ScanLocalFiles => {
                Task::perform(async move {
                    let mut items = Vec::new();
                    let dir = PathBuf::from("data/dem/source");
                    if let Ok(entries) = std::fs::read_dir(dir) {
                        for entry in entries.flatten() {
                            if let Ok(name) = entry.file_name().into_string() {
                                if !entry.path().is_dir() {
                                    items.push(BrowserItem { s3_key: name.clone(), name: name.clone(), is_folder: false, exists_locally: true });
                                }
                            }
                        }
                    }
                    items
                }, Message::LocalFilesReceived)
            }
            Message::LocalFilesReceived(items) => { self.local_files = items; Task::none() }
            Message::LocalSearchChanged(q) => { self.downloaded_state.search_query = q; Task::none() }
            Message::LocalFilterChanged(f) => { self.downloaded_state.filter = f; Task::none() }
            Message::DeleteLocalFile(name) => {
                let path = PathBuf::from("data/dem/source").join(&name);
                if path.exists() { let _ = std::fs::remove_file(path); }
                self.update(Message::ScanLocalFiles)
            }
            Message::SourceInitialized(res) => {
                match res {
                    Ok(s) => { self.source = Some(s); self.status_message = "Connected".into(); self.error_message = None; }
                    Err(e) => { self.error_message = Some(format!("Plugin Failed: {}. Check 'plugins/' folder.", e)); }
                }
                Task::none()
            }
            Message::RetryConnection => {
                self.error_message = None;
                self.status_message = "Reconnecting...".into();
                Task::perform(async move { veldmap_core::create_data_provider().await.map_err(|e| e.to_string()) }, Message::SourceInitialized)
            }
            Message::DownloadFile(identifier) => {
                let dest = self.get_local_path(&identifier);
                if dest.exists() { return Task::none(); }
                if let Some(source) = &self.source {
                    let source = source.clone();
                    self.download_progress = Some(0.0);
                    return Task::stream(stream! {
                        let s_task = source.clone();
                        let k_task = identifier.clone();
                        let d_task = dest.clone();
                        let _ = tokio::spawn(async move { s_task.download(k_task, d_task.to_string_lossy().to_string()).await }).await;
                        yield Message::DownloadProgress(1.0);
                        yield Message::DownloadCompleted(Ok(dest));
                    });
                }
                Task::none()
            }
            Message::DownloadProgress(p) => { self.download_progress = Some(p); Task::none() }
            Message::ViewFile(identifier) => {
                let is_local = !identifier.contains('/');
                let dest = if is_local { PathBuf::from("data/dem/source").join(&identifier) } else { self.get_local_path(&identifier) };
                let filename = identifier.split('/').last().unwrap_or("file").to_string();
                
                let source = self.source.clone();
                self.status_message = format!("Opening {}...", filename);
                self.download_progress = Some(0.0);

                return Task::stream(stream! {
                    if !dest.exists() {
                        if let Some(s) = source {
                            let k = identifier.clone();
                            let d = dest.clone();
                            let _ = tokio::spawn(async move { s.download(k, d.to_string_lossy().to_string()).await }).await;
                        }
                    }
                    
                    let (tx, mut rx) = tokio::sync::mpsc::channel(10);
                    let d = dest.clone();
                    let handle = tokio::task::spawn_blocking(move || {
                        generate_preview_with_progress(&d, Some(tx))
                    });

                    while let Some(p) = rx.recv().await {
                        yield Message::DownloadProgress(p);
                    }
                    
                    match handle.await {
                        Ok(Ok(raw)) => yield Message::PreviewReady(Ok(raw)),
                        Ok(Err(e)) => yield Message::PreviewReady(Err(e.to_string())),
                        Err(_) => yield Message::PreviewReady(Err("Thread panic".into())),
                    }
                });
            }
            Message::PreviewReady(res) => {
                self.download_progress = None;
                match res {
                    Ok(raw) => { 
                        self.current_image = Some(Handle::from_rgba(raw.width, raw.height, raw.pixels)); 
                        self.status_message = "Preview loaded".into(); 
                    }
                    Err(e) => { self.error_message = Some(format!("Preview Error: {}", e)); }
                }
                Task::none()
            }
            Message::ClosePreview => { self.current_image = None; Task::none() }
            Message::DownloadCompleted(res) => {
                self.download_progress = None;
                if let Ok(path) = res {
                    let base_path = PathBuf::from("data/dem/source");
                    for item in self.browse_items.iter_mut() {
                        let name = item.s3_key.split('/').last().unwrap_or("");
                        if base_path.join(name) == path { item.exists_locally = true; }
                    }
                    for item in self.product_files.iter_mut() {
                        let name = item.s3_key.split('/').last().unwrap_or("");
                        if base_path.join(name) == path { item.exists_locally = true; }
                    }
                }
                Task::none()
            }
            Message::ClearError => { self.error_message = None; Task::none() }
        }
    }

    pub fn view(&self) -> Element<'_, Message> {
        let title_bar = column![
            text("VeldMap Tools").size(24),
            row![
                button("Search").on_press(Message::SwitchMode(ViewMode::Search)).padding(8),
                button("Browse").on_press(Message::SwitchMode(ViewMode::Browse)).padding(8),
                button("Downloaded").on_press(Message::SwitchMode(ViewMode::Downloaded)).padding(8),
            ].spacing(10),
        ].spacing(10);

        let error_view: Element<Message> = if let Some(err) = &self.error_message {
            container(row![
                button("X").on_press(Message::ClearError).padding(5),
                text(err).size(13).color(Color::from_rgb(1.0, 0.4, 0.4)).width(Length::Fill),
            ].spacing(15).align_y(Alignment::Center))
            .padding(10)
            .style(|_| container::Style::default().background(Color::from_rgb(0.25, 0.1, 0.1)))
            .into()
        } else { column![].into() };

        let progress_view: Element<Message> = if let Some(p) = self.download_progress {
            column![text(format!("Processing... {:.0}%", p * 100.0)).size(12), progress_bar(0.0..=1.0, p).height(5)].spacing(5).into()
        } else { column![].into() };

        let main_content: Element<Message> = if let Some(handle) = &self.current_image {
            preview::view(handle)
        } else if let Some(product_name) = &self.selected_product {
            column![
                button("← Back").on_press(Message::BackToList).padding(5),
                text(format!("Product: {}", product_name)).size(18),
                scrollable(column(self.product_files.iter().map(|item| {
                    let previewable = is_previewable(&item.name);
                    let label_color = if item.exists_locally { Color::from_rgb(0.3, 0.8, 0.3) } else { Color::WHITE };
                    let download_btn: Element<Message> = if item.exists_locally { 
                        horizontal_space().width(0).into() 
                    } else { 
                        button("Download").on_press(Message::DownloadFile(item.s3_key.clone())).padding(3).into() 
                    };
                    let controls: Element<Message> = if previewable {
                        row![download_btn, button("View").on_press(Message::ViewFile(item.s3_key.clone())).padding(3)].spacing(5).into()
                    } else if item.exists_locally {
                        text("Ready").size(12).into()
                    } else {
                        button("Download").on_press(Message::DownloadFile(item.s3_key.clone())).padding(3).into()
                    };
                    row![
                        icon_text(if item.exists_locally { "✅" } else { "📄" }, &item.name, label_color),
                        horizontal_space().width(Length::Fill),
                        controls
                    ].spacing(20).align_y(Alignment::Center).into()
                }).collect::<Vec<Element<Message>>>()).spacing(8)).height(Length::Fill)
            ].spacing(15).into()
        } else {
            match self.view_mode {
                ViewMode::Search => search::view(&self.search_state, &self.search_results),
                ViewMode::Browse => if self.source.is_none() {
                    column![
                        text("No Data Provider Plugin found in 'plugins/'.").color(Color::from_rgb(0.8, 0.4, 0.4)),
                        button("Retry Loading Plugins").on_press(Message::RetryConnection).padding(10)
                    ].spacing(10).into()
                } else {
                    browse::view(&self.current_browse_path, &self.browse_items, &self.status_message, !self.token_stack.is_empty(), self.next_token.is_some())
                },
                ViewMode::Downloaded => downloaded::view(&self.downloaded_state, &self.local_files),
            }
        };

        container(column![title_bar, vertical_space().height(10), error_view, progress_view, main_content].spacing(10).padding(20))
            .width(Length::Fill).height(Length::Fill).into()
    }

    fn get_local_path(&self, identifier: &str) -> PathBuf {
        let filename = identifier.split('/').last().unwrap_or("downloaded");
        PathBuf::from("data/dem/source").join(filename)
    }
}