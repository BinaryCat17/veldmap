use tokio::net::TcpStream;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use crate::rpc::core::{RpcRequest, RpcResponse};
use prost::Message;
use std::sync::Arc;
use tokio::sync::Mutex;

pub struct VeldmapClient {
    stream: Arc<Mutex<TcpStream>>,
}

impl VeldmapClient {
    pub async fn connect(addr: &str) -> anyhow::Result<Self> {
        let stream = TcpStream::connect(addr).await?;
        Ok(Self {
            stream: Arc::new(Mutex::new(stream)),
        })
    }

    pub async fn call(&self, service: &str, method: &str, payload: Vec<u8>) -> anyhow::Result<Vec<u8>> {
        let request = RpcRequest {
            service: service.to_string(),
            method: method.to_string(),
            payload,
            sync: None,
        };

        let buf = request.encode_to_vec();
        let mut stream = self.stream.lock().await;

        stream.write_all(&(buf.len() as u32).to_be_bytes()).await?;
        stream.write_all(&buf).await?;

        let mut len_buf = [0u8; 4];
        stream.read_exact(&mut len_buf).await?;
        let len = u32::from_be_bytes(len_buf) as usize;

        let mut res_buf = vec![0u8; len];
        stream.read_exact(&mut res_buf).await?;

        let response = RpcResponse::decode(&res_buf[..])?;
        if !response.error.is_empty() {
            return Err(anyhow::anyhow!(response.error));
        }

        Ok(response.payload)
    }
}
