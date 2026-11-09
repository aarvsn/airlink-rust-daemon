use std::collections::{HashMap, HashSet};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use base64::Engine;
use bollard::container::{Config as ContainerConfig, CreateContainerOptions, RemoveContainerOptions, StartContainerOptions, StopContainerOptions};
use bollard::image::CreateImageOptions;
use bollard::models::{HostConfig, PortBinding};
use bollard::Docker;
use futures_util::StreamExt;
use sha2::{Digest, Sha256};

use crate::validation::validate_container_id;
use crate::log_info;

const SESSION_TTL_SECS: u64 = 24 * 60 * 60;
const SFTP_IMAGE: &str = "atmoz/sftp";
const SFTP_USER_PREFIX: &str = "alsftp_";
const PORT_RANGE_START: u16 = 3003;
const PORT_RANGE_END: u16 = 4000;

#[derive(Clone, Debug)]
pub struct SftpCredential {
    pub username: String,
    pub password: String,
    pub host: String,
    pub port: u16,
    pub expires_at: u64,
}

struct ActiveSession {
    pub container_id: String,
    pub _username: String,
    pub sftp_container_name: String,
    pub port: u16,
    pub expires_at: u64,
}

pub struct SftpManager {
    docker: Docker,
    remote_host: String,
    sessions: Mutex<HashMap<String, ActiveSession>>,
}

impl SftpManager {
    pub fn new(docker: Docker, remote_host: String) -> Arc<Self> {
        let mgr = Arc::new(Self {
            docker,
            remote_host,
            sessions: Mutex::new(HashMap::new()),
        });

        let mgr_clone = mgr.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(3600));
            loop {
                interval.tick().await;
                mgr_clone.cleanup_expired().await;
            }
        });

        mgr
    }

    fn generate_username(&self, container_id: &str) -> String {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis();
        let payload = format!("{}{}", container_id, now);
        let mut hasher = Sha256::new();
        hasher.update(payload.as_bytes());
        let hash = hex::encode(hasher.finalize());
        format!("{}{}", SFTP_USER_PREFIX, &hash[..8])
    }

    fn generate_password(&self) -> String {
        use rand::RngCore;
        let mut bytes = [0u8; 24];
        rand::thread_rng().fill_bytes(&mut bytes);
        base64::prelude::BASE64_URL_SAFE_NO_PAD.encode(bytes)
    }

    fn is_port_free(&self, port: u16) -> bool {
        TcpListener::bind(("0.0.0.0", port)).is_ok()
    }

    fn allocate_port(&self) -> Result<u16, String> {
        let blocked_ports: HashSet<u16> = [
            3000, 3001, 3002, 3003, 3306, 3389, 4000, 5432, 5900, 6379, 8080, 8443, 8888,
        ].into_iter().collect();

        let used: HashSet<u16> = self.sessions.lock().unwrap().values().map(|s| s.port).collect();

        for port in PORT_RANGE_START..=PORT_RANGE_END {
            if blocked_ports.contains(&port) || used.contains(&port) {
                continue;
            }
            if self.is_port_free(port) {
                return Ok(port);
            }
        }
        Err("No free SFTP ports available in range.".to_string())
    }

    async fn pull_sftp_image(&self) -> Result<(), String> {
        if self.docker.inspect_image(SFTP_IMAGE).await.is_ok() {
            return Ok(());
        }

        let options = CreateImageOptions {
            from_image: SFTP_IMAGE,
            ..Default::default()
        };

        let mut stream = self.docker.create_image(Some(options), None, None);
        while let Some(res) = stream.next().await {
            if let Err(e) = res {
                return Err(format!("Failed to pull {}: {}", SFTP_IMAGE, e));
            }
        }
        Ok(())
    }

    pub async fn generate_credential(&self, container_id: &str) -> Result<SftpCredential, String> {
        if !validate_container_id(container_id) {
            return Err("Invalid container ID".to_string());
        }

        let volume_path = PathBuf::from("volumes").join(container_id);
        if !volume_path.exists() {
            return Err(format!("Volume for container {} does not exist", container_id));
        }

        let real_vol = volume_path.canonicalize().map_err(|e| e.to_string())?;

        let session_key = format!("container:{}", container_id);
        let _ = self.revoke_credential(&session_key).await;

        self.pull_sftp_image().await?;

        let port = self.allocate_port()?;
        let username = self.generate_username(container_id);
        let password = self.generate_password();
        let sftp_name = format!("alsftp_{}", container_id);
        let expires_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
            + (SESSION_TTL_SECS * 1000);

        // Remove old sftp container if any
        let _ = self.docker.remove_container(&sftp_name, Some(RemoveContainerOptions { force: true, ..Default::default() })).await;

        #[cfg(unix)]
        {
            use std::os::unix::fs::chown;
            let _ = chown(&real_vol, Some(1000), Some(1000));
        }

        let mut port_bindings = HashMap::new();
        port_bindings.insert("22/tcp".to_string(), Some(vec![PortBinding {
            host_ip: None,
            host_port: Some(port.to_string()),
        }]));

        let container_config = ContainerConfig {
            image: Some(SFTP_IMAGE.to_string()),
            cmd: Some(vec![format!("{}:{}:::upload", username, password)]),
            host_config: Some(HostConfig {
                binds: Some(vec![format!("{}:/home/{}/upload", real_vol.to_string_lossy(), username)]),
                port_bindings: Some(port_bindings),
                auto_remove: Some(true),
                ..Default::default()
            }),
            ..Default::default()
        };

        self.docker.create_container(
            Some(CreateContainerOptions {
                name: sftp_name.clone(),
                platform: None,
            }),
            container_config,
        ).await.map_err(|e| format!("Failed to create SFTP container: {}", e))?;

        self.docker.start_container(&sftp_name, None::<StartContainerOptions<String>>)
            .await
            .map_err(|e| format!("Failed to start SFTP container: {}", e))?;

        let cred = SftpCredential {
            username: username.clone(),
            password,
            host: self.remote_host.clone(),
            port,
            expires_at,
        };

        self.sessions.lock().unwrap().insert(session_key, ActiveSession {
            container_id: container_id.to_string(),
            _username: username,
            sftp_container_name: sftp_name,
            port,
            expires_at,
        });

        log_info!("SFTP session started for container {}: port={}", container_id, port);

        Ok(cred)
    }

    pub async fn revoke_credential(&self, session_key: &str) -> Result<(), String> {
        let session = {
            let mut map = self.sessions.lock().unwrap();
            map.remove(session_key)
        };

        if let Some(s) = session {
            let _ = self.docker.stop_container(&s.sftp_container_name, Some(StopContainerOptions { t: 3 })).await;
            log_info!("SFTP session ended for container {}", s.container_id);
        }

        Ok(())
    }

    pub async fn revoke_for_container(&self, container_id: &str) -> Result<(), String> {
        self.revoke_credential(&format!("container:{}", container_id)).await
    }

    pub fn get_active_count(&self) -> usize {
        self.sessions.lock().unwrap().len()
    }

    pub async fn cleanup_expired(&self) {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64;
        let expired_keys: Vec<String> = {
            let map = self.sessions.lock().unwrap();
            map.iter().filter(|(_, s)| s.expires_at <= now).map(|(k, _)| k.clone()).collect()
        };

        for key in expired_keys {
            let _ = self.revoke_credential(&key).await;
        }
    }
}
