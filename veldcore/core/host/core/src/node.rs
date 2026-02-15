use iroh::Endpoint;
use iroh_gossip::net::Gossip;
use std::sync::Arc;
use crate::core::{RpcRequest, RpcResponse};
use prost::Message;
use crate::dispatcher::Dispatcher;

pub struct VeldmapNode {
    endpoint: Endpoint,
    gossip: Gossip,
    dispatcher: Arc<Dispatcher>,
}

impl VeldmapNode {
    pub async fn new(endpoint: Endpoint, dispatcher: Arc<Dispatcher>) -> anyhow::Result<Self> {
        let gossip = Gossip::builder().spawn(endpoint.clone());
        Ok(Self { endpoint, gossip, dispatcher })
    }

    pub fn node_id(&self) -> iroh::EndpointId {
        self.endpoint.id()
    }

    pub fn dispatcher(&self) -> Arc<Dispatcher> {
        self.dispatcher.clone()
    }

    pub async fn run(self: Arc<Self>) -> anyhow::Result<()> {
        log::info!("Veldmap Iroh Node listening. Node ID: {}", self.endpoint.id());
        
        while let Some(incoming) = self.endpoint.accept().await {
            let dispatcher = self.dispatcher.clone();
            tokio::spawn(async move {
                if let Err(e) = handle_connection(incoming, dispatcher).await {
                    log::error!("Connection error: {}", e);
                }
            });
        }
        Ok(())
    }

    pub async fn broadcast_update(&self, topic_id_bytes: [u8; 32], data: Vec<u8>) -> anyhow::Result<()> {
        let topic_id = iroh_gossip::proto::TopicId::from(topic_id_bytes);
        let mut topic = self.gossip.subscribe(topic_id, vec![]).await?;
        topic.broadcast(data.into()).await?;
        Ok(())
    }
}

async fn handle_connection(incoming: iroh::endpoint::Incoming, dispatcher: Arc<Dispatcher>) -> anyhow::Result<()> {
    let connection = incoming.await?;
    // log::info!("New connection from {}", connection.remote_node_id());

    loop {
        match connection.accept_bi().await {
            Ok((mut send, mut recv)) => {
                let dispatcher = dispatcher.clone();
                tokio::spawn(async move {
                    let mut len_buf = [0u8; 4];
                    if recv.read_exact(&mut len_buf).await.is_err() { return; }
                    let len = u32::from_be_bytes(len_buf) as usize;

                    let mut buf = vec![0u8; len];
                    if recv.read_exact(&mut buf).await.is_err() { return; }

                    let request = match RpcRequest::decode(&buf[..]) {
                        Ok(req) => req,
                        Err(_) => return,
                    };

                    let (payload, error) = match dispatcher.call(&request.service, &request.method, request.payload, request.instance_id).await {
                        Ok(p) => (p, String::new()),
                        Err(e) => (Vec::new(), e.to_string()),
                    };

                    let response = RpcResponse {
                        payload,
                        error,
                        sync: None,
                    };

                    let mut out_buf = Vec::new();
                    if response.encode(&mut out_buf).is_ok() {
                        let _ = send.write_all(&(out_buf.len() as u32).to_be_bytes()).await;
                        let _ = send.write_all(&out_buf).await;
                    }
                    let _ = send.finish();
                });
            }
            Err(e) => {
                log::debug!("Connection closed: {}", e);
                break;
            }
        }
    }
    Ok(())
}
