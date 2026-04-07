use veldmap_host_core::dispatcher::NativeService;
use veldmap_host_core::resources::ResourceManager;
use veldmap_host_core::core::{
    FsReadRequest, FsReadResponse, FsWriteRequest, FsListRequest, FsListResponse, ResourceHandle
};
use prost::Message;
use std::sync::Arc;
use std::path::Path;
use std::fs;

pub struct FsService {
    resources: Arc<ResourceManager>,
}

impl FsService {
    pub fn new(resources: Arc<ResourceManager>) -> Self {
        Self { resources }
    }

    fn is_path_safe(path: &str) -> bool {
        let path_obj = Path::new(path);
        if path_obj.is_absolute() { return false; }
        for component in path_obj.components() {
            if matches!(component, std::path::Component::ParentDir) { return false; }
        }
        true
    }
}

impl NativeService for FsService {
    fn call(&self, method: &str, payload: Vec<u8>, requestor_id: u32) -> anyhow::Result<Vec<u8>> {
        match method {
            "fs_read" => {
                let req = FsReadRequest::decode(&payload[..])?;
                if !Self::is_path_safe(&req.path) { return Err(anyhow::anyhow!("Access denied")); }
                
                let data = fs::read(&req.path)?;
                let size = data.len() as u64;
                let id = self.resources.create_data_resource(data, requestor_id);
                
                let handle = ResourceHandle {
                    id,
                    size,
                    content_hash: self.resources.compute_hash(id, requestor_id).unwrap_or_default(),
                };
                Ok(FsReadResponse { handle: Some(handle) }.encode_to_vec())
            }
            "fs_write" => {
                let req = FsWriteRequest::decode(&payload[..])?;
                if !Self::is_path_safe(&req.path) { return Err(anyhow::anyhow!("Access denied")); }
                let handle = req.handle.ok_or_else(|| anyhow::anyhow!("Missing handle"))?;
                
                let data = if handle.id == 0 {
                    return Err(anyhow::anyhow!("Handle ID 0 not supported for fs_write yet"));
                } else {
                    self.resources.read_resource(handle.id, 0, handle.size, requestor_id)?
                };

                if let Some(parent) = Path::new(&req.path).parent() { fs::create_dir_all(parent)?; }
                fs::write(&req.path, &data)?;
                Ok(Vec::new())
            }
            "fs_list" => {
                let req = FsListRequest::decode(&payload[..])?;
                if !Self::is_path_safe(&req.path) { return Err(anyhow::anyhow!("Access denied")); }
                let mut entries = Vec::new();
                if Path::new(&req.path).exists() {
                    for entry in fs::read_dir(&req.path)? {
                        let entry = entry?;
                        if let Some(name) = entry.file_name().to_str() { entries.push(name.to_string()); }
                    }
                }
                Ok(FsListResponse { entries }.encode_to_vec())
            }
            _ => Err(anyhow::anyhow!("Unknown FS method")),
        }
    }
}
