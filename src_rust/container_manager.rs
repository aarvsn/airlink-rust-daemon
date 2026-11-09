use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use bollard::container::{
    AttachContainerOptions, Config as ContainerConfig, CreateContainerOptions,
    RemoveContainerOptions, StartContainerOptions, StatsOptions,
    StopContainerOptions, WaitContainerOptions,
};
use bollard::image::CreateImageOptions;
use bollard::models::{HostConfig, PortBinding, RestartPolicy, RestartPolicyNameEnum};
use bollard::Docker;
use futures_util::StreamExt;
use serde_json::Value;
use tokio::io::AsyncWriteExt;

use crate::container_state::ContainerStateManager;
use crate::event_bus::{ContainerEvent, EventBus};
use crate::install_logs::set_server_state;

pub struct ContainerManager {
    pub docker: Docker,
    pub event_bus: Arc<EventBus>,
    pub state_mgr: Arc<ContainerStateManager>,
}

impl ContainerManager {
    pub fn new(docker: Docker, event_bus: Arc<EventBus>, state_mgr: Arc<ContainerStateManager>) -> Arc<Self> {
        Arc::new(Self {
            docker,
            event_bus,
            state_mgr,
        })
    }

    pub fn init_container_volume(&self, id: &str) -> PathBuf {
        let volumes_dir = std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join("volumes");
        let volume_path = volumes_dir.join(id);

        let _ = fs::create_dir_all(&volume_path);
        volume_path
    }

    pub async fn pull_image_if_needed(&self, id: &str, image: &str) -> Result<(), String> {
        if self.docker.inspect_image(image).await.is_ok() {
            return Ok(());
        }

        self.event_bus.emit(id, ContainerEvent {
            event_type: "pulling".to_string(),
            message: format!("Pulling {}", image),
        });

        let options = CreateImageOptions {
            from_image: image,
            ..Default::default()
        };

        let mut stream = self.docker.create_image(Some(options), None, None);
        while let Some(item_res) = stream.next().await {
            match item_res {
                Ok(info) => {
                    if let Some(status) = info.status {
                        if status == "Status" {
                            if let Some(progress_id) = info.id {
                                self.event_bus.emit(id, ContainerEvent {
                                    event_type: "pulling".to_string(),
                                    message: progress_id,
                                });
                            }
                        }
                    }
                }
                Err(e) => {
                    let err_msg = format!("Pull failed: {}", e);
                    self.event_bus.emit(id, ContainerEvent {
                        event_type: "error".to_string(),
                        message: err_msg.clone(),
                    });
                    return Err(err_msg);
                }
            }
        }

        self.event_bus.emit(id, ContainerEvent {
            event_type: "pulling".to_string(),
            message: format!("Image {} pulled successfully", image),
        });

        Ok(())
    }

    pub async fn start_container(
        &self,
        id: &str,
        image: &str,
        mut env: HashMap<String, String>,
        ports: &str,
        memory: u64,
        cpu: u64,
    ) -> Result<(), String> {
        // Remove existing container
        let _ = self.docker.remove_container(id, Some(RemoveContainerOptions {
            force: true,
            ..Default::default()
        })).await;

        let volume_path = self.init_container_volume(id);
        let canonical_volume = volume_path.canonicalize().map_err(|e| e.to_string())?;

        // Port bindings
        let mut port_bindings: HashMap<String, Option<Vec<PortBinding>>> = HashMap::new();
        let mut exposed_ports: HashMap<String, HashMap<(), ()>> = HashMap::new();

        for mapping in ports.split(',') {
            let parts: Vec<&str> = mapping.split(':').collect();
            if parts.len() == 2 {
                let host_port = parts[0].trim();
                let container_port = parts[1].trim();
                if !host_port.is_empty() && !container_port.is_empty() {
                    let container_key = format!("{}/tcp", container_port);
                    port_bindings.insert(
                        container_key.clone(),
                        Some(vec![PortBinding {
                            host_ip: None,
                            host_port: Some(host_port.to_string()),
                        }]),
                    );
                    exposed_ports.insert(container_key, HashMap::new());
                }
            }
        }

        self.pull_image_if_needed(id, image).await?;

        self.event_bus.emit(id, ContainerEvent {
            event_type: "creating".to_string(),
            message: "Creating container".to_string(),
        });

        // Pre-write eula.txt
        let eula_path = canonical_volume.join("eula.txt");
        if !eula_path.exists() || !fs::read_to_string(&eula_path).unwrap_or_default().contains("eula=true") {
            let _ = fs::write(&eula_path, "#By installing Minecraft you agree to the EULA\neula=true\n");
        }

        // Setup .airlinkd/init.sh wrapper
        let inspect_res = self.docker.inspect_image(image).await.ok();
        let raw_entrypoint = inspect_res.as_ref()
            .and_then(|i| i.config.as_ref())
            .and_then(|c| c.entrypoint.clone())
            .unwrap_or_default();
        let raw_cmd = inspect_res.as_ref()
            .and_then(|i| i.config.as_ref())
            .and_then(|c| c.cmd.clone())
            .unwrap_or_default();

        let quote_arg = |s: &str| format!("'{}'", s.replace('\'', "'\\''"));
        let exec_line = if !raw_entrypoint.is_empty() {
            let ep_str = raw_entrypoint.iter().map(|s| quote_arg(s)).collect::<Vec<_>>().join(" ");
            let cmd_str = if !raw_cmd.is_empty() {
                format!(" {}", raw_cmd.iter().map(|s| quote_arg(s)).collect::<Vec<_>>().join(" "))
            } else {
                "".to_string()
            };
            format!("exec {}{}", ep_str, cmd_str)
        } else if !raw_cmd.is_empty() {
            format!("exec {}", raw_cmd.iter().map(|s| quote_arg(s)).collect::<Vec<_>>().join(" "))
        } else {
            "exec /bin/sh".to_string()
        };

        let airlinkd_dir = canonical_volume.join(".airlinkd");
        let _ = fs::create_dir_all(&airlinkd_dir);
        let wrapper_script = format!(
            "#!/bin/sh\necho 'airlinkd' > /etc/hostname 2>/dev/null || true\nhostname airlinkd 2>/dev/null || true\nif [ -f /etc/passwd ]; then\n  sed -i 's|^container:|airlinkd:|' /etc/passwd 2>/dev/null || true\n  sed -i 's|^user:|airlinkd:|'      /etc/passwd 2>/dev/null || true\n  sed -i 's|^app:|airlinkd:|'       /etc/passwd 2>/dev/null || true\nfi\n{}\n",
            exec_line
        );
        let wrapper_path = airlinkd_dir.join("init.sh");
        let _ = fs::write(&wrapper_path, &wrapper_script);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&wrapper_path, fs::Permissions::from_mode(0o755));
        }

        env.insert("PS1".to_string(), "airlinkd~ ".to_string());
        env.insert("PROMPT".to_string(), "airlinkd~ ".to_string());
        env.insert("prompt".to_string(), "airlinkd~ ".to_string());

        let env_vec: Vec<String> = env.into_iter().map(|(k, v)| format!("{}={}", k, v)).collect();

        let mem_bytes = (memory * 1024 * 1024) as i64;
        let nano_cpus = ((cpu as f64 / 100.0).max(0.5) * 1e9) as i64;

        let host_config = HostConfig {
            binds: Some(vec![format!("{}:/home/container", canonical_volume.to_string_lossy())]),
            port_bindings: Some(port_bindings),
            memory: Some(mem_bytes),
            nano_cpus: Some(nano_cpus),
            restart_policy: Some(RestartPolicy {
                name: Some(RestartPolicyNameEnum::NO),
                maximum_retry_count: None,
            }),
            ..Default::default()
        };

        let container_config = ContainerConfig {
            hostname: Some("airlinkd".to_string()),
            image: Some(image.to_string()),
            env: Some(env_vec),
            entrypoint: Some(vec!["/bin/sh".to_string(), "/home/container/.airlinkd/init.sh".to_string()]),
            working_dir: Some("/home/container".to_string()),
            host_config: Some(host_config),
            exposed_ports: Some(exposed_ports),
            attach_stdout: Some(true),
            attach_stderr: Some(true),
            attach_stdin: Some(true),
            open_stdin: Some(true),
            tty: Some(true),
            ..Default::default()
        };

        self.docker.create_container(
            Some(CreateContainerOptions {
                name: id,
                platform: None,
            }),
            container_config,
        ).await.map_err(|e| format!("Failed to create container: {}", e))?;

        self.event_bus.emit(id, ContainerEvent {
            event_type: "starting".to_string(),
            message: "Starting container".to_string(),
        });

        self.docker.start_container(id, None::<StartContainerOptions<String>>)
            .await
            .map_err(|e| format!("Failed to start container: {}", e))?;

        self.state_mgr.set_running(id, true);

        self.event_bus.emit(id, ContainerEvent {
            event_type: "started".to_string(),
            message: "Server started".to_string(),
        });

        Ok(())
    }

    pub async fn stop_container(&self, id: &str, stop_cmd: Option<&str>) -> Result<(), String> {
        let inspect = self.docker.inspect_container(id, None).await.ok();
        let running = inspect.as_ref()
            .and_then(|i| i.state.as_ref())
            .and_then(|s| s.running)
            .unwrap_or(false);

        if !running {
            return Ok(());
        }

        self.event_bus.emit(id, ContainerEvent {
            event_type: "stopping".to_string(),
            message: "Stopping server".to_string(),
        });

        if let Some(cmd) = stop_cmd {
            if cmd != "kill" {
                let _ = self.send_command(id, cmd).await;
                let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
                while tokio::time::Instant::now() < deadline {
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    if let Ok(info) = self.docker.inspect_container(id, None).await {
                        if info.state.and_then(|s| s.running).unwrap_or(false) == false {
                            self.state_mgr.set_running(id, false);
                            self.event_bus.emit(id, ContainerEvent {
                                event_type: "stopped".to_string(),
                                message: "Server stopped".to_string(),
                            });
                            return Ok(());
                        }
                    }
                }
            }
        }

        let _ = self.docker.stop_container(id, Some(StopContainerOptions { t: 5 })).await;
        let _ = self.docker.remove_container(id, Some(RemoveContainerOptions { force: true, ..Default::default() })).await;

        self.state_mgr.set_running(id, false);
        self.event_bus.emit(id, ContainerEvent {
            event_type: "stopped".to_string(),
            message: "Server stopped".to_string(),
        });

        Ok(())
    }

    pub async fn kill_container(&self, id: &str) -> Result<(), String> {
        let _ = self.docker.remove_container(id, Some(RemoveContainerOptions { force: true, ..Default::default() })).await;
        self.state_mgr.set_running(id, false);
        self.event_bus.emit(id, ContainerEvent {
            event_type: "killed".to_string(),
            message: "Container forcibly removed".to_string(),
        });
        Ok(())
    }

    pub async fn delete_container_and_volume(&self, id: &str) -> Result<(), String> {
        let _ = self.kill_container(id).await;
        let volume_path = PathBuf::from("volumes").join(id);
        if volume_path.exists() {
            let _ = fs::remove_dir_all(&volume_path);
        }
        Ok(())
    }

    pub async fn send_command(&self, id: &str, command: &str) -> Result<(), String> {
        let inspect = self.docker.inspect_container(id, None).await.map_err(|e| e.to_string())?;
        if inspect.state.and_then(|s| s.running).unwrap_or(false) == false {
            return Err("Container is not running".to_string());
        }

        let options = AttachContainerOptions::<String> {
            stdin: Some(true),
            stream: Some(true),
            ..Default::default()
        };

        if let Ok(bollard::container::AttachContainerResults { mut input, .. }) = self.docker.attach_container(id, Some(options)).await {
            let cmd_with_newline = format!("{}\n", command);
            let _ = input.write_all(cmd_with_newline.as_bytes()).await;
            let _ = input.flush().await;
        }

        Ok(())
    }

    pub async fn get_container_stats(&self, id: &str) -> Option<Value> {
        let inspect = self.docker.inspect_container(id, None).await.ok()?;
        let state = inspect.state?;
        let running = state.running.unwrap_or(false);
        let status = state.status.map(|s| format!("{:?}", s).to_lowercase()).unwrap_or_default();
        let _starting = status == "created" || (running && state.started_at == state.finished_at);
        let stopping = status == "exited" || status == "removing" || state.paused.unwrap_or(false);

        if !running {
            return Some(serde_json::json!({
                "running": false,
                "starting": false,
                "stopping": stopping,
                "memory": { "usage": 0, "limit": 0, "percentage": "NaN" },
                "cpu": { "percentage": "NaN" },
                "storage": { "usage": "0" },
            }));
        }

        let mut stream = self.docker.stats(id, Some(StatsOptions { stream: false, one_shot: true }));
        if let Some(Ok(stats)) = stream.next().await {
            let memory_usage = stats.memory_stats.usage.unwrap_or(0);
            let memory_limit = stats.memory_stats.limit.unwrap_or(1);
            let memory_perc = (memory_usage as f64 / memory_limit as f64) * 100.0;

            let cpu_delta = stats.cpu_stats.cpu_usage.total_usage as f64 - stats.precpu_stats.cpu_usage.total_usage as f64;
            let sys_delta = stats.cpu_stats.system_cpu_usage.unwrap_or(0) as f64 - stats.precpu_stats.system_cpu_usage.unwrap_or(0) as f64;
            let cpus = stats.cpu_stats.online_cpus.unwrap_or(1) as f64;
            let cpu_perc = if sys_delta > 0.0 {
                (cpu_delta / sys_delta) * cpus * 100.0
            } else {
                0.0
            };

            let volume_dir = PathBuf::from("volumes").join(id);
            let storage_mb = (crate::fs_handler::FsManager::new().get_directory_size(&volume_dir, 0, 20) as f64) / (1024.0 * 1000.0);

            Some(serde_json::json!({
                "running": true,
                "starting": false,
                "stopping": false,
                "memory": {
                    "usage": memory_usage,
                    "limit": memory_limit,
                    "percentage": format!("{:.2}", memory_perc)
                },
                "cpu": {
                    "percentage": format!("{:.2}", cpu_perc)
                },
                "storage": {
                    "usage": format!("{:.2}", storage_mb)
                }
            }))
        } else {
            None
        }
    }

    pub async fn create_installer(
        &self,
        id: &str,
        image: &str,
        script: &str,
        env: HashMap<String, String>,
        entrypoint: &str,
    ) -> Result<(), String> {
        let installer_name = format!("installer_{}", id);
        let _ = self.docker.remove_container(&installer_name, Some(RemoveContainerOptions { force: true, ..Default::default() })).await;

        let volume_path = self.init_container_volume(id);
        let canonical_volume = volume_path.canonicalize().map_err(|e| e.to_string())?;

        self.event_bus.emit(id, ContainerEvent {
            event_type: "installing".to_string(),
            message: "Preparing installer".to_string(),
        });

        self.pull_image_if_needed(id, image).await?;

        self.event_bus.emit(id, ContainerEvent {
            event_type: "installing".to_string(),
            message: "Running install script".to_string(),
        });

        let env_vec: Vec<String> = env.into_iter().map(|(k, v)| format!("{}={}", k, v)).collect();

        let clean_script = script.replace("\r\n", "\n").replace('\r', "\n");
        let container_config = ContainerConfig {
            image: Some(image.to_string()),
            entrypoint: Some(vec![entrypoint.to_string(), "-c".to_string(), clean_script]),
            env: Some(env_vec),
            attach_stdout: Some(true),
            attach_stderr: Some(true),
            host_config: Some(HostConfig {
                binds: Some(vec![format!("{}:/mnt/server", canonical_volume.to_string_lossy())]),
                auto_remove: Some(false),
                network_mode: Some("host".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };

        self.docker.create_container(
            Some(CreateContainerOptions {
                name: installer_name.clone(),
                platform: None,
            }),
            container_config,
        ).await.map_err(|e| format!("Failed to create installer container: {}", e))?;

        let attach_opts = AttachContainerOptions::<String> {
            stdout: Some(true),
            stderr: Some(true),
            stream: Some(true),
            ..Default::default()
        };

        let attach_res = self.docker.attach_container(&installer_name, Some(attach_opts)).await.map_err(|e| e.to_string())?;

        self.docker.start_container(&installer_name, None::<StartContainerOptions<String>>).await.map_err(|e| e.to_string())?;

        let event_bus = self.event_bus.clone();
        let id_str = id.to_string();
        tokio::spawn(async move {
            let mut output_stream = attach_res.output;
            while let Some(Ok(chunk)) = output_stream.next().await {
                let text = chunk.to_string();
                for line in text.lines() {
                    let clean = line.trim();
                    if !clean.is_empty() {
                        event_bus.emit(&id_str, ContainerEvent {
                            event_type: "installing".to_string(),
                            message: clean.to_string(),
                        });
                    }
                }
            }
        });

        let mut wait_stream = self.docker.wait_container(&installer_name, None::<WaitContainerOptions<String>>);
        let exit_code = if let Some(Ok(res)) = wait_stream.next().await {
            res.status_code
        } else {
            -1
        };

        let _ = self.docker.remove_container(&installer_name, Some(RemoveContainerOptions { force: true, ..Default::default() })).await;

        if exit_code != 0 {
            set_server_state(id, "failed");
            let err = format!("Install script failed with exit code {}", exit_code);
            self.event_bus.emit(id, ContainerEvent {
                event_type: "error".to_string(),
                message: err.clone(),
            });
            return Err(err);
        }

        self.event_bus.emit(id, ContainerEvent {
            event_type: "installed".to_string(),
            message: "Installation complete".to_string(),
        });
        set_server_state(id, "installed");

        Ok(())
    }
}
