//! Product manifest: what is a measurement in it is declared by the manifest
//! itself.
//!
//! Every `.SAFE` and `.SEN3` has an XFDU manifest in its root — `manifest.safe`
//! or `xfdumanifest.xml`. This is not metadata about the acquisition but an
//! inventory of the package itself: the content map names the data units with
//! their kind, and the object section names the files that record those units.
//! A unit of kind "Measurement Data Unit" is the measurement; everything else
//! is geometry, coordinates, quality flags, tables.
//!
//! ```xml
//! <xfdu:contentUnit ID="NTC_AOD_Unit" unitType="Measurement Data Unit" …>
//!   <dataObjectPointer dataObjectID="NTC_AOD_Data"/>
//! …
//! <dataObject ID="NTC_AOD_Data">
//!   <byteStream mimeType="application/x-netcdf" size="35064511">
//!     <fileLocation locatorType="URL" href="NTC_AOD.nc"/>
//! ```
//!
//! Without a manifest one has to choose by file names, and this guessing fails
//! where a failure costs most: for an SLSTR granule `LST_ancillary_ds.nc`
//! sorts earlier in the alphabet than `LST_in.nc`, and for Sentinel-1 OCN the
//! very first is `preview/icons/logo.png`. The manifest settles the question
//! completely and costs one request: for Sentinel-1 and Sentinel-2 it is
//! 25–70 KB, for Sentinel-3 granules hundreds of kilobytes, and for
//! full-resolution OLCI the manifest reaches one and a half megabytes.

use std::collections::{HashMap, HashSet};

use quick_xml::events::Event;
use quick_xml::Reader;

/// The manifest names. There are two because there are two packagings:
/// `.SAFE` for Sentinel-1 and Sentinel-2, `.SEN3` for Sentinel-3. Inside —
/// the very same XFDU.
const NAMES: [&str; 2] = ["manifest.safe", "xfdumanifest.xml"];

/// An opening tag or some other. Only one place needs this — unit nesting —
/// and `Event` does not report it about itself: `Start` and `Empty` carry the
/// very same `BytesStart`.
enum Kind {
    Start,
    Other,
}

/// The manifest in the product root — the single key whose last segment is one
/// of [`NAMES`], with exactly the product path before it.
///
/// Exactly in the root: the manifest describes the whole product, and one
/// found deeper is already someone else's (a product inside a product occurs
/// in assembled deliveries), and it would describe a part instead of the
/// whole.
pub fn key<'a>(identifier: &str, keys: &'a [String]) -> Option<&'a String> {
    let root = format!("{}/", identifier.trim_end_matches('/'));
    keys.iter().find(|key| {
        NAMES.iter().any(|name| {
            key.len() == root.len() + name.len() && key.starts_with(&root) && key.ends_with(name)
        })
    })
}

/// The files that record the measurement — as paths relative to the product
/// root, in manifest order.
///
/// Empty — the manifest did not parse or named no measurement unit. This is
/// not an error: the answer "I don't know" here means exactly what held before
/// the manifest, and selection falls back to file names.
pub fn measurements(body: &[u8]) -> Vec<String> {
    let mut reader = Reader::from_reader(body);
    reader.config_mut().trim_text(true);

    // Units are nested one inside another ("Information Package" → the unit
    // itself), and the kind is declared on the outer one exactly as on the
    // inner one. Therefore a pointer counts as a measurement if a measurement
    // kind is declared on any ancestor: the stack stores how many are open
    // right now.
    let mut depth: Vec<bool> = Vec::new();
    let mut measured: HashSet<String> = HashSet::new();
    let mut order: Vec<String> = Vec::new();
    let mut files: HashMap<String, String> = HashMap::new();
    let mut object = String::new();
    let mut buf = Vec::new();
    // Whether the manifest was read through to the closing of the root — see
    // below.
    let mut whole = false;

    loop {
        let event = match reader.read_event_into(&mut buf) {
            Ok(event) => event,
            // Truncated or foreign XML — the same "I don't know": half of the
            // manifest would name half of the measurements, and that is worse
            // than naming nothing.
            Err(_) => return Vec::new(),
        };
        let event_kind = match event {
            Event::Start(_) => Kind::Start,
            _ => Kind::Other,
        };
        match event {
            // An opening and a self-closing tag read the same, and one thing
            // distinguishes them: a self-closing one has no children and does
            // not push onto the nesting stack.
            Event::Start(tag) | Event::Empty(tag) => {
                let opens = matches!(event_kind, Kind::Start);
                match tag.local_name().as_ref() {
                    b"contentUnit" if opens => {
                        depth.push(is_measurement(&attribute(&tag, b"unitType")))
                    }
                    b"dataObjectPointer" if depth.iter().any(|unit| *unit) => {
                        let id = attribute(&tag, b"dataObjectID");
                        if !id.is_empty() && measured.insert(id.clone()) {
                            order.push(id);
                        }
                    }
                    b"dataObject" if opens => object = attribute(&tag, b"ID"),
                    b"fileLocation" if !object.is_empty() => {
                        files.insert(object.clone(), relative(&attribute(&tag, b"href")));
                    }
                    _ => {}
                }
            }
            Event::End(tag) => match tag.local_name().as_ref() {
                b"contentUnit" => {
                    depth.pop();
                }
                b"dataObject" => object.clear(),
                // The root has closed — the manifest is read to the end, and
                // whatever it named is named completely.
                b"XFDU" => whole = true,
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }

    // A body truncated at an element boundary does not look like a parse
    // error: `read_event` reaches `Eof` and stays silent. Half of the manifest
    // would name half of the measurements — and that is worse than naming
    // nothing: the missing measurement would be chosen by alphabet, silently.
    if !whole {
        return Vec::new();
    }
    order.into_iter().filter_map(|id| files.remove(&id)).filter(|href| !href.is_empty()).collect()
}

/// The unit kind declared as a measurement. Matching by the word
/// "measurement": in XFDU stand side by side "Measurement Data Unit" for
/// Sentinel-3 and the same kind with its own `repID` for Sentinel-1, while
/// other kinds — "Annotation Data Unit", "Metadata Unit", "Information
/// Package" — do not contain this word.
fn is_measurement(unit_type: &str) -> bool {
    unit_type.to_ascii_lowercase().contains("measurement")
}

/// Attribute value by name; empty — there is no such attribute.
fn attribute(tag: &quick_xml::events::BytesStart<'_>, name: &[u8]) -> String {
    tag.attributes()
        .flatten()
        .find(|attribute| attribute.key.local_name().as_ref() == name)
        .map(|attribute| String::from_utf8_lossy(&attribute.value).to_string())
        .unwrap_or_default()
}

/// An `href` path into a path from the product root: `./measurement/x.tiff`
/// and `measurement/x.tiff` are the same file, and a leading slash in the
/// manifest means the same, not the bucket root.
fn relative(href: &str) -> String {
    href.trim_start_matches("./").trim_start_matches('/').to_string()
}
