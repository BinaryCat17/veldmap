use std::net::SocketAddr;
use std::path::PathBuf;

pub struct ServerConfig {
    pub addr: SocketAddr,
    pub data_path: PathBuf,
}

pub trait VeldMapServer: Send + Sync {
    fn run(&self) -> anyhow::Result<()>;
}