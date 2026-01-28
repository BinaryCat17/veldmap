use std::collections::HashMap;
pub use veldmap_core::data_module::TileId;

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