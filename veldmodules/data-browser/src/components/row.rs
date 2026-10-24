use crate::module::state::library::{status_of, LibraryState};
use crate::module::state::overlay::{Pace, Progress};
use crate::proto::data_library::{LibraryEntry, LibraryStatus};

#[derive(Clone, PartialEq)]
pub enum RowStatus {
    Remote,
    Downloading { done: u64, total: u64 },
    Paused { done: u64, total: u64, trouble: String },
    Complete,
    Partial { done: usize, trouble: String },
}

impl RowStatus {
    pub fn progress(&self) -> Option<(u64, u64)> {
        match self {
            RowStatus::Downloading { done, total } | RowStatus::Paused { done, total, .. } if *total > 0 => {
                Some((*done, *total))
            }
            _ => None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum OnOutline {
    #[default]
    Off,
    Asking,
    Blank,
    Failed,
    Drawn,
}

#[derive(Clone, Copy, PartialEq, Default)]
pub enum OnGlobe {
    #[default]
    Off,
    Asked,
    Assembling,
    Laid { hidden: bool, progress: Progress },
}

impl OnGlobe {
    pub fn pace(self) -> Pace {
        match self {
            OnGlobe::Off => Pace::Idle,
            OnGlobe::Asked | OnGlobe::Assembling => Pace::Unknown,
            OnGlobe::Laid { progress, .. } => progress.pace(),
        }
    }

    pub fn parts(self) -> Vec<String> {
        match self {
            OnGlobe::Off => Vec::new(),
            OnGlobe::Asked => vec!["asking the catalogue…".to_string()],
            OnGlobe::Assembling => vec!["opening rasters…".to_string()],
            OnGlobe::Laid { progress, .. } => progress.parts(),
        }
    }

    pub fn said(self) -> Option<String> {
        let parts = self.parts();
        match parts.is_empty() {
            true => None,
            false => Some(parts.join(" · ")),
        }
    }

    pub fn any(self) -> bool {
        !matches!(self, OnGlobe::Off)
    }

    pub fn laid(self) -> bool {
        matches!(self, OnGlobe::Assembling | OnGlobe::Laid { .. })
    }

}

#[derive(Clone, Copy, PartialEq)]
pub enum RowKind {
    File,
    Folder,
    Product { folder: bool },
}

impl RowKind {
    pub fn is_folder(self) -> bool {
        matches!(self, RowKind::Folder | RowKind::Product { folder: true })
    }

    pub fn is_product(self) -> bool {
        matches!(self, RowKind::Product { .. })
    }
}

pub fn bare(key: &str) -> &str {
    key.trim_end_matches('/')
}

pub fn last_segment(path: &str) -> &str {
    bare(path).rsplit('/').next().unwrap_or("")
}

pub fn folder_of(identifier: &str) -> &str {
    let path = bare(identifier);
    match path.rfind('/') {
        Some(cut) => &path[..cut],
        None => "",
    }
}

pub fn folder_path(identifier: &str) -> String {
    format!("{}/", bare(identifier))
}

pub fn preview_of(
    library: &LibraryState,
    identifier: &str,
    folder: bool,
) -> crate::module::ViewMsg {
    use crate::module::ViewMsg;
    if let Some(name) = library.local_name(identifier) {
        return ViewMsg::Preview(name.to_string());
    }
    match folder {
        true => ViewMsg::PreviewProduct(identifier.to_string()),
        false => ViewMsg::PreviewRemote(identifier.to_string()),
    }
}

const SCENE: &str = "scene:";

pub fn scene_key(identifier: &str) -> String {
    format!("{}{}", SCENE, identifier)
}

pub fn is_scene_key(key: &str) -> bool {
    key.starts_with(SCENE)
}

pub struct Row {
    pub identifier: String,
    pub group: Option<String>,
    pub name: String,
    pub title: String,
    pub kind: RowKind,
    pub outlined: OnOutline,
    pub size: u64,
    pub date: i64,
    pub product_type: String,
    pub product: String,
    pub children: Vec<Row>,
    pub folded: bool,
    pub loading: bool,
    pub globe: OnGlobe,
    pub unviewable: String,
    pub unshowable: String,
    pub status: RowStatus,
}

impl Default for Row {
    fn default() -> Row {
        Row {
            identifier: String::new(),
            group: None,
            name: String::new(),
            title: String::new(),
            kind: RowKind::File,
            outlined: OnOutline::Off,
            size: 0,
            date: 0,
            product_type: String::new(),
            product: String::new(),
            children: Vec::new(),
            folded: false,
            loading: false,
            globe: OnGlobe::Off,
            unviewable: String::new(),
            unshowable: String::new(),
            status: RowStatus::Remote,
        }
    }
}

impl Row {
    pub fn key(&self) -> &str {
        if let Some(group) = &self.group {
            return group;
        }
        if self.identifier.is_empty() { &self.name } else { &self.identifier }
    }

    pub fn snapshot_key(&self) -> &str {
        bare(&self.identifier)
    }

    pub fn product_key(&self) -> &str {
        match self.product.is_empty() {
            false => bare(&self.product),
            true => self.snapshot_key(),
        }
    }

    pub fn named(&self, key: &str) -> bool {
        !key.is_empty()
            && (bare(self.key()) == bare(key) || bare(&self.identifier) == bare(key))
    }

    pub fn is_snapshot(&self) -> bool {
        self.kind.is_product()
            || (!self.product.is_empty() && self.product == self.snapshot_key())
    }

    pub fn choice_key(&self) -> &str {
        match self.is_snapshot() {
            true => self.snapshot_key(),
            false => self.key(),
        }
    }

    pub fn choosable(&self) -> bool {
        !matches!(self.kind, RowKind::Folder) && !self.choice_key().is_empty()
    }

    pub fn expandable(&self) -> bool {
        self.folded || self.group.is_some() || self.kind.is_folder()
    }

    pub fn folder(&self) -> &str {
        folder_of(&self.identifier)
    }

    pub fn stored(&self) -> u64 {
        if self.folded {
            return self.children.iter().map(Row::stored).sum();
        }
        match &self.status {
            RowStatus::Complete => self.size,
            RowStatus::Downloading { done, .. } | RowStatus::Paused { done, .. } => *done,
            RowStatus::Remote | RowStatus::Partial { .. } => 0,
        }
    }

    pub fn format(&self) -> String {
        if !self.product_type.is_empty() {
            return self.product_type.clone();
        }
        match self.kind {
            RowKind::Product { .. } => return "scene".to_string(),
            RowKind::Folder => return "folder".to_string(),
            RowKind::File => {}
        }
        match self.title.rfind('.') {
            Some(dot) => self.title[dot + 1..].to_uppercase(),
            None => String::new(),
        }
    }

    pub fn container_row(identifier: String, title: String, status: RowStatus, kind: RowKind) -> Row {
        Row { identifier, title, kind, status, ..Default::default() }
    }

    pub fn from_entry(entry: &LibraryEntry) -> Row {
        let status = match status_of(entry) {
            LibraryStatus::LibDownloading => RowStatus::Downloading { done: entry.done, total: entry.total },
            LibraryStatus::LibPaused => RowStatus::Paused {
                done: entry.done,
                total: entry.total,
                trouble: entry.trouble.clone(),
            },
            LibraryStatus::LibComplete => RowStatus::Complete,
        };
        Row {
            identifier: entry.identifier.clone(),
            name: entry.name.clone(),
            title: last_segment(&entry.name).to_string(),
            kind: RowKind::File,
            size: if entry.total > 0 { entry.total } else { entry.done },
            date: entry.modified,
            product: entry.product.clone(),
            status,
            ..Default::default()
        }
    }

    pub fn remote(
        library: &LibraryState,
        identifier: String,
        title: String,
        size: u64,
        date: i64,
        kind: RowKind,
    ) -> Row {
        match library.by_identifier(&identifier) {
            Some(entry) => Row {
                title,
                kind,
                size: if size > 0 { size } else { entry.total.max(entry.done) },
                date: if date > 0 { date } else { entry.modified },
                ..Row::from_entry(entry)
            },
            None => Row { identifier, title, kind, size, date, ..Default::default() },
        }
    }
}

pub fn downloaded_rows(library: &LibraryState) -> Vec<Row> {
    let mut products: Vec<(String, Vec<Row>)> = Vec::new();
    let mut alone: Vec<Row> = Vec::new();

    for entry in &library.entries {
        let row = Row::from_entry(entry);
        if entry.product.is_empty() {
            alone.push(row);
            continue;
        }
        match products.iter_mut().find(|(key, _)| key == &entry.product) {
            Some((_, files)) => files.push(row),
            None => products.push((entry.product.clone(), vec![row])),
        }
    }

    products
        .into_iter()
        .map(|(key, mut files)| match files.len() == 1 && files[0].identifier == key {
            true => files.remove(0),
            false => {
                let (_, siblings) = library.snapshot(&key);
                snapshot(key, siblings, files)
            }
        })
        .chain(alone)
        .collect()
}

fn common_trouble(files: &[Row]) -> String {
    let mut said: Option<&str> = None;
    for file in files {
        let RowStatus::Paused { trouble, .. } = &file.status else { continue };
        match said {
            None => said = Some(trouble),
            Some(before) if before == trouble => {}
            Some(_) => return String::new(),
        }
    }
    said.unwrap_or_default().to_string()
}

fn snapshot(product: String, siblings: u32, files: Vec<Row>) -> Row {
    let done = files.iter().filter(|file| matches!(file.status, RowStatus::Complete)).count();
    let downloading = files.iter().any(|file| matches!(file.status, RowStatus::Downloading { .. }));
    let size = files.iter().map(Row::stored).sum();
    let date = files.iter().map(|file| file.date).max().unwrap_or(0);

    let status = match (downloading, done, files.len()) {
        (true, ..) => RowStatus::Downloading { done: size, total: 0 },
        (false, done, total) if siblings > 0 && done == total && total as u32 == siblings => {
            RowStatus::Complete
        }
        (false, 0, _) => RowStatus::Paused { done: size, total: 0, trouble: common_trouble(&files) },
        (false, done, _) => RowStatus::Partial { done, trouble: common_trouble(&files) },
    };

    Row {
        identifier: product.clone(),
        title: last_segment(&product).to_string(),
        kind: RowKind::Product { folder: true },
        size,
        date,
        product,
        children: files,
        folded: true,
        status,
        ..Default::default()
    }
}
