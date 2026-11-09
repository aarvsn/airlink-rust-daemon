use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use bollard::container::ListContainersOptions;
use bollard::system::EventsOptions;
use bollard::Docker;
use futures_util::StreamExt;

use crate::{log_error, log_info};

pub struct ContainerStateManager {
    docker: Docker,
    state_map: Mutex<HashMap<String, bool>>,
}

impl ContainerStateManager {
    pub fn new(docker: Docker) -> Arc<Self> {
        Arc::new(Self {
            docker,
            state_map: Mutex::new(HashMap::new()),
        })
    }

    pub fn is_running(&self, id: &str) -> Option<bool> {
        let map = self.state_map.lock().unwrap();
        map.get(id).copied()
    }

    pub fn set_running(&self, id: &str, running: bool) {
        let mut map = self.state_map.lock().unwrap();
        map.insert(id.to_string(), running);
    }

    pub async fn init(self: Arc<Self>) {
        self.seed().await;
        let self_clone = self.clone();
        tokio::spawn(async move {
            self_clone.subscribe_events().await;
        });
    }

    async fn seed(&self) {
        let filters = HashMap::new();
        let options = ListContainersOptions::<String> {
            all: true,
            filters,
            ..Default::default()
        };

        if let Ok(containers) = self.docker.list_containers(Some(options)).await {
            let mut map = self.state_map.lock().unwrap();
            for c in &containers {
                let running = c.state.as_deref() == Some("running");
                if let Some(ref id) = c.id {
                    map.insert(id.clone(), running);
                }
                if let Some(names) = &c.names {
                    for name in names {
                        let clean = name.trim_start_matches('/');
                        map.insert(clean.to_string(), running);
                    }
                }
            }
            log_info!("Container state map seeded with {} containers", containers.len());
        }
    }

    async fn subscribe_events(&self) {
        loop {
            let mut filters = HashMap::new();
            filters.insert("type".to_string(), vec!["container".to_string()]);
            let options = EventsOptions::<String> {
                since: None,
                until: None,
                filters,
            };

            let mut stream = self.docker.events(Some(options));
            log_info!("Docker event stream subscribed");

            while let Some(msg_res) = stream.next().await {
                if let Ok(msg) = msg_res {
                    let action = msg.action.unwrap_or_default();
                    let id = msg.actor.as_ref().and_then(|a| a.id.clone()).unwrap_or_default();
                    let name = msg.actor.as_ref()
                        .and_then(|a| a.attributes.as_ref())
                        .and_then(|attrs| attrs.get("name"))
                        .cloned()
                        .unwrap_or_default();

                    let mut map = self.state_map.lock().unwrap();
                    if action == "start" {
                        if !id.is_empty() { map.insert(id.clone(), true); }
                        if !name.is_empty() { map.insert(name, true); }
                    } else if action == "die" || action == "stop" {
                        if !id.is_empty() { map.insert(id.clone(), false); }
                        if !name.is_empty() { map.insert(name, false); }
                    } else if action == "destroy" {
                        if !id.is_empty() { map.remove(&id); }
                        if !name.is_empty() { map.remove(&name); }
                    }
                }
            }

            log_error!("Docker event stream ended — reconnecting in 3s");
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        }
    }
}
