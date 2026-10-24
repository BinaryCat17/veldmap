//! components/row.rs — a list row: what all three views show.
//!
//! The record's state is not derived here: data-library sends it, being the
//! only one that knows what lies on disk, what is downloading, and where it
//! came from. What remains is the mapping "catalogue record -> what we draw".

use crate::module::state::library::{status_of, LibraryState};
use crate::module::state::overlay::{Pace, Progress};
use crate::proto::data_library::{LibraryEntry, LibraryStatus};

/// Row state. A sum type, not a set of boolean fields: combinations like
/// "half-downloaded, but zero bytes and no task" stop being expressible.
#[derive(Clone, PartialEq)]
pub enum RowStatus {
    /// No record exists in the library — only the provider has one.
    Remote,
    Downloading { done: u64, total: u64 },
    /// Started but not finished. `done: 0` — the download failed before the
    /// first bytes.
    ///
    /// `trouble` — why the last attempt failed. Empty — there is no reason:
    /// a person stopped the download, or the attempt predates a restart (the
    /// reason lives in the library's memory), or this is a scene row whose
    /// files are stopped for different reasons. The record is stopped the same
    /// way in every case and resumes the same way, so this is not a separate
    /// state but a reason attached to it: the only difference is whether the
    /// person knows why it is stopped.
    Paused { done: u64, total: u64, trouble: String },
    Complete,
    /// A folder or scene, part of whose content is already on disk. The
    /// catalogue does not say how many files there are in total, so only the
    /// downloaded count is stated.
    ///
    /// `trouble` — why the rest is stopped; empty — nothing is stopped or the
    /// parts have different reasons. It is needed here for the same reason as
    /// on an interrupted row, and even more so: "3 on disk" is a calm, green
    /// label, and a failure behind it is not visible at all.
    Partial { done: usize, trouble: String },
}

impl RowStatus {
    /// How much is done out of how much, if anything is running at all.
    /// `None` — there is no room for a download bar in this row.
    pub fn progress(&self) -> Option<(u64, u64)> {
        match self {
            RowStatus::Downloading { done, total } | RowStatus::Paused { done, total, .. } if *total > 0 => {
                Some((*done, *total))
            }
            _ => None,
        }
    }
}

/// Whether the row's scene is outlined on the globe.
///
/// Its own state, not derived from selection: outline, raster display, and row
/// selection are three independent things, and merged into one they would
/// answer for each other. Two parties ask for it: the outline icon in the row
/// (whether it lights up) and the tooltip beside it — one can only hover what
/// is on the globe.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum OnOutline {
    #[default]
    Off,
    /// Outlining was requested, but the geometry is still coming from the
    /// catalogue: there is nothing to draw yet, though the click is accepted.
    Asking,
    /// Requested, but the scene has no geometry at all — nothing to draw and
    /// nothing to wait for. A state separate from [`OnOutline::Asking`],
    /// because otherwise the icon would promise an outline that never arrives.
    Blank,
    /// Asking failed — the network or the service refused. It says nothing
    /// about the scene itself, and is therefore separate from
    /// [`OnOutline::Blank`]: there nothing can be re-asked, while here that is
    /// all that is left.
    Failed,
    /// Drawn.
    Drawn,
}

/// How the row's scene lies on the globe.
///
/// One answer for all screens and all three lists. This knowledge belongs to
/// the application, not the tab — there is one globe but any number of lists —
/// and four parties ask for it: the globe icon in the row (whether it lights
/// up), the progress bar under the row, the layer list, and the hatching that
/// marks the scene's place on the globe while there is no picture yet (see
/// `handlers::outline::send`). Expressed four times inline they would silently
/// diverge: the icon would stay lit for a removed layer until the very next
/// layout rebuild.
///
/// Nothing is said about the outline here: it has its own state — [`OnOutline`].
#[derive(Clone, Copy, PartialEq, Default)]
pub enum OnGlobe {
    #[default]
    /// It is not on the globe as a raster.
    Off,
    /// Display was requested, but the product under this key is still being
    /// reassembled by the catalogue: there is no overlay at all. Separate from
    /// [`OnGlobe::Assembling`], because in this position there is nothing to
    /// aim at — neither a layer nor its frame exists yet — while in every other
    /// respect they say the same thing.
    Asked,
    /// Rasters are being requested from the provider or opened: the scene is
    /// not on the globe yet, but it is on its way there.
    Assembling,
    /// It lies as a raster. `hidden` — it stays in the set but is not drawn;
    /// `progress` — the acquisition progress as the globe reported it.
    ///
    /// The whole progress, not just one share of it: the bar draws the share
    /// while the caption explains it, and both must be computed from the same
    /// source — otherwise the bar and the tooltip under one icon say different
    /// things.
    Laid { hidden: bool, progress: Progress },
}

impl OnGlobe {
    /// What to draw the bar under the row with (see [`Pace`]). Assembling has
    /// no share at all: rasters are still being requested and opened, and
    /// there is nothing to count there — but the fact that work has started
    /// must be stated exactly then, because that is the first thing the person
    /// who clicked the icon must see.
    pub fn pace(self) -> Pace {
        match self {
            OnGlobe::Off => Pace::Idle,
            OnGlobe::Asked | OnGlobe::Assembling => Pace::Unknown,
            OnGlobe::Laid { progress, .. } => progress.pace(),
        }
    }

    /// Progress in parts, from coarse to fine: what the bar counts, how much
    /// has been read, and what is arriving right now. Empty — the work is done
    /// or there is nothing to say about it yet.
    ///
    /// In parts because this phrase has two places of different widths: not
    /// everything fits in the column cell, while the tooltip fits everything
    /// (see `Progress::parts`).
    pub fn parts(self) -> Vec<String> {
        match self {
            OnGlobe::Off => Vec::new(),
            OnGlobe::Asked => vec!["asking the catalogue…".to_string()],
            OnGlobe::Assembling => vec!["opening rasters…".to_string()],
            OnGlobe::Laid { progress, .. } => progress.parts(),
        }
    }

    /// The same progress in one phrase. `None` — there is nothing to say.
    pub fn said(self) -> Option<String> {
        let parts = self.parts();
        match parts.is_empty() {
            true => None,
            false => Some(parts.join(" · ")),
        }
    }

    /// Whether the scene is on the globe in any form — that is what makes the
    /// icon light up, and the same reserves the room for the bar.
    pub fn any(self) -> bool {
        !matches!(self, OnGlobe::Off)
    }

    /// Whether there is anywhere to aim the camera: a layer exists — its frame
    /// is computed. One request is not enough: until the catalogue answers,
    /// not even the scene's location is known.
    pub fn laid(self) -> bool {
        matches!(self, OnGlobe::Assembling | OnGlobe::Laid { .. })
    }

}

/// What the record is to the application. A sum type, not a pair of boolean
/// fields: "folder" and "scene" are not two independent properties but one
/// kind. A pair cannot express it: a scene can be both a directory and a
/// single object, while "a folder that is not a scene" and "a scene that is
/// not a folder" are different rows with different actions, not combinations
/// of flags.
#[derive(Clone, Copy, PartialEq)]
pub enum RowKind {
    File,
    /// A folder of the path: inside are scenes or other folders. By itself it
    /// shows nothing — one only enters it.
    Folder,
    /// A scene — one logical unit. It may lie as an expanded directory (.SAFE,
    /// .SEN3) or as a single object; the display does not care: the decoder
    /// assembles both into one raster. `folder` only says whether there is
    /// anything to enter inside.
    Product { folder: bool },
}

impl RowKind {
    /// Whether there is content inside to enter. A catalogue folder has it,
    /// but entering is not its main action: the scene is viewed as a whole.
    pub fn is_folder(self) -> bool {
        matches!(self, RowKind::Folder | RowKind::Product { folder: true })
    }

    /// Whether this is a scene — what is put on the globe and viewed as one
    /// picture.
    pub fn is_product(self) -> bool {
        matches!(self, RowKind::Product { .. })
    }
}

/// The bare storage key: without the trailing slash that a listing uses to
/// mark a folder.
///
/// The slash is a mark of a listing row, not part of the path: the same
/// product arrives from the catalogue with a slash and from search results
/// without it. The rule is written here once for everyone — the record's
/// folder, the listing path, the scene key, and the name it is called by (see
/// [`last_segment`]) all depend on it, as does comparing a row with a
/// navigation key, while five copies of it would one day count one scene as
/// two.
pub fn bare(key: &str) -> &str {
    key.trim_end_matches('/')
}

/// The last path segment — what the path is called to a person: the file name
/// for an object key, the scene name for a directory key, the title for a tab
/// path.
///
/// The trailing slash does not count (see [`bare`]): it marks a listing row
/// rather than separating an empty segment after itself, so
/// `eodata/S2B_X.SAFE/` is called by the same name as `eodata/S2B_X.SAFE` —
/// there is one scene behind them. Only a path without a single segment
/// answers empty: the root and the empty key — and this reads as "there is
/// nowhere to take a name from", not as "the name is empty".
pub fn last_segment(path: &str) -> &str {
    bare(path).rsplit('/').next().unwrap_or("")
}

/// The folder the provider key lies in. Derived from the key itself — neither
/// the catalogue nor the library has a field for it — and written here once
/// for everyone: both the list row and the bar under the globe ask for it, and
/// two copies of the "where the path ends" rule would one day answer
/// differently.
pub fn folder_of(identifier: &str) -> &str {
    let path = bare(identifier);
    match path.rfind('/') {
        Some(cut) => &path[..cut],
        None => "",
    }
}

/// The path used to browse this record's content: a folder key always ends
/// with a slash. Without it, a prefix listing would show the folder itself
/// instead of what lies inside it.
pub fn folder_path(identifier: &str) -> String {
    format!("{}/", bare(identifier))
}

/// How to view a scene known by a single key.
///
/// One rule for everyone who offers "view" outside a list row: the bar under
/// the globe and the layer list. A row decides this on its own — the kind and
/// the library record are written right into it.
///
/// The order of questions goes from cheap to expensive. Downloaded content is
/// viewed from disk: the file is at hand, and going over the network for it
/// means waiting for what already exists. A scene lying as a directory is
/// opened through the provider — it picks the raster inside, because a `GET`
/// on a directory path answers 404. Everything else is opened directly.
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

/// Prefix of a scene row key whose scene has several packages
/// (see [`scene_key`]).
const SCENE: &str = "scene:";

/// The key of a scene row assembled from several packages.
///
/// It cannot be the product path: the shown package stands under the scene as
/// a row of its own, and with equal keys both would expand at once. Colons
/// never occur in storage keys, so the prefix collides with nothing.
pub fn scene_key(identifier: &str) -> String {
    format!("{}{}", SCENE, identifier)
}

/// The scene row key is not a storage path: there is nothing to browse by it,
/// the packages arrived along with the catalogue answer.
pub fn is_scene_key(key: &str) -> bool {
    key.starts_with(SCENE)
}

pub struct Row {
    /// Provider key; empty — unknown, then resuming a download and remote
    /// preview are not offered.
    pub identifier: String,
    /// The row's own key — only for a scene with several packages
    /// (see [`scene_key`]). All others have `None`, and the path serves as
    /// their key.
    pub group: Option<String>,
    /// The record's name in the library — the key for removal and preview.
    /// Empty if there is no record yet (the file is not downloaded).
    pub name: String,
    /// Displayed name.
    pub title: String,
    pub kind: RowKind,
    /// Whether the row's scene is outlined (see [`OnOutline`]).
    pub outlined: OnOutline,
    /// Size in bytes; 0 — unknown.
    pub size: u64,
    /// File time, unix seconds; 0 — unknown.
    pub date: i64,
    /// What the catalogue calls the record: for a scene this is the product
    /// type ("S2MSI2A"). Empty — the catalogue did not say, and the format is
    /// derived from the kind and the name (see [`Row::format`]).
    pub product_type: String,
    /// The scene this record belongs to; empty — it stands on its own. It
    /// travels with it up to the download itself: the library writes the file
    /// into the scene it was called from, and it cannot derive that from the
    /// key — the provider knows the bucket layout (see `ListEntry.product`).
    pub product: String,
    /// The row's content: the scene's files or the records of an expanded
    /// folder.
    ///
    /// On a folded row they are always present — it is folded from them, and
    /// everything known about it is computed from them (see [`Row::stored`]);
    /// a catalogue folder is filled only when expanded, because before the
    /// expansion they were never requested. Whether to show the content or
    /// stay silent about it is not this field's business but the layout's
    /// (see `arrange::expand`).
    pub children: Vec<Row>,
    /// The row is folded from library records rather than being a record
    /// itself. Its files are what must be downloaded and opened: its key is
    /// the scene's path in storage, and sending it to the download would mean
    /// asking for the folder to be downloaded as a single object.
    ///
    /// A separate field rather than "has children": an expanded catalogue
    /// folder also has children, and it is exactly an ordinary record, with
    /// nobody taking away its entry.
    pub folded: bool,
    /// The expanded folder's content is still arriving. Emptiness under it
    /// reads as "there is nothing here", and that is untrue until the listing
    /// ends.
    pub loading: bool,
    /// How this row's scene lies on the globe (see [`OnGlobe`]). It stands
    /// here, not in the layout, because it is asked by the scene key, and the
    /// row has the key.
    pub globe: OnGlobe,
    /// Why not to offer "to the globe"; empty — offer it. The provider said it
    /// — only it knows both the processing level and which formats the overlay
    /// opens (see `DataProduct.unviewable`).
    ///
    /// A reason rather than a flag: an icon over something that will surely
    /// not display promises what never happens, but silently removing it
    /// leaves one guessing. With a reason it stays in place, disabled, and
    /// explains itself with a tooltip — the same way a hover on a scene that is
    /// not on the globe is explained. In a cramped panel there is no
    /// explanation: there icons become menu items, and a disabled one never
    /// leaves (see `table::actions`).
    ///
    /// For a row the provider said nothing about it is empty: silence is no
    /// reason to take an action away (this is how downloaded content stands —
    /// the library assembled it), while a refusal, if any, explains itself
    /// after the click.
    pub unviewable: String,
    /// Why showing this scene failed last time; empty — not attempted or it
    /// succeeded.
    ///
    /// Separate from [`Row::unviewable`], though both say "it will not
    /// display": the difference is who knows it and what to do next. The
    /// provider knows in advance and for sure — the icon goes out and there is
    /// nothing to press. This reason is the outcome of a real attempt, and the
    /// attempt may have failed on the network: the icon stays pressable but
    /// carries a warning and holds the reason in a tooltip. Merged into one
    /// field, they would force "cannot at all" and "did not work now" into one
    /// look.
    pub unshowable: String,
    pub status: RowStatus,
}

/// A row about which nothing is known: no key, no content, not on the globe.
/// It exists for `..Default::default()` in row assemblies — it has a dozen and
/// a half fields, and listed once per assembly they silently drift apart: an
/// added field reaches only some of them.
///
/// Written by hand, not derived: two fields do not answer with their type's
/// zero. The default kind is File: it is the only record that contains
/// nothing; the state is `Remote`: until the library has spoken about the
/// record, the record lies in storage. The refusal reason is empty at the same
/// time, and here the type's zero speaks for itself: the provider's silence is
/// no reason to take an action away (see [`Row::unviewable`]).
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
    /// The row's stable name in the list — by it rows are matched between
    /// frames (see `Element::key`) and by it the row's menu is addressed. The
    /// provider key, and failing that the record name: both cannot be empty at
    /// once, otherwise the row has nowhere to come from.
    pub fn key(&self) -> &str {
        if let Some(group) = &self.group {
            return group;
        }
        if self.identifier.is_empty() { &self.name } else { &self.identifier }
    }

    /// The scene key as the provider knows it (see [`bare`]). By it the scene
    /// is marked on the globe, put on it, and opened. Empty for a record the
    /// provider does not know at all — such a scene can be neither outlined
    /// nor shown.
    pub fn snapshot_key(&self) -> &str {
        bare(&self.identifier)
    }

    /// The key of the scene this row belongs to: its own for a scene, the
    /// product root for a file inside one.
    ///
    /// It is asked where the action is about the scene while a file was
    /// clicked: what goes on the globe is the scene, and the overlay is laid
    /// under the key the provider calls it by (see
    /// `handlers::overlay::on_located`). Were it called by the file key,
    /// removing what the same menu item laid would be impossible: under the
    /// file key there is nothing on the globe.
    ///
    /// The root is known to the provider and arrives with the record
    /// (`ListEntry.product`); we cannot derive it from the key — the bucket
    /// layout is not ours. Empty — the provider stayed silent, and then the
    /// scene is the record itself.
    pub fn product_key(&self) -> &str {
        match self.product.is_empty() {
            false => bare(&self.product),
            true => self.snapshot_key(),
        }
    }

    /// Whether this is the row the navigation key speaks about. Compared with
    /// bare keys for the same reason they are bare: the slash is present in the
    /// catalogue and absent in search results, while the row is the same one.
    ///
    /// The product path is asked too: one navigates to the scene from anywhere
    /// by the storage key, while an own key exists only for a scene row with
    /// several packages, and only its own children know it.
    pub fn named(&self, key: &str) -> bool {
        !key.is_empty()
            && (bare(self.key()) == bare(key) || bare(&self.identifier) == bare(key))
    }

    /// Whether this is a scene — what has an outline, rasters, and its own
    /// unit of display. The kind answers this almost always; the exception is
    /// a downloaded file that is a scene to itself: as a library record it
    /// remains a file (see `downloaded_rows`).
    pub fn is_snapshot(&self) -> bool {
        self.kind.is_product()
            || (!self.product.is_empty() && self.product == self.snapshot_key())
    }

    /// The key by which the row is chosen with a checkbox. For a scene it is
    /// its provider key — the very one by which it is outlined on the globe —
    /// for anything else the row's own key.
    ///
    /// Written here once for everyone who asks about selection: the row's
    /// checkbox, the header's checkbox, and the selection handler. Two copies
    /// of this rule would one day count one row as two — checked and unchecked
    /// at once.
    pub fn choice_key(&self) -> &str {
        match self.is_snapshot() {
            true => self.snapshot_key(),
            false => self.key(),
        }
    }

    /// Whether this row can be chosen.
    ///
    /// A folder of the path — cannot: one enters it, while what is chosen lies
    /// in the catalogue or on disk. A scene lying as a directory (.SAFE,
    /// .SEN3) does not count as a folder in this sense — it is exactly what is
    /// chosen.
    ///
    /// A nameless one — cannot either: the choice key is what addresses the
    /// choice later, and nothing can be addressed by an empty one.
    pub fn choosable(&self) -> bool {
        !matches!(self.kind, RowKind::Folder) && !self.choice_key().is_empty()
    }

    /// Whether there is anything to expand under this row: the scene's
    /// packages, its files, or a folder's content. A catalogue folder's
    /// content has not arrived yet — the expansion is what asks for it.
    pub fn expandable(&self) -> bool {
        self.folded || self.group.is_some() || self.kind.is_folder()
    }

    /// The path of the folder the record lies in: rows are grouped by it, and
    /// it is shown in the row's menu.
    pub fn folder(&self) -> &str {
        folder_of(&self.identifier)
    }

    /// How many bytes of this record are already on disk: for a
    /// half-downloaded one — what is downloaded, for a finished one — the
    /// whole size.
    pub fn stored(&self) -> u64 {
        // A folded row has no bytes of its own: it is the sum of its files, and
        // asking for its own status would mean counting the scene as empty
        // exactly when part of it already lies on disk.
        if self.folded {
            return self.children.iter().map(Row::stored).sum();
        }
        match &self.status {
            RowStatus::Complete => self.size,
            RowStatus::Downloading { done, .. } | RowStatus::Paused { done, .. } => *done,
            RowStatus::Remote | RowStatus::Partial { .. } => 0,
        }
    }

    /// The "format" column: what the catalogue called the record, failing that
    /// — the record's kind, failing that — the extension in capitals. The type
    /// from the catalogue outranks everything else: a scene is signed with its
    /// type ("S2MSI2A"), and only the nameless gets a lowercase word — that is
    /// not an extension, hence the different casing.
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

    /// A row of something that contains other things: a folder of the path or
    /// a scene laid out as a directory. No size or time stands behind it — in
    /// S3 a folder is a shared key prefix, and it has neither; for a scene the
    /// catalogue knows them, and the caller assigns them (see
    /// `components::rows::from_key`).
    pub fn container_row(identifier: String, title: String, status: RowStatus, kind: RowKind) -> Row {
        Row { identifier, title, kind, status, ..Default::default() }
    }

    /// A row from a library record — the "Downloaded" view.
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
            // The record name is the path inside the scene, and the whole of
            // it is not needed in the row: the scene is already named by the
            // row above, and under it one reads the file. The key stays the
            // whole name (the `name` field) — it addresses the record.
            title: last_segment(&entry.name).to_string(),
            kind: RowKind::File,
            size: if entry.total > 0 { entry.total } else { entry.done },
            date: entry.modified,
            // The scene travels onward with the record: the download
            // continuation goes as the same message, and a scene lost here
            // would erase the belonging in the seedcard — the file would fall
            // out of its scene silently.
            product: entry.product.clone(),
            status,
            ..Default::default()
        }
    }

    /// A row of a provider catalogue object: the state is pulled from the
    /// library if a record is already there.
    ///
    /// The kind arrives as a parameter, not derived here: the storage layout
    /// is what makes an object a scene, and only the provider knows it (see
    /// `s3::product_root`).
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
                // The catalogue's size and time are more precise: the library
                // knows only what has made it to disk.
                size: if size > 0 { size } else { entry.total.max(entry.done) },
                date: if date > 0 { date } else { entry.modified },
                ..Row::from_entry(entry)
            },
            // There is no record in the library — so the record lies in
            // storage, and that is exactly what the row answers by default.
            None => Row { identifier, title, kind, size, date, ..Default::default() },
        }
    }
}

/// All rows of the "Downloaded" view: files of one scene are gathered into a
/// single row, while what does not belong to a scene stays on its own.
///
/// The folding is here, not in the library: it keeps track of files — that is
/// its subject, and one file is one record there — while "a scene" is what it
/// is shown as. The library meanwhile keeps and hands out the knowledge of
/// which scene a file belongs to (`LibraryEntry.product`): it could not derive
/// it from the name.
pub fn downloaded_rows(library: &LibraryState) -> Vec<Row> {
    let mut products: Vec<(String, Vec<Row>)> = Vec::new();
    let mut alone: Vec<Row> = Vec::new();

    for entry in &library.entries {
        let row = Row::from_entry(entry);
        if entry.product.is_empty() {
            alone.push(row);
            continue;
        }
        // The order of scenes is the order of the first record encountered:
        // the library hands them out in its own order already, and the display
        // will have to re-sort them anyway (see `arrange`).
        match products.iter_mut().find(|(key, _)| key == &entry.product) {
            Some((_, files)) => files.push(row),
            None => products.push((entry.product.clone(), vec![row])),
        }
    }

    products
        .into_iter()
        .map(|(key, mut files)| match files.len() == 1 && files[0].identifier == key {
            // A scene of a single file is exactly that file: a wrapper around
            // it would carry the same row key, and the list would match widget
            // state between it and its own child.
            true => files.remove(0),
            // The scene's composition is asked of the library, not summed up
            // here: the "how many files are in a scene" rule is one for
            // everyone, and a second carrier of it would one day diverge from
            // the first (see `LibraryState::snapshot`).
            false => {
                let (_, siblings) = library.snapshot(&key);
                snapshot(key, siblings, files)
            }
        })
        .chain(alone)
        .collect()
}

/// A reason common to all stopped files of a scene. Empty — their reasons
/// differ or there is none at all.
///
/// It is needed because the scene arrives folded, and in the most common case
/// — the signature failed, and with it all two dozen files at once — the person
/// sees only this row. It cannot name one reason out of several different
/// ones: the row speaks for all files at once, and naming one would lie about
/// the rest.
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

/// A scene row on top of its files: what can be said about the scene is folded
/// from what is known about each file — with one exception. How many files the
/// scene has in total (`siblings`) is not derivable from the records: there are
/// as many as were downloaded.
fn snapshot(product: String, siblings: u32, files: Vec<Row>) -> Row {
    let done = files.iter().filter(|file| matches!(file.status, RowStatus::Complete)).count();
    let downloading = files.iter().any(|file| matches!(file.status, RowStatus::Downloading { .. }));
    let size = files.iter().map(Row::stored).sum();
    // The scene's time is the time of its freshest file: the scene "appeared
    // on disk" when the last of them landed.
    let date = files.iter().map(|file| file.date).max().unwrap_or(0);

    let status = match (downloading, done, files.len()) {
        // While any file is arriving, the whole scene is arriving: the sum of
        // bytes over parts would be true about the files, not about it.
        (true, ..) => RowStatus::Downloading { done: size, total: 0 },
        // "On disk" only when it is known how many files the scene consists of
        // and all of them are here and finished. Without that number every
        // scene with its downloaded part finished would look finished — three
        // files out of twenty-six would read as a whole scene.
        (false, done, total) if siblings > 0 && done == total && total as u32 == siblings => {
            RowStatus::Complete
        }
        // Nothing finished — the scene is interrupted, not "partially
        // downloaded": a green row with zero on disk promises what does not
        // exist, and passes the "on disk" filter together with the whole ones.
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
