use std::collections::HashMap;
pub use veldmap_data::TileId;

pub struct TileManager {
    pub loaded_tiles: HashMap<TileId, usize>,
    free_slots: Vec<usize>,
    pub max_tiles: usize,
    pub indirection_data: Vec<u8>, 
}

impl TileManager {
    pub fn new(max_tiles: usize) -> Self {
        Self {
            loaded_tiles: HashMap::new(),
            free_slots: (0..max_tiles).rev().collect(),
            max_tiles,
            indirection_data: vec![255; 128 * 64],
        }
    }

    pub fn update_indirection(&mut self) {
        self.indirection_data.fill(255);
        for (id, &slot) in &self.loaded_tiles {
            if id.z == 0 {
                 self.indirection_data.fill(slot as u8);
            }
        }
    }

    pub fn assign_slot(&mut self, id: TileId) -> Option<usize> {
        if let Some(&slot) = self.loaded_tiles.get(&id) {
            return Some(slot);
        }
        if let Some(slot) = self.free_slots.pop() {
            self.loaded_tiles.insert(id, slot);
            return Some(slot);
        }
        None
    }
}

pub fn get_visible_tiles(lat: f64, lon: f64, altitude: f64) -> Vec<TileId> {
    let z = if altitude > 5_000_000.0 { 0 }
            else if altitude > 1_000_000.0 { 1 }
            else { 2 };
    
    let n = 2u32.pow(z);
    let x = ((lon + 180.0) / 360.0 * n as f64) as u32;
    let y = ((90.0 - lat) / 180.0 * n as f64) as u32;
    
    let mut tiles = Vec::new();
    for dx in -1..=1 {
        for dy in -1..=1 {
            let tx = (x as i32 + dx).max(0).min(n as i32 - 1) as u32;
            let ty = (y as i32 + dy).max(0).min(n as i32 - 1) as u32;
            tiles.push(TileId { z, x: tx, y: ty });
        }
    }
    tiles
}
