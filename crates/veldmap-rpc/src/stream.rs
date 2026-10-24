use std::num::NonZeroUsize;

use connectrpc::{ConnectError, ServiceStream};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("stream receiver is closed")]
pub struct StreamSendError;

pub struct BoundedSender<T> {
    sender: mpsc::Sender<Result<T, ConnectError>>,
}

impl<T> Clone for BoundedSender<T> {
    fn clone(&self) -> Self {
        Self {
            sender: self.sender.clone(),
        }
    }
}

impl<T> BoundedSender<T> {
    pub async fn send(&self, value: T) -> Result<(), StreamSendError> {
        self.sender
            .send(Ok(value))
            .await
            .map_err(|_| StreamSendError)
    }

    pub async fn fail(&self, error: ConnectError) -> Result<(), StreamSendError> {
        self.sender
            .send(Err(error))
            .await
            .map_err(|_| StreamSendError)
    }

    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.sender.is_closed()
    }

    pub async fn closed(&self) {
        self.sender.closed().await;
    }
}

pub fn bounded_stream<T>(capacity: NonZeroUsize) -> (BoundedSender<T>, ServiceStream<T>)
where
    T: Send + 'static,
{
    let (sender, receiver) = mpsc::channel(capacity.get());
    (
        BoundedSender { sender },
        Box::pin(ReceiverStream::new(receiver)),
    )
}
