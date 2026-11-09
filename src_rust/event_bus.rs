use std::sync::Arc;
use tokio::sync::broadcast;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContainerEvent {
    #[serde(rename = "type")]
    pub event_type: String,
    pub message: String,
}

pub struct EventBus {
    sender: broadcast::Sender<(String, ContainerEvent)>,
}

impl EventBus {
    pub fn new() -> Arc<Self> {
        let (sender, _) = broadcast::channel(1000);
        Arc::new(Self { sender })
    }

    pub fn emit(&self, container_id: &str, event: ContainerEvent) {
        let _ = self.sender.send((container_id.to_string(), event));
    }

    pub fn subscribe(&self) -> broadcast::Receiver<(String, ContainerEvent)> {
        self.sender.subscribe()
    }
}
