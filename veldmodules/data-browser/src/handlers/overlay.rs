use crate::module::footprint;
use crate::module::state::{
    overlay::Assembly, overlay::OverlayState, overlay::Part, overlay::Progress, Locate, Located,
    Open, Shift,
    State, ViewId,
};
use crate::proto::data_provider::{
    DataProduct, ImageryRequest, ImageryResponse, LocateRequest, LocateResponse,
};
use crate::proto::globe::{
    GeoPoint, Overlay, OverlayRaster, OverlayRole, Overlays, UtmFrame,
};
use veldsdk::proto::core::ResourceOpened;

const WHOLE_EARTH_DEG: f64 = 350.0;

fn role_for_globe(role: crate::proto::data_provider::ImageryRole) -> OverlayRole {
    use crate::proto::data_provider::ImageryRole;
    match role {
        ImageryRole::ImageryPreview => OverlayRole::OverlayPreview,
        ImageryRole::ImageryDetailed => OverlayRole::OverlayDetailed,
    }
}

fn file_name(identifier: &str) -> &str {
    identifier.trim_end_matches('/').rsplit('/').next().unwrap_or_default()
}

pub fn on_toggle_pressed(state: &mut State, view: ViewId, identifier: String) {
    state.close_menus();
    state.unshowable.remove(&identifier);
    if state.overlays.iter().any(|overlay| overlay.identifier == identifier) {
        remove(state, &identifier);
        return;
    }
    if state.showing.remove(&identifier) {
        veldsdk::log::info!(target: "handlers", "show of '{}' cancelled before the catalogue answered", identifier);
        state.file_wishes.remove(&identifier);
        send_set(state);
        return;
    }
    if super::search::show(state, view, &identifier) {
        super::nav::on_new_globe(state);
        return;
    }
    if !matches!(state.located.get(&identifier), Some(Located::Found(_))) {
        state.located.insert(identifier.clone(), Located::Asking);
    }
    state.showing.insert(identifier.clone());
    let correlation = state.locates.begin(Locate::Overlay(identifier.clone()));
    crate::calls::data_provider::on_locate(&LocateRequest { identifier }, &correlation);
    send_set(state);
    super::nav::on_new_globe(state);
}

pub fn on_located(state: &mut State, key: &str, response: LocateResponse) {
    if !state.showing.remove(key) {
        veldsdk::log::info!(target: "handlers", "show of '{}' became unwanted while the answer was in flight", key);
        return;
    }
    let Some(product) = response.product else {
        veldsdk::log::warn!(target: "handlers", "could not show on the globe: {}", response.error);
        state.notice = Some(format!("Could not show on the globe: {}", response.error));
        state.file_wishes.remove(key);
        send_set(state);
        return;
    };

    if key != product.identifier && !state.outlines.contains(key) {
        state.located.remove(key);
    }
    state.located.insert(product.identifier.clone(), Located::Found(product.clone()));
    show(state, &product, None);
}

pub fn show(state: &mut State, product: &DataProduct, source: Option<ViewId>) {
    state.unshowable.remove(&product.identifier);
    if state.overlays.iter().any(|overlay| overlay.identifier == product.identifier) {
        send_set(state);
        return;
    }

    let mut overlay = OverlayState::new(
        product.identifier.clone(),
        product.name.clone(),
        product.folder,
        source,
        quad_of(product),
        footprint::frame(&product.footprint),
    );
    overlay.wanted_file = state.file_wishes.remove(&product.identifier);
    state.overlays.push(overlay);

    request_imagery(state, &product.identifier);
    send_set(state);
}

fn request_imagery(state: &mut State, key: &str) {
    let correlation = state.imageries.begin(key.to_string());
    let Some(overlay) = state.overlays.iter_mut().find(|o| o.identifier == key) else { return };
    overlay.imagery = Some(correlation.clone());
    let request = imagery_request(&state.library, overlay);
    crate::calls::data_provider::on_imagery(&request, &correlation);
}

fn imagery_request(library: &crate::module::state::LibraryState, overlay: &OverlayState) -> ImageryRequest {
    ImageryRequest {
        downloaded: library.whole(&overlay.identifier),
        identifier: overlay.identifier.clone(),
        wanted: overlay.wanted_file.clone().unwrap_or_default(),
    }
}

pub fn on_file_pressed(state: &mut State, product: &str, file: String) {
    state.close_menus();
    place_file(state, product, file);
}

fn place_file(state: &mut State, product: &str, file: String) {
    state.unshowable.remove(product);
    if let Some(overlay) = state.overlays.iter_mut().find(|o| o.identifier == product) {
        if overlay.wanted_file.as_deref() == Some(file.as_str()) {
            return;
        }
        if overlay.imagery.is_some() || overlay.assembly.is_some() {
            veldsdk::log::info!(target: "handlers", "'{}': rasters are still being assembled — file {} will wait", overlay.label, file_name(&file));
            state.file_wishes.insert(product.to_string(), file);
            return;
        }
        overlay.wanted_file = Some(file);
        overlay.variable_wanted = None;
        request_imagery(state, product);
        return;
    }
    state.file_wishes.insert(product.to_string(), file);
    if state.showing.contains(product) {
        return;
    }
    let found = match state.located.get(product) {
        Some(Located::Found(found)) => Some(found.clone()),
        _ => None,
    };
    if let Some(found) = found {
        show(state, &found, None);
        super::nav::on_new_globe(state);
        return;
    }
    state.located.insert(product.to_string(), Located::Asking);
    state.showing.insert(product.to_string());
    let correlation = state.locates.begin(Locate::Overlay(product.to_string()));
    crate::calls::data_provider::on_locate(&LocateRequest { identifier: product.to_string() }, &correlation);
    send_set(state);
    super::nav::on_new_globe(state);
}

pub fn remove(state: &mut State, key: &str) {
    let Some(index) = state.overlays.iter().position(|overlay| overlay.identifier == key) else {
        return;
    };
    let overlay = state.overlays.remove(index);
    abandon(state, overlay);
    super::outline::forget_gone(state);
    send_set(state);
}

pub fn shift(state: &mut State, key: &str, shift: Shift) {
    let Some(index) = state.overlays.iter().position(|overlay| overlay.identifier == key) else {
        return;
    };
    let target = match shift {
        Shift::Up if index + 1 < state.overlays.len() => index + 1,
        Shift::Down if index > 0 => index - 1,
        _ => return,
    };
    state.overlays.swap(index, target);
    send_set(state);
}

pub fn clear_all(state: &mut State) {
    state.showing.clear();
    for overlay in std::mem::take(&mut state.overlays) {
        abandon(state, overlay);
    }
    super::outline::forget_gone(state);
    send_set(state);
}

fn abandon(state: &mut State, overlay: OverlayState) {
    if let Some(correlation) = &overlay.imagery {
        state.imageries.take(correlation);
    }
    state.file_wishes.remove(&overlay.identifier);
    let Some(assembly) = overlay.assembly else { return };
    for correlation in &assembly.opens {
        if let Some((_, role, ordinal, part)) = state.opens.take(correlation) {
            state.opens.insert(correlation.clone(), (String::new(), role, ordinal, part));
        }
    }
    for (.., handle) in assembly.collected {
        veldsdk::resource::release(handle);
    }
}

pub fn keep_only(state: &mut State, view: ViewId, alive: impl Fn(&str) -> bool) {
    retain(state, |overlay| {
        overlay.source != Some(view) || alive(&overlay.identifier)
    });
}

pub fn source_closed(state: &mut State, view: ViewId) {
    retain(state, |overlay| overlay.source != Some(view));
}

fn retain(state: &mut State, keep: impl Fn(&OverlayState) -> bool) {
    let (kept, gone): (Vec<_>, Vec<_>) =
        std::mem::take(&mut state.overlays).into_iter().partition(&keep);
    state.overlays = kept;
    let said = match gone.as_slice() {
        [] => None,
        [one] => Some(format!(
            "\"{}\" removed from the globe: it is no longer in the results",
            crate::module::components::format::ellipsize(&one.label, 40)
        )),
        many => Some(format!(
            "{} {} removed from the globe: they are no longer in the results",
            many.len(),
            crate::module::components::format::plural(
                many.len(),
                ["snapshot", "snapshots", "snapshots"]
            )
        )),
    };
    for overlay in gone {
        abandon(state, overlay);
    }
    if let Some(said) = said {
        state.notice = Some(said);
    }
    super::outline::forget_gone(state);
    send_set(state);
}

pub fn set_opacity(state: &mut State, key: &str, opacity: f32) {
    let Some(overlay) = state.overlays.iter_mut().find(|o| o.identifier == key) else { return };
    overlay.opacity = opacity.clamp(0.0, 1.0);
    send_set(state);
}

pub fn set_hidden(state: &mut State, key: &str, hidden: bool) {
    let Some(overlay) = state.overlays.iter_mut().find(|o| o.identifier == key) else { return };
    overlay.hidden = hidden;
    send_set(state);
}

pub fn menu(state: &mut State, key: Option<String>) {
    match key {
        Some(key) => state.toggle_open(Open::Layer(key)),
        None => state.close_menus(),
    }
}

pub fn hide_all(state: &mut State, hidden: bool) {
    for overlay in &mut state.overlays {
        overlay.hidden = hidden;
    }
    send_set(state);
}

fn give_up(state: &mut State, key: &str, label: &str, why: String) {
    veldsdk::log::warn!(target: "handlers", "'{}': {}", label, why);
    state.notice =
        Some(format!("\"{}\": {}", crate::module::components::format::ellipsize(label, 40), why));
    state.unshowable.insert(key.to_string(), why);
    remove(state, key);
}

pub fn on_imagery_result(state: &mut State, response: ImageryResponse) {
    let Some(key) = state.imageries.take(&veldsdk::correlation()) else { return };
    let Some(overlay) = state.overlays.iter().find(|o| o.identifier == key) else { return };
    let label = overlay.label.clone();
    if overlay.assembly.is_some() {
        veldsdk::log::warn!(target: "handlers", "'{}': rasters are already being assembled", label);
        return;
    }

    if !response.error.is_empty() {
        return give_up(state, &key, &label, format!("rasters could not be requested: {}", response.error));
    }
    if !response.wanted_refused.is_empty() {
        state.notice = Some(format!("«{}»: {}", crate::module::components::format::ellipsize(&label, 40), response.wanted_refused));
        if let Some(named) = state.overlays.iter_mut().find(|o| o.identifier == key) {
            named.wanted_file = None;
        }
    }
    let Some(overlay) = state.overlays.iter().find(|o| o.identifier == key) else { return };
    if response.rasters.is_empty() {
        let why = match response.reason.is_empty() {
            true => "no rasters to overlay".to_string(),
            false => response.reason,
        };
        return give_up(state, &key, &label, why);
    }
    if response.utm.is_none() && overlay.quad.is_none() {
        return give_up(state, &key, &label, "no binding: the product has no geometry".to_string());
    }

    let utm = response.utm.map(|utm| UtmFrame {
        zone: utm.zone,
        south: utm.south,
        x0: utm.x0,
        y0: utm.y0,
        x1: utm.x1,
        y1: utm.y1,
    });
    let mut opens = Vec::new();
    let files: Vec<String> = response.rasters.iter().map(|raster| raster.identifier.clone()).collect();
    for (ordinal, raster) in (0u32..).zip(response.rasters) {
        let role = role_for_globe(raster.role());
        let files = [(Part::Raster, raster.identifier)]
            .into_iter()
            .chain((!raster.geolocation.is_empty()).then_some((
                Part::Geolocation,
                raster.geolocation,
            )));
        for (part, identifier) in files {
            let local = state.library.local_name(&identifier).map(str::to_string);
            let correlation = state.opens.begin((key.clone(), role, ordinal, part));
            opens.push(correlation.clone());
            super::open_resource(identifier, local, &correlation);
        }
    }

    let Some(overlay) = state.overlays.iter_mut().find(|o| o.identifier == key) else { return };
    overlay.imagery = None;
    overlay.assembly = Some(Assembly { utm, files, opens, collected: Vec::new() });
}

pub fn on_opened(state: &mut State, opened: &ResourceOpened) -> bool {
    let correlation = veldsdk::correlation();
    let Some((key, role, ordinal, part)) = state.opens.take(&correlation) else { return false };

    let Some(overlay) = state.overlays.iter_mut().find(|o| o.identifier == key) else {
        if let Ok(handle) = veldsdk::resource::accept(opened) {
            veldsdk::resource::release(handle);
        }
        return true;
    };
    let Some(assembly) = overlay.assembly.as_mut() else {
        if let Ok(handle) = veldsdk::resource::accept(opened) {
            veldsdk::resource::release(handle);
        }
        return true;
    };

    assembly.opens.retain(|waiting| waiting != &correlation);
    match veldsdk::resource::accept(opened) {
        Ok(handle) => assembly.collected.push((role, ordinal, part, handle)),
        Err(error) => veldsdk::log::warn!(target: "handlers", "overlay file: {}", error),
    }
    if assembly.opens.is_empty() {
        finish(state, &key);
    }
    true
}

fn paired<H>(
    collected: Vec<(OverlayRole, u32, Part, H)>,
) -> (Vec<(OverlayRole, u32, H, Option<H>)>, Vec<H>) {
    let mut pairs: Vec<(OverlayRole, u32, H, Option<H>)> = Vec::new();
    let mut coordinates = Vec::new();
    for (role, ordinal, part, handle) in collected {
        match part {
            Part::Raster => pairs.push((role, ordinal, handle, None)),
            Part::Geolocation => coordinates.push((ordinal, handle)),
        }
    }
    pairs.sort_by_key(|(role, ordinal, ..)| (*role as i32, *ordinal));

    let mut orphans = Vec::new();
    for (ordinal, handle) in coordinates {
        match pairs.iter_mut().find(|(_, other, ..)| *other == ordinal) {
            Some(pair) => pair.3 = Some(handle),
            None => orphans.push(handle),
        }
    }
    (pairs, orphans)
}

fn finish(state: &mut State, key: &str) {
    let Some(overlay) = state.overlays.iter_mut().find(|o| o.identifier == key) else { return };
    let Some(assembly) = overlay.assembly.take() else { return };
    let label = overlay.label.clone();

    let (pairs, orphans) = paired(assembly.collected);
    for handle in orphans {
        veldsdk::log::warn!(target: "handlers", "'{}': coordinates without a raster — releasing", label);
        veldsdk::resource::release(handle);
    }

    let mut rasters = Vec::new();
    for (role, ordinal, raster, coordinates) in pairs {
        let raster = match veldsdk::resource::hand_off(raster, "globe") {
            Ok(handle) => handle,
            Err(error) => {
                veldsdk::log::warn!(target: "handlers", "the raster was not handed off to the globe: {}", error);
                if let Some(handle) = coordinates {
                    veldsdk::resource::release(handle);
                }
                continue;
            }
        };
        let coordinates = coordinates.and_then(|handle| {
            match veldsdk::resource::hand_off(handle, "globe") {
                Ok(handle) => Some(handle),
                Err(error) => {
                    veldsdk::log::warn!(target: "handlers",
                        "the coordinates were not handed off to the globe: {}", error);
                    None
                }
            }
        });
        rasters.push(OverlayRaster {
            resource: Some(raster),
            role: role as i32,
            geolocation: coordinates,
            ordinal,
            variable: String::new(),
        });
    }
    if rasters.is_empty() {
        return give_up(state, &key, &label, "not a single raster opened".to_string());
    }

    overlay.utm = assembly.utm;
    overlay.rasters = rasters;
    overlay.files = assembly.files;
    overlay.progress = Progress { working: true, blank: true, ..Progress::default() };
    send_set(state);
    if let Some(file) = state.file_wishes.remove(key) {
        place_file(state, key, file);
    }
}

fn send_set(state: &State) {
    let overlays = state
        .overlays
        .iter()
        .filter(|overlay| overlay.on_globe())
        .map(|overlay| {
            let (quad, rough) = match (overlay.utm.is_some(), overlay.quad) {
                (false, Some(points)) => (
                    points.map(|(lat, lon)| GeoPoint { lat, lon }).to_vec(),
                    overlay.rough,
                ),
                _ => (Vec::new(), false),
            };
            Overlay {
                key: overlay.identifier.clone(),
                label: overlay.label.clone(),
                utm: overlay.utm.clone(),
                quad,
                rough,
                rasters: with_variable(&overlay.rasters, overlay.variable_wanted.as_ref()),
                opacity: Some(overlay.opacity),
                hidden: overlay.hidden,
            }
        })
        .collect();

    crate::calls::globe::on_overlay(&Overlays { overlays });
    super::outline::send(state);
}

fn with_variable(rasters: &[OverlayRaster], wanted: Option<&(u32, String)>) -> Vec<OverlayRaster> {
    rasters
        .iter()
        .map(|raster| OverlayRaster {
            variable: match wanted {
                Some((ordinal, path)) if *ordinal == raster.ordinal => path.clone(),
                _ => String::new(),
            },
            ..raster.clone()
        })
        .collect()
}

pub fn variables_menu(state: &mut State, key: Option<String>) {
    match key {
        Some(key) => state.toggle_open(Open::LayerVariables(key)),
        None => state.close_menus(),
    }
}

pub fn variable(state: &mut State, key: &str, variable: String) {
    state.close_menus();
    let Some(overlay) = state.overlays.iter_mut().find(|o| o.identifier == key) else { return };
    if overlay.variable.as_ref().is_some_and(|shown| shown.path == variable) {
        return;
    }
    let Some(ordinal) = overlay.detailed_ordinal else { return };
    if overlay.variable_wanted.as_ref() == Some(&(ordinal, variable.clone())) {
        return;
    }
    overlay.variable_wanted = Some((ordinal, variable));
    send_set(state);
}

pub fn on_overlay_progress(state: &mut State, msg: crate::proto::globe::OverlaysProgress) {
    for overlay in &mut state.overlays {
        let Some(said) = msg.overlays.iter().find(|progress| progress.key == overlay.identifier)
        else {
            continue;
        };
        overlay.trouble = said.trouble.clone();
        overlay.detailed_trouble = said.detailed_trouble.clone();
        overlay.variable = said.detailed_variable.clone();
        overlay.detailed_ordinal = said.detailed;
        if !said.detailed_variables.is_empty() {
            overlay.variables = said.detailed_variables.clone();
        }
        if overlay.variable_wanted.as_ref().is_some_and(|(ordinal, _)| said.detailed != Some(*ordinal)) {
            overlay.variable_wanted = None;
        }
        overlay.detailed = said
            .detailed
            .and_then(|at| overlay.files.get(at as usize))
            .map(|identifier| file_name(identifier).to_string());
        overlay.progress = Progress {
            ready: said.ready,
            total: said.total,
            share: said.share,
            working: said.working,
            step: said.step,
            steps: said.steps,
            blank: said.blank,
            pass: (said.pass_read, said.pass_total),
        };
    }

    let doomed: Vec<(String, String)> = msg
        .overlays
        .iter()
        .filter(|progress| !progress.error.is_empty())
        .map(|progress| (progress.key.clone(), progress.error.clone()))
        .collect();
    for (key, why) in doomed {
        let Some(overlay) = state.overlays.iter().find(|o| o.identifier == key) else { continue };
        let label = overlay.label.clone();
        give_up(state, &key, &label, why);
    }

    super::outline::send(state);
}

fn quad_of(product: &DataProduct) -> Option<([(f64, f64); 4], bool)> {
    if product.footprint.len() == 1
        && let Some(ring) = product.footprint.first()
    {
        let points = match ring.points.as_slice() {
            [first, .., last] if first.lat == last.lat && first.lon == last.lon => {
                &ring.points[..ring.points.len() - 1]
            }
            all => all,
        };
        if let [a, b, c, d] = points {
            let lons = [a.lon, b.lon, c.lon, d.lon];
            let span = lons.iter().fold(f64::MIN, |wide, lon| wide.max(*lon))
                - lons.iter().fold(f64::MAX, |narrow, lon| narrow.min(*lon));
            if span < WHOLE_EARTH_DEG {
                return Some((
                    oriented([(a.lat, a.lon), (b.lat, b.lon), (c.lat, c.lon), (d.lat, d.lon)]),
                    false,
                ));
            }
        }
    }
    let circle = footprint::frame(&product.footprint)?;
    let (north, south) = (
        (circle.lat + circle.radius_deg).min(90.0),
        (circle.lat - circle.radius_deg).max(-90.0),
    );
    let (west, east) = (circle.lon - circle.radius_deg, circle.lon + circle.radius_deg);
    Some(([(north, west), (north, east), (south, east), (south, west)], true))
}

fn oriented(corners: [(f64, f64); 4]) -> [(f64, f64); 4] {
    let base = corners[0].1;
    let unwound = |lon: f64| base + (lon - base + 540.0).rem_euclid(360.0) - 180.0;
    let (lat_mid, lon_mid) = corners
        .iter()
        .fold((0.0, 0.0), |(lat, lon), corner| (lat + corner.0 / 4.0, lon + unwound(corner.1) / 4.0));
    let quarter = |north: bool, west: bool| -> Vec<(f64, f64)> {
        corners
            .iter()
            .copied()
            .filter(|&(lat, lon)| (lat > lat_mid) == north && (unwound(lon) < lon_mid) == west)
            .collect()
    };
    match (
        quarter(true, true).as_slice(),
        quarter(true, false).as_slice(),
        quarter(false, false).as_slice(),
        quarter(false, true).as_slice(),
    ) {
        ([ul], [ur], [lr], [ll]) => [*ul, *ur, *lr, *ll],
        _ => corners,
    }
}
