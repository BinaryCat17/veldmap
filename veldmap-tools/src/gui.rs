use iced::widget::{column, container, text, text_input, button, row, scrollable, vertical_space, pick_list, image as image_widget};
use iced::{Alignment, Element, Length, Task, Color};
use crate::copernicus::CopernicusSource;
use std::sync::Arc;
use std::path::PathBuf;
use std::fs::File;
use std::io::BufReader;

#[derive(Debug, Clone, PartialEq)]
pub enum ViewMode {
    Search,
    Browse,
    Index,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProductItem {
    name: String,
    path: String,
    grid_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Copy)]
pub enum BrowseRoot {
    Public30,
    Private30,
    CCMRoot,
    CCM30,
}

impl BrowseRoot {
    fn path(&self) -> &'static str {
        match self {
            BrowseRoot::Public30 => "auxdata/CopDEM/COP-DEM_GLO-30-DGED_PUBLIC/",
            BrowseRoot::Private30 => "auxdata/CopDEM/COP-DEM_GLO-30-DGED/",
            BrowseRoot::CCMRoot => "CCM/",
            BrowseRoot::CCM30 => "CCM/COP-DEM_GLO-30-DGED/",
        }
    }
    
    fn all() -> [BrowseRoot; 4] {
        [BrowseRoot::Public30, BrowseRoot::Private30, BrowseRoot::CCMRoot, BrowseRoot::CCM30]
    }
}

impl std::fmt::Display for BrowseRoot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", match self {
            BrowseRoot::Public30 => "Public GLO-30 (Aux)",
            BrowseRoot::Private30 => "Private GLO-30 (Aux)",
            BrowseRoot::CCMRoot => "CCM Root",
            BrowseRoot::CCM30 => "CCM GLO-30 (Main)",
        })
    }
}

pub struct VeldMapToolsGui {
    view_mode: ViewMode,
    search_query: String,
    last_grid_id: Option<String>,
    status_message: String,
    
    results: Vec<ProductItem>,
    
    // Browse state
    public_products: Vec<String>,
    token_stack: Vec<String>,
    current_token: Option<String>,
    next_token: Option<String>,
    current_browse_root: BrowseRoot,
    current_browse_path: String,
    
    selected_product: Option<String>,
    product_files: Vec<String>,
    
    // Preview
    current_image: Option<image::Handle>,
    
    // Index state
    index_products: Vec<ProductItem>,
    is_building_index: bool,
    
    source: Option<Arc<CopernicusSource>>,
}

impl Default for VeldMapToolsGui {
    fn default() -> Self {
        Self {
            view_mode: ViewMode::Search,
            search_query: String::new(),
            last_grid_id: None,
            status_message: "Initializing...".to_string(),
            results: Vec::new(),
            public_products: Vec::new(),
            token_stack: Vec::new(),
            current_token: None,
            next_token: None,
            current_browse_root: BrowseRoot::Public30,
            current_browse_path: BrowseRoot::Public30.path().to_string(),
            selected_product: None,
            product_files: Vec::new(),
            current_image: None,
            index_products: Vec::new(),
            is_building_index: false,
            source: None,
        }
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    SwitchMode(ViewMode),
    SearchInputChanged(String),
    SearchPressed,
    SourceInitialized(Result<Arc<CopernicusSource>, String>),
    SearchResultsReceived(Result<Vec<ProductItem>, String>),
    
    // Browse messages
    BrowseFetchPage(Option<String>),
    NextPage,
    PrevPage,
    PublicProductsReceived(Result<(Vec<String>, Option<String>), String>),
    BrowseRootSelected(BrowseRoot),
    BrowsePath(String), // Drill down
    BrowseUp,
    
    ProductSelected(ProductItem),
    FilesReceived(Result<Vec<String>, String>),
    BackToList,
    DownloadFile(String),
    DownloadCompleted(Result<PathBuf, String>),
    ViewFile(String),
    PreviewReady(Result<Vec<u8>, String>),
    ClosePreview,
    
    // Index messages
    BuildIndex,
    IndexBuilt(Result<Vec<ProductItem>, String>),
    LoadIndex,
}

impl VeldMapToolsGui {
    pub fn new() -> (Self, Task<Message>) {
        (
            Self::default(),
            Task::perform(async {
                let source = CopernicusSource::new().await
                    .map_err(|e| e.to_string())?;
                source.check_access().await.ok(); 
                Ok(Arc::new(source))
            }, Message::SourceInitialized),
        )
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::SwitchMode(mode) => {
                self.view_mode = mode;
                self.selected_product = None;
                self.product_files.clear();
                self.current_image = None;
                
                if self.view_mode == ViewMode::Browse && self.public_products.is_empty() {
                    return self.update(Message::BrowseFetchPage(None));
                }
                if self.view_mode == ViewMode::Index && self.index_products.is_empty() {
                    return self.update(Message::LoadIndex);
                }
                Task::none()
            }
            Message::SearchInputChanged(query) => {
                self.search_query = query;
                Task::none()
            }
            Message::SearchPressed => {
                // ... (Index search logic same as before)
                if self.view_mode == ViewMode::Index {
                    self.status_message = format!("Searching index for {}...", self.search_query);
                    let query = self.search_query.to_lowercase();
                    self.results = self.index_products.iter()
                        .filter(|p| p.name.to_lowercase().contains(&query))
                        .cloned()
                        .collect();
                    self.status_message = format!("Index: found {} matches", self.results.len());
                    return Task::none();
                }

                if let Some(source) = &self.source {
                    self.status_message = format!("Searching OData for {}...", self.search_query);
                    self.last_grid_id = Some(self.search_query.clone());
                    let source = source.clone();
                    let query = self.search_query.clone();
                    self.selected_product = None;
                    Task::perform(async move {
                        source.search_grid_id(&query).await
                            .map(|items| items.into_iter().map(|(n, p, g)| ProductItem { name: n, path: p, grid_id: g }).collect())
                            .map_err(|e| e.to_string())
                    }, Message::SearchResultsReceived)
                } else {
                    Task::none()
                }
            }
            // BROWSE LOGIC
            Message::BrowseRootSelected(root) => {
                self.current_browse_root = root;
                self.current_browse_path = root.path().to_string();
                self.token_stack.clear();
                self.current_token = None;
                self.update(Message::BrowseFetchPage(None))
            }
            Message::BrowsePath(path) => {
                if path.ends_with('/') {
                    self.current_browse_path = path;
                    self.token_stack.clear();
                    self.current_token = None;
                    self.update(Message::BrowseFetchPage(None))
                } else {
                    self.update(Message::DownloadFile(path))
                }
            }
            Message::BrowseUp => {
                let current = self.current_browse_path.trim_end_matches('/');
                if let Some(idx) = current.rfind('/') {
                    self.current_browse_path = current[..=idx].to_string();
                    self.token_stack.clear();
                    self.current_token = None;
                    self.update(Message::BrowseFetchPage(None))
                } else {
                    Task::none()
                }
            }
            Message::BrowseFetchPage(token) => {
                if let Some(source) = &self.source {
                    self.status_message = format!("Browsing {}...", self.current_browse_path);
                    let source = source.clone();
                    self.current_token = token.clone();
                    let path = self.current_browse_path.clone();
                    
                    Task::perform(async move {
                        source.list_browser_path(&path, token).await
                            .map_err(|e| e.to_string())
                    }, Message::PublicProductsReceived)
                } else {
                    Task::none()
                }
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
            Message::PublicProductsReceived(result) => {
                match result {
                    Ok((products, next)) => {
                        self.public_products = products;
                        self.next_token = next;
                        self.status_message = format!("Items: {}", self.public_products.len());
                    }
                    Err(e) => self.status_message = format!("Browse Error: {}", e),
                }
                Task::none()
            }
            Message::SourceInitialized(result) => {
                match result {
                    Ok(source) => {
                        self.source = Some(source);
                        self.status_message = "API Connected".to_string();
                        return self.update(Message::LoadIndex);
                    }
                    Err(e) => self.status_message = format!("Init Error: {}", e),
                }
                Task::none()
            }
            Message::SearchResultsReceived(result) => {
                match result {
                    Ok(results) => {
                        self.results = results;
                        self.status_message = format!("Found {} results", self.results.len());
                    }
                    Err(e) => self.status_message = format!("Search Error: {}", e),
                }
                Task::none()
            }
            Message::ProductSelected(item) => {
                if let Some(source) = &self.source {
                    self.selected_product = Some(item.name.clone());
                    self.status_message = "Fetching file list...".to_string();
                    let source = source.clone();
                    let path = item.path;
                    let grid_id = item.grid_id.or(Some(self.search_query.clone()));
                    
                    Task::perform(async move {
                        source.list_product_files(&path, grid_id.as_deref()).await
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
                        self.status_message = format!("Total files: {}", self.product_files.len());
                    }
                    Err(e) => self.status_message = format!("List Error: {}", e),
                }
                Task::none()
            }
            Message::BackToList => {
                self.selected_product = None;
                self.product_files.clear();
                self.current_image = None;
                Task::none()
            }
            Message::DownloadFile(s3_key) => {
                // Same download logic
                if let Some(source) = &self.source {
                    let source = source.clone();
                    let key = s3_key.clone();
                    let filename = if s3_key.contains("CCM/") && !s3_key.contains('.') {
                         let name = s3_key.trim_end_matches('/').split('/').last().unwrap_or("product");
                         format!("{}.zip", name)
                    } else {
                         s3_key.split('/').last().unwrap_or("downloaded.tif").to_string()
                    };
                    
                    let dest = PathBuf::from("data/dem/source").join(filename);
                    self.status_message = format!("Downloading to {:?}...", dest);

                    Task::perform(async move {
                        source.download_file(&key, &dest).await
                            .map(|_| dest)
                            .map_err(|e| e.to_string())
                    }, Message::DownloadCompleted)
                } else {
                    Task::none()
                }
            }
            Message::ViewFile(s3_key) => {
                // First download, then generate preview
                if let Some(source) = &self.source {
                    let source = source.clone();
                    let key = s3_key.clone();
                    let filename = s3_key.split('/').last().unwrap_or("preview.tif").to_string();
                    let dest = PathBuf::from("data/dem/source").join(&filename);
                    self.status_message = "Downloading for preview...".to_string();

                    Task::perform(async move {
                        // Check if file exists first to skip download
                        if !dest.exists() {
                            source.download_file(&key, &dest).await.map_err(|e| e.to_string())?;
                        }
                        // Generate preview
                        CopernicusSource::generate_preview(&dest).map_err(|e| e.to_string())
                    }, Message::PreviewReady)
                } else {
                    Task::none()
                }
            }
            Message::PreviewReady(result) => {
                match result {
                    Ok(bytes) => {
                        self.current_image = Some(image::Handle::from_bytes(bytes));
                        self.status_message = "Preview loaded".to_string();
                    }
                    Err(e) => {
                        self.status_message = format!("Preview Error: {}", e);
                    }
                }
                Task::none()
            }
            Message::ClosePreview => {
                self.current_image = None;
                Task::none()
            }
            Message::DownloadCompleted(result) => {
                match result {
                    Ok(path) => {
                        self.status_message = format!("Saved to {:?}", path);
                    }
                    Err(e) => {
                        self.status_message = format!("Download failed: {}", e);
                    }
                }
                Task::none()
            }
            Message::BuildIndex => {
                // ... (Index build logic)
                if let Some(source) = &self.source {
                    self.status_message = "Building index...".to_string();
                    self.is_building_index = true;
                    let source = source.clone();
                    Task::perform(async move {
                        let raw = source.fetch_full_product_index().await.map_err(|e| e.to_string())?;
                        let items: Vec<ProductItem> = raw.into_iter()
                            .map(|(n, p)| ProductItem { name: n, path: p, grid_id: None })
                            .collect();
                        
                        let file = File::create("dem_index.json").map_err(|e| e.to_string())?;
                        serde_json::to_writer(file, &items).map_err(|e| e.to_string())?;
                        
                        Ok(items)
                    }, Message::IndexBuilt)
                } else {
                    Task::none()
                }
            }
            Message::LoadIndex => {
                if let Ok(file) = File::open("dem_index.json") {
                    let reader = BufReader::new(file);
                    if let Ok(items) = serde_json::from_reader(reader) {
                        self.index_products = items;
                        self.status_message = format!("Loaded {} products from index", self.index_products.len());
                    }
                }
                Task::none()
            }
            Message::IndexBuilt(result) => {
                self.is_building_index = false;
                match result {
                    Ok(items) => {
                        self.index_products = items;
                        self.status_message = format!("Index built: {} products", self.index_products.len());
                    }
                    Err(e) => {
                        self.status_message = format!("Index error: {}", e);
                    }
                }
                Task::none()
            }
        }
    }

    pub fn view(&self) -> Element<'_, Message> {
        let title_bar = column![
            text("VeldMap Copernicus Explorer").size(24),
            row![
                button("Search").on_press(Message::SwitchMode(ViewMode::Search)).padding(8),
                button("Browse").on_press(Message::SwitchMode(ViewMode::Browse)).padding(8),
                button("Index").on_press(Message::SwitchMode(ViewMode::Index)).padding(8),
            ].spacing(10),
        ].spacing(10);

        let main_content = if let Some(product_name) = &self.selected_product {
            if let Some(handle) = &self.current_image {
                // IMAGE PREVIEW
                column![
                    button("Close Preview").on_press(Message::ClosePreview).padding(5),
                    image_widget(handle.clone()).width(Length::Fill).height(Length::Fill)
                ].spacing(10)
            } else {
                // FILE VIEW
                let files_content = if self.product_files.is_empty() {
                    column![
                        text("No individual files found via S3 listing.").size(14),
                        text("This might be a restricted CCM product or an archive.").size(12),
                        button("Download Product (Raw)").on_press(Message::DownloadFile(
                            self.results.iter().chain(self.index_products.iter())
                                .find(|i| i.name == *product_name)
                                .map(|i| i.path.clone())
                                .unwrap_or_default()
                        )).padding(10)
                    ].spacing(10)
                } else {
                    column(
                        self.product_files.iter().map(|f| {
                            let filename = f.split('/').last().unwrap_or(f);
                            let is_tif = filename.to_lowercase().ends_with(".tif");
                            
                            let controls: Element<Message> = if is_tif { 
                                row![
                                    button("Download").on_press(Message::DownloadFile(f.clone())).padding(3),
                                    button("View").on_press(Message::ViewFile(f.clone())).padding(3)
                                ].spacing(5).into()
                            } else { 
                                button("Download").on_press(Message::DownloadFile(f.clone())).padding(3).into()
                            };
                            
                            row![
                                text(filename).size(14),
                                controls
                            ].spacing(20).align_y(Alignment::Center).into()
                        }).collect::<Vec<Element<Message>>>()
                    ).spacing(8).padding(10)
                };

                column![
                    button("← Back to results").on_press(Message::BackToList).padding(5),
                    text(format!("Folder: {}", product_name)).size(18),
                    text(&self.status_message).size(12).color(Color::from_rgb(0.4, 0.4, 0.4)),
                    scrollable(files_content).height(Length::Fill)
                ].spacing(15)
            }
        } else {
            // LIST VIEW (Simplified for brevity, same as before but calling Element::from() where needed)
            match self.view_mode {
                ViewMode::Browse => {
                    column![
                        row![
                            pick_list(BrowseRoot::all(), Some(self.current_browse_root), Message::BrowseRootSelected),
                            button("UP").on_press(Message::BrowseUp),
                        ].spacing(10),
                        text(format!("Path: {}", self.current_browse_path)).size(12),
                        row![
                            button("PREV").on_press_maybe(if self.token_stack.is_empty() { None } else { Some(Message::PrevPage) }),
                            text(&self.status_message).size(12),
                            button("NEXT").on_press_maybe(if self.next_token.is_some() { Some(Message::NextPage) } else { None }),
                        ].spacing(20).align_y(Alignment::Center),
                        scrollable(
                            column(
                                self.public_products.iter().map(|p| {
                                    let name = p.trim_end_matches('/').split('/').last().unwrap_or(p);
                                    let is_folder = p.ends_with('/');
                                    let label = if is_folder { format!("📁 {}", name) } else { format!("📄 {}", name) };
                                    
                                    if is_folder {
                                        button(text(label).size(15))
                                            .on_press(Message::BrowsePath(p.clone()))
                                            .width(Length::Fill)
                                            .padding(8).into()
                                    } else {
                                        // It's a file in browse mode
                                        let is_tif = name.to_lowercase().ends_with(".tif");
                                        let btn = button(
                                            row![
                                                text(label).size(15),
                                                if is_tif { text("(View available)").size(10).color(Color::from_rgb(0.0, 0.5, 0.0)) } else { text("").size(0) }
                                            ].spacing(10)
                                        )
                                        .on_press(if is_tif { Message::ViewFile(p.clone()) } else { Message::DownloadFile(p.clone()) })
                                        .width(Length::Fill)
                                        .padding(8);
                                        Element::from(btn)
                                    }
                                }).collect::<Vec<Element<Message>>>()
                            ).spacing(5)
                        ).height(Length::Fill)
                    ].spacing(15)
                },
                _ => { // Search/Index view
                    column![
                        row![
                            text_input("Enter gridId / Filter", &self.search_query)
                                .on_input(Message::SearchInputChanged)
                                .on_submit(Message::SearchPressed)
                                .padding(10),
                            button("Find").on_press(Message::SearchPressed).padding(10),
                            if self.view_mode == ViewMode::Index {
                                let btn = button(if self.is_building_index { "Building..." } else { "Rebuild Index" })
                                    .on_press(if self.is_building_index { Message::SearchPressed } else { Message::BuildIndex })
                                    .padding(10);
                                Element::from(btn)
                            } else {
                                Element::from(vertical_space().width(0))
                            }
                        ].spacing(10).align_y(Alignment::Center),
                        text(&self.status_message).size(12),
                        scrollable(
                            column(
                                self.results.iter().map(|item| {
                                    button(
                                        column![
                                            text(&item.name).size(15),
                                            text(item.grid_id.as_deref().unwrap_or("")).size(10).color(Color::from_rgb(0.0, 0.5, 0.0)),
                                            text(&item.path).size(10).color(Color::from_rgb(0.5, 0.5, 0.5)),
                                        ]
                                    )
                                    .on_press(Message::ProductSelected(item.clone()))
                                    .width(Length::Fill)
                                    .padding(8).into()
                                }).collect::<Vec<Element<Message>>>()
                            ).spacing(5)
                        ).height(Length::Fill)
                    ].spacing(15)
                }
            }
        };

        container(column![title_bar, vertical_space().height(10), main_content].spacing(10).padding(20))
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }
}