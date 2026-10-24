use std::collections::{HashMap, HashSet};

use quick_xml::events::Event;
use quick_xml::Reader;

const NAMES: [&str; 2] = ["manifest.safe", "xfdumanifest.xml"];

enum Kind {
    Start,
    Other,
}

pub fn key<'a>(identifier: &str, keys: &'a [String]) -> Option<&'a String> {
    let root = format!("{}/", identifier.trim_end_matches('/'));
    keys.iter().find(|key| {
        NAMES.iter().any(|name| {
            key.len() == root.len() + name.len() && key.starts_with(&root) && key.ends_with(name)
        })
    })
}

pub fn measurements(body: &[u8]) -> Vec<String> {
    let mut reader = Reader::from_reader(body);
    reader.config_mut().trim_text(true);

    let mut depth: Vec<bool> = Vec::new();
    let mut measured: HashSet<String> = HashSet::new();
    let mut order: Vec<String> = Vec::new();
    let mut files: HashMap<String, String> = HashMap::new();
    let mut object = String::new();
    let mut buf = Vec::new();
    let mut whole = false;

    loop {
        let event = match reader.read_event_into(&mut buf) {
            Ok(event) => event,
            Err(_) => return Vec::new(),
        };
        let event_kind = match event {
            Event::Start(_) => Kind::Start,
            _ => Kind::Other,
        };
        match event {
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
                b"XFDU" => whole = true,
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }

    if !whole {
        return Vec::new();
    }
    order.into_iter().filter_map(|id| files.remove(&id)).filter(|href| !href.is_empty()).collect()
}

fn is_measurement(unit_type: &str) -> bool {
    unit_type.to_ascii_lowercase().contains("measurement")
}

fn attribute(tag: &quick_xml::events::BytesStart<'_>, name: &[u8]) -> String {
    tag.attributes()
        .flatten()
        .find(|attribute| attribute.key.local_name().as_ref() == name)
        .map(|attribute| String::from_utf8_lossy(&attribute.value).to_string())
        .unwrap_or_default()
}

fn relative(href: &str) -> String {
    href.trim_start_matches("./").trim_start_matches('/').to_string()
}
