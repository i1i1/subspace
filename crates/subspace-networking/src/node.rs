use crate::shared::{Command, CreatedSubscription, Shared};
use bytes::Bytes;
use event_listener_primitives::HandlerId;
use futures::channel::{mpsc, oneshot};
use futures::SinkExt;
use libp2p::core::multihash::Multihash;
use libp2p::gossipsub::error::SubscriptionError;
use libp2p::gossipsub::Sha256Topic;
use libp2p::{Multiaddr, PeerId};
use std::ops::{Deref, DerefMut};
use std::sync::Arc;
use thiserror::Error;

/// Topic subscription, will unsubscribe when last instance is dropped for a particular topic.
#[derive(Debug)]
pub struct TopicSubscription {
    topic: Option<Sha256Topic>,
    subscription_id: usize,
    command_sender: Option<mpsc::Sender<Command>>,
    receiver: mpsc::UnboundedReceiver<Bytes>,
}

impl Deref for TopicSubscription {
    type Target = mpsc::UnboundedReceiver<Bytes>;

    fn deref(&self) -> &Self::Target {
        &self.receiver
    }
}

impl DerefMut for TopicSubscription {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.receiver
    }
}

impl Drop for TopicSubscription {
    fn drop(&mut self) {
        let topic = self
            .topic
            .take()
            .expect("Always specified on creation and only removed on drop; qed");
        let subscription_id = self.subscription_id;
        let mut command_sender = self
            .command_sender
            .take()
            .expect("Always specified on creation and only removed on drop; qed");

        tokio::spawn(async move {
            // Doesn't matter if node runner is already dropped.
            let _ = command_sender
                .send(Command::Unsubscribe {
                    topic,
                    subscription_id,
                })
                .await;
        });
    }
}

#[derive(Debug, Error)]
pub enum GetValueError {
    /// Node runner was dropped, impossible to get value.
    #[error("Node runner was dropped, impossible to get value")]
    NodeRunnerDropped,
}

#[derive(Debug, Error)]
pub enum SubscribeError {
    /// Node runner was dropped, impossible to subscribe.
    #[error("Node runner was dropped, impossible to get value")]
    NodeRunnerDropped,
    /// Failed to create subscription.
    #[error("Failed to create subscription: {0}")]
    Subscription(#[from] SubscriptionError),
}

#[derive(Debug, Error)]
pub enum PublishError {
    /// Node runner was dropped, impossible to publish.
    #[error("Node runner was dropped, impossible to get value")]
    NodeRunnerDropped,
    /// Failed to publish message.
    #[error("Failed to publish message: {0}")]
    Publish(#[from] libp2p::gossipsub::error::PublishError),
}

/// Implementation of a network node on Subspace Network.
#[derive(Debug, Clone)]
pub struct Node {
    shared: Arc<Shared>,
}

impl Node {
    pub(crate) fn new(shared: Arc<Shared>) -> Self {
        Self { shared }
    }

    /// Node's own local ID.
    pub fn id(&self) -> PeerId {
        self.shared.id
    }

    pub async fn get_value(&self, key: Multihash) -> Result<Option<Vec<u8>>, GetValueError> {
        let (result_sender, result_receiver) = oneshot::channel();

        self.shared
            .command_sender
            .clone()
            .send(Command::GetValue { key, result_sender })
            .await
            .map_err(|_error| GetValueError::NodeRunnerDropped)?;

        result_receiver
            .await
            .map_err(|_error| GetValueError::NodeRunnerDropped)
    }

    pub async fn subscribe(&self, topic: Sha256Topic) -> Result<TopicSubscription, SubscribeError> {
        let (result_sender, result_receiver) = oneshot::channel();

        self.shared
            .command_sender
            .clone()
            .send(Command::Subscribe {
                topic: topic.clone(),
                result_sender,
            })
            .await
            .map_err(|_error| SubscribeError::NodeRunnerDropped)?;

        let CreatedSubscription {
            subscription_id,
            receiver,
        } = result_receiver
            .await
            .map_err(|_error| SubscribeError::NodeRunnerDropped)?
            .map_err(SubscribeError::Subscription)?;

        Ok(TopicSubscription {
            topic: Some(topic),
            subscription_id,
            command_sender: Some(self.shared.command_sender.clone()),
            receiver,
        })
    }

    pub async fn publish(&self, topic: Sha256Topic, message: Vec<u8>) -> Result<(), PublishError> {
        let (result_sender, result_receiver) = oneshot::channel();

        self.shared
            .command_sender
            .clone()
            .send(Command::Publish {
                topic,
                message,
                result_sender,
            })
            .await
            .map_err(|_error| PublishError::NodeRunnerDropped)?;

        result_receiver
            .await
            .map_err(|_error| PublishError::NodeRunnerDropped)?
            .map_err(PublishError::Publish)
    }

    /// Node's own addresses where it listens for incoming requests.
    pub fn listeners(&self) -> Vec<Multiaddr> {
        self.shared.listeners.lock().clone()
    }

    /// Callback is called when node starts listening on new address.
    pub fn on_new_listener(
        &self,
        callback: Arc<dyn Fn(&Multiaddr) + Send + Sync + 'static>,
    ) -> HandlerId {
        self.shared.handlers.new_listener.add(callback)
    }
}
