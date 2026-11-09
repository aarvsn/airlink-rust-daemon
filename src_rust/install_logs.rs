use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::Mutex;

const INSTALL_LOGS_PATH: &str = "storage/install_logs.json";

static LOGS_LOCK: Mutex<()> = Mutex::new(());

fn read_install_logs() -> HashMap<String, String> {
    let _guard = LOGS_LOCK.lock().unwrap();
    if !Path::new(INSTALL_LOGS_PATH).exists() {
        return HashMap::new();
    }
    if let Ok(content) = fs::read_to_string(INSTALL_LOGS_PATH) {
        if let Ok(map) = serde_json::from_str::<HashMap<String, String>>(&content) {
            return map;
        }
    }
    HashMap::new()
}

fn write_install_logs(logs: &HashMap<String, String>) {
    let _guard = LOGS_LOCK.lock().unwrap();
    if let Some(parent) = Path::new(INSTALL_LOGS_PATH).parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(json_str) = serde_json::to_string_pretty(logs) {
        let _ = fs::write(INSTALL_LOGS_PATH, json_str);
    }
}

pub fn set_server_state(container_id: &str, state: &str) {
    let mut logs = read_install_logs();
    logs.insert(container_id.to_string(), state.to_string());
    write_install_logs(&logs);
}

pub fn get_server_state(container_id: &str) -> Option<String> {
    let logs = read_install_logs();
    logs.get(container_id).cloned()
}
