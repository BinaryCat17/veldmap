use veldsdk::proto::core::ResourceHandle;

use crate::proto::globe::{OverlayRaster, UtmFrame, Variable};
use super::ViewId;

pub struct OverlayState {
    pub identifier: String,
    pub label: String,
    pub folder: bool,
    pub source: Option<ViewId>,
    pub quad: Option<[(f64, f64); 4]>,
    pub rough: bool,
    pub focus: Option<crate::module::footprint::Frame>,
    pub opacity: f32,
    pub hidden: bool,
    pub imagery: Option<String>,
    pub assembly: Option<Assembly>,
    pub progress: Progress,
    pub trouble: String,
    pub files: Vec<String>,
    pub detailed: Option<String>,
    pub detailed_trouble: String,
    pub variable: Option<Variable>,
    pub variables: Vec<Variable>,
    pub detailed_ordinal: Option<u32>,
    pub wanted_file: Option<String>,
    pub variable_wanted: Option<(u32, String)>,
    pub rasters: Vec<OverlayRaster>,
    pub utm: Option<UtmFrame>,
}

impl OverlayState {
    pub fn new(
        identifier: String,
        label: String,
        folder: bool,
        source: Option<ViewId>,
        quad: Option<([(f64, f64); 4], bool)>,
        focus: Option<crate::module::footprint::Frame>,
    ) -> Self {
        Self {
            identifier,
            label,
            folder,
            source,
            quad: quad.map(|(points, _)| points),
            rough: quad.is_some_and(|(_, rough)| rough),
            focus,
            opacity: 1.0,
            hidden: false,
            imagery: None,
            assembly: None,
            progress: Progress::default(),
            trouble: String::new(),
            files: Vec::new(),
            detailed: None,
            detailed_trouble: String::new(),
            variable: None,
            variables: Vec::new(),
            detailed_ordinal: None,
            wanted_file: None,
            variable_wanted: None,
            rasters: Vec::new(),
            utm: None,
        }
    }

    pub fn on_globe(&self) -> bool {
        !self.rasters.is_empty()
    }
}

#[derive(Clone, Copy, PartialEq, Default)]
pub struct Progress {
    pub ready: u32,
    pub total: u32,
    pub share: f32,
    pub working: bool,
    pub step: u32,
    pub steps: u32,
    pub blank: bool,
    pub pass: (u64, u64),
}

#[derive(Clone, Copy, PartialEq)]
pub enum Pace {
    Idle,
    Unknown,
    Share(f32),
}

impl Progress {
    pub fn parts(&self) -> Vec<String> {
        if !self.working {
            return Vec::new();
        }
        let mut said = Vec::new();
        if self.steps > 0 {
            said.push(format!("step {}/{}", self.step + 1, self.steps));
        }
        if self.pass.1 > 0 {
            said.push(crate::module::components::format::progress(self.pass.0, self.pass.1));
        }
        said.push(match self.total {
            0 => "preparing…".to_string(),
            total => format!("tiles {}/{}", self.ready, total),
        });
        said
    }

    pub fn said(&self) -> Option<String> {
        let parts = self.parts();
        match parts.is_empty() {
            true => None,
            false => Some(parts.join(" · ")),
        }
    }

    pub fn pace(&self) -> Pace {
        match (self.working, self.blank) {
            (false, _) => Pace::Idle,
            (true, true) => Pace::Unknown,
            (true, false) => Pace::Share(self.share),
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum Part {
    Raster,
    Geolocation,
}

pub struct Assembly {
    pub utm: Option<UtmFrame>,
    pub files: Vec<String>,
    pub opens: Vec<String>,
    pub collected: Vec<(crate::proto::globe::OverlayRole, u32, Part, ResourceHandle)>,
}
