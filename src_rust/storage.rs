use std::collections::HashMap;
use std::fs;
use std::path::Path;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetaConfig {
    pub meta: MetaInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetaInfo {
    pub version: String,
    pub codename: String,
}

pub fn load_meta_config() -> MetaConfig {
    let path = Path::new("storage/config.json");
    if let Ok(content) = fs::read_to_string(path) {
        if let Ok(meta) = serde_json::from_str::<MetaConfig>(&content) {
            return meta;
        }
    }

    MetaConfig {
        meta: MetaInfo {
            version: "1.0.1".to_string(),
            codename: "Glazzer Fridge".to_string(),
        },
    }
}

pub fn get_file_category(extension: &str) -> Option<String> {
    let path = Path::new("storage/fileSpecifier.json");
    if let Ok(content) = fs::read_to_string(path) {
        if let Ok(categories) = serde_json::from_str::<HashMap<String, Vec<String>>>(&content) {
            let ext_lower = extension.to_lowercase();
            for (category, extensions) in categories {
                if extensions.iter().any(|e| e.eq_ignore_ascii_case(&ext_lower)) {
                    return Some(category);
                }
            }
        }
    }
    None
}
