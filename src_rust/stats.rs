use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sysinfo::System;

const MAX_AGE_MS: i64 = 30 * 60 * 1000;
const STORAGE_PATH: &str = "storage/systemStats.json";
const TEMP_STORAGE_PATH: &str = "storage/systemStats.tmp.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemStat {
    pub timestamp: String,
    #[serde(rename = "RamMax")]
    pub ram_max: String,
    #[serde(rename = "Ram")]
    pub ram: String,
    #[serde(rename = "CoresMax")]
    pub cores_max: usize,
    #[serde(rename = "Cores")]
    pub cores: String,
}

pub struct StatsManager {
    system: Mutex<System>,
    stats_log: Mutex<Vec<SystemStat>>,
    start_time: Instant,
}

impl StatsManager {
    pub fn new() -> Arc<Self> {
        let mut sys = System::new_all();
        sys.refresh_all();

        let mut initial_log = Vec::new();
        if let Ok(content) = fs::read_to_string(STORAGE_PATH) {
            if let Ok(parsed) = serde_json::from_str::<Vec<SystemStat>>(&content) {
                initial_log = parsed;
            }
        }

        let mgr = Arc::new(Self {
            system: Mutex::new(sys),
            stats_log: Mutex::new(initial_log),
            start_time: Instant::now(),
        });

        mgr.clean_old_entries();
        mgr
    }

    pub fn get_uptime_formatted(&self) -> String {
        let uptime_seconds = self.start_time.elapsed().as_secs();
        let minutes = (uptime_seconds / 60) % 60;
        let hours = (uptime_seconds / 3600) % 24;
        let days = uptime_seconds / 86400;

        let mut parts = Vec::new();
        if days > 0 {
            parts.push(format!("{}d", days));
        }
        if hours > 0 {
            parts.push(format!("{}h", hours));
        }
        if minutes > 0 || parts.is_empty() {
            parts.push(format!("{}m", minutes));
        }

        parts.join(" ")
    }

    pub fn get_current_stats(&self) -> SystemStat {
        let mut sys = self.system.lock().unwrap();
        sys.refresh_cpu_all();
        sys.refresh_memory();

        let total_mem_mb = (sys.total_memory() as f64) / (1024.0 * 1024.0);
        let used_mem_mb = (sys.used_memory() as f64) / (1024.0 * 1024.0);
        let cpu_usage = sys.global_cpu_usage();
        let cores_max = sys.cpus().len();

        SystemStat {
            timestamp: Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            ram_max: format!("{:.2} MB", total_mem_mb),
            ram: format!("{:.2} MB", used_mem_mb),
            cores_max,
            cores: format!("{:.2}%", cpu_usage),
        }
    }

    pub fn save_current_stats(&self) {
        let stat = self.get_current_stats();
        let mut log = self.stats_log.lock().unwrap();
        log.push(stat);

        // Filter old entries
        let now = Utc::now().timestamp_millis();
        log.retain(|entry| {
            if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(&entry.timestamp) {
                now - dt.timestamp_millis() <= MAX_AGE_MS
            } else {
                false
            }
        });

        if let Ok(json_str) = serde_json::to_string_pretty(&*log) {
            if let Some(parent) = Path::new(STORAGE_PATH).parent() {
                let _ = fs::create_dir_all(parent);
            }
            if fs::write(TEMP_STORAGE_PATH, &json_str).is_ok() {
                let _ = fs::rename(TEMP_STORAGE_PATH, STORAGE_PATH);
            }
        }
    }

    fn clean_old_entries(&self) {
        let mut log = self.stats_log.lock().unwrap();
        let now = Utc::now().timestamp_millis();
        log.retain(|entry| {
            if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(&entry.timestamp) {
                now - dt.timestamp_millis() <= MAX_AGE_MS
            } else {
                false
            }
        });
    }

    pub fn get_total_stats(&self) -> Vec<SystemStat> {
        self.stats_log.lock().unwrap().clone()
    }

    pub fn start_collector(self: Arc<Self>, interval_ms: u64) {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(interval_ms));
            loop {
                interval.tick().await;
                self.save_current_stats();
            }
        });
    }
}
