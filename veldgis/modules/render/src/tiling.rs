use std::collections::HashMap;
use veldmap_gis_api::common::TileId;

pub struct TileManager {
    pub loaded_tiles: HashMap<TileId, usize>,
    free_slots: Vec<usize>,
    pub indirection_data: Vec<u8>, 
}

impl TileManager {
    pub fn new(max_tiles: usize) -> Self {
        Self {
            loaded_tiles: HashMap::new(),
            free_slots: (0..max_tiles).rev().collect(),
            indirection_data: vec![255; 64 * 32 * 2],
        }
    }

    pub fn update_indirection(&mut self) {
        self.indirection_data.fill(255);
        
        // Find if we have the root tile
        let mut root_slot = None;
        for (id, &slot) in &self.loaded_tiles {
            if id.z == 0 {
                root_slot = Some(slot);
                break;
            }
        }

        // Fill texture: R=Slot, G=Zoom
        if let Some(slot) = root_slot {
            for i in 0..(64 * 32) {
                self.indirection_data[i * 2] = slot as u8;     // R: Slot ID
                self.indirection_data[i * 2 + 1] = 0;          // G: Zoom Level (0)
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