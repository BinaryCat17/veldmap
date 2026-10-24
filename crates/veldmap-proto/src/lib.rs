connectrpc::include_generated!();

pub const FILE_DESCRIPTOR_SET: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/veldmap_descriptor.bin"));
