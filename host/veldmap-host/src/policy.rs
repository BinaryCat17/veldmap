use std::collections::BTreeMap;
use std::fs;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Policy {
    pub components: BTreeMap<String, ComponentPolicy>,
}

#[derive(Debug, Deserialize)]
pub struct ComponentPolicy {
    #[allow(dead_code)]
    pub imports: Vec<String>,
    pub socket_addresses: Vec<String>,
    pub egress_origins: Vec<String>,
    pub additional_tls_roots: Vec<PathBuf>,
}

#[derive(Clone, Copy)]
pub struct AllowedSocket {
    address: IpAddr,
    port: Option<u16>,
}

impl AllowedSocket {
    pub fn matches(self, address: SocketAddr) -> bool {
        self.address == address.ip() && self.port.is_none_or(|port| port == address.port())
    }
}

pub struct LoadedPolicy {
    pub component: ComponentPolicy,
    pub sockets: Vec<AllowedSocket>,
    pub directory: PathBuf,
}

impl Policy {
    pub fn load(path: &Path, component: &str) -> anyhow::Result<LoadedPolicy> {
        let source = fs::read_to_string(path)?;
        let policy: Self = toml::from_str(&source)?;
        let component_policy = policy
            .components
            .into_iter()
            .find_map(|(name, policy)| (name == component).then_some(policy))
            .ok_or_else(|| anyhow::anyhow!("no policy for component {component}"))?;
        let sockets = component_policy
            .socket_addresses
            .iter()
            .map(|value| parse_socket(value))
            .collect::<anyhow::Result<Vec<_>>>()?;
        let directory = path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        Ok(LoadedPolicy {
            component: component_policy,
            sockets,
            directory,
        })
    }
}

fn parse_socket(value: &str) -> anyhow::Result<AllowedSocket> {
    let (address, port) = value
        .rsplit_once(':')
        .ok_or_else(|| anyhow::anyhow!("invalid socket policy {value}"))?;
    let address = address.trim_matches(['[', ']']).parse()?;
    let port = (port != "*").then(|| port.parse()).transpose()?;
    Ok(AllowedSocket { address, port })
}
