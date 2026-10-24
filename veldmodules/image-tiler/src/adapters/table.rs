use super::super::budget::Peak;
use super::super::cascade;
use super::super::pyramid;
use super::grid::Overview;
use super::{frame_fits, jpeg, Info, Kind, Tie};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Serve {
    Pointwise,
    Pass { from: u32 },
}

#[derive(Clone, Debug)]
pub struct Level {
    pub serve: Serve,
    pub pixels: u64,
    pub peak: Peak,
    pub fits: bool,
}

impl Info {
    pub fn levels(&self) -> Vec<Level> {
        let base = (self.width, self.height);
        let whole = u64::from(self.width) * u64::from(self.height);
        let pass = |peak: Peak, fits: bool| Level { serve: Serve::Pass { from: 0 }, pixels: whole, fits, peak };
        (0..pyramid::level_count(self.width, self.height))
            .map(|level| {
                let lw = pyramid::level_size(self.width, level);
                let lh = pyramid::level_size(self.height, level);
                let chunked = |grid: &super::grid::Grid| match grid.footprint(base, level) {
                    Some(footprint) => {
                        let peak = grid.direct_peak(base, level).expect("pointwise level");
                        Level { serve: Serve::Pointwise, pixels: footprint.held, fits: peak.fits(), peak }
                    }
                    None => {
                        let peak = grid.pass_peak(base);
                        pass(peak.clone(), peak.fits())
                    }
                };
                match &self.kind {
                    Kind::Tiff(layout) => chunked(&layout.grid),
                    Kind::Jp2(layout) => chunked(&layout.grid),
                    Kind::Netcdf(layout) => chunked(&layout.grid),
                    Kind::Png { interlaced: false } => {
                        let peak = Peak::new()
                            .with("row", u64::from(self.width) * 4)
                            .with("cascade", cascade::bytes(self.width, self.height));
                        pass(peak.clone(), peak.fits())
                    }
                    Kind::Png { interlaced: true } | Kind::Full(_) => {
                        let peak = Peak::new()
                            .with("frame", whole * 4 * 2)
                            .with("cascade", cascade::bytes(self.width, self.height));
                        pass(peak.clone(), frame_fits(self.width, self.height) && peak.fits())
                    }
                    Kind::Jpeg => {
                        let (dw, dh) = jpeg::decoded_size(self.width, self.height, level);
                        let decoded = u64::from(dw) * u64::from(dh);
                        let peak = Peak::new()
                            .with("raw frame", decoded * 3)
                            .with("frame", decoded * 4)
                            .with("level", u64::from(lw) * u64::from(lh) * 4)
                            .with("cascade", cascade::bytes(lw, lh));
                        Level {
                            serve: Serve::Pass { from: level },
                            pixels: whole,
                            fits: frame_fits(dw, dh) && peak.fits(),
                            peak,
                        }
                    }
                }
            })
            .collect()
    }

    pub fn level(&self, level: u32) -> Option<Level> {
        self.levels().into_iter().nth(level as usize)
    }

    pub fn windowed(&self) -> u32 {
        self.levels().iter().take_while(|row| row.serve == Serve::Pointwise).count() as u32
    }

    pub fn finest(&self) -> u32 {
        let rows = self.levels();
        let top = rows.len().saturating_sub(1) as u32;
        rows.iter().position(|row| row.fits).map_or(top, |at| at as u32)
    }

    pub fn footprint(&self) -> u64 {
        let ties = (self.ties.len() * std::mem::size_of::<Tie>()) as u64;
        let kind = match &self.kind {
            Kind::Tiff(layout) => (layout.grid.overviews.len() * std::mem::size_of::<Overview>()) as u64,
            Kind::Jp2(layout) => (layout.grid.overviews.len() * std::mem::size_of::<Overview>()) as u64,
            Kind::Netcdf(_) | Kind::Png { .. } | Kind::Jpeg | Kind::Full(_) => 0,
        };
        ties + kind
    }
}
