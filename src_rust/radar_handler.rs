use std::fs;
use std::io::Write;
use std::path::PathBuf;
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;
use zip::write::SimpleFileOptions;

use crate::validation::validate_container_id;

#[derive(Debug, Deserialize, Serialize)]
pub struct RadarPattern {
    #[serde(rename = "type")]
    pub pattern_type: String,
    pub pattern: String,
    pub description: Option<String>,
    pub content: Option<String>,
    pub size_less_than: Option<u64>,
    pub size_greater_than: Option<u64>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct RadarScript {
    pub name: String,
    pub description: Option<String>,
    pub version: Option<String>,
    pub patterns: Vec<RadarPattern>,
}

#[derive(Debug, Serialize)]
pub struct ScanMatch {
    pub path: String,
    pub size: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct ScanResult {
    pub pattern: RadarPattern,
    pub matches: Vec<ScanMatch>,
}

pub fn scan_volume(id: &str, script: RadarScript) -> Result<Vec<ScanResult>, String> {
    if !validate_container_id(id) {
        return Err("Invalid container ID".to_string());
    }

    let base_dir = PathBuf::from("volumes").join(id);
    if !base_dir.exists() {
        return Err(format!("Volume directory for {} does not exist", id));
    }
    let real_base = base_dir.canonicalize().map_err(|e| e.to_string())?;

    let mut results = Vec::new();

    for pattern in script.patterns {
        let mut matches = Vec::new();

        for entry in WalkDir::new(&real_base).into_iter().filter_map(|e| e.ok()) {
            let path = entry.path();
            let is_dir = entry.file_type().is_dir();
            let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            let rel_path = path.strip_prefix(&real_base).map(|p| p.to_string_lossy().to_string()).unwrap_or_default();

            if is_dir && (pattern.pattern_type == "extension" || pattern.content.is_some()) {
                continue;
            }

            let meta = entry.metadata().ok();
            let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);

            if let Some(slt) = pattern.size_less_than {
                if size >= slt {
                    continue;
                }
            }
            if let Some(sgt) = pattern.size_greater_than {
                if size <= sgt {
                    continue;
                }
            }

            let mut matched = false;

            if pattern.pattern_type == "filename" {
                if file_name.contains(&pattern.pattern) {
                    matched = true;
                }
            } else if pattern.pattern_type == "extension" {
                if file_name.ends_with(&pattern.pattern) {
                    matched = true;
                }
            }

            if matched && pattern.content.is_some() {
                if let Some(ref c_pattern) = pattern.content {
                    if size < 10 * 1024 * 1024 {
                        if let Ok(content_str) = fs::read_to_string(path) {
                            if let Ok(re) = regex::RegexBuilder::new(c_pattern).case_insensitive(true).build() {
                                if !re.is_match(&content_str) {
                                    matched = false;
                                }
                            }
                        } else {
                            matched = false;
                        }
                    } else {
                        matched = false;
                    }
                }
            }

            if matched {
                matches.push(ScanMatch {
                    path: rel_path,
                    size: Some(size),
                });
            }
        }

        if !matches.is_empty() {
            results.push(ScanResult {
                pattern,
                matches,
            });
        }
    }

    Ok(results)
}

pub fn zip_scan_volume(
    id: &str,
    include: Option<Vec<String>>,
    exclude: Option<Vec<String>>,
    max_file_size_mb: Option<u64>,
) -> Result<Vec<u8>, String> {
    if !validate_container_id(id) {
        return Err("Invalid container ID".to_string());
    }

    let base_dir = PathBuf::from("volumes").join(id);
    if !base_dir.exists() {
        return Err(format!("Volume directory for {} does not exist", id));
    }
    let real_base = base_dir.canonicalize().map_err(|e| e.to_string())?;

    let default_include = vec!["plugins", "mods", "config", "addons", "datapacks"];
    let hardcoded_exclude = [
        "world", "world_nether", "world_the_end",
        "logs", "cache", "crash-reports", "debug",
        "backups", ".git", "node_modules",
    ];

    let scannable_extensions = [
        ".jar", ".sh", ".bat", ".cmd", ".ps1", ".py",
        ".php", ".exe", ".dll", ".so", ".elf", ".bin",
        ".json", ".yml", ".yaml", ".toml", ".conf", ".cfg", ".ini",
        ".js", ".ts", ".rb", ".pl", ".lua",
    ];

    let target_includes: Vec<String> = include
        .unwrap_or_else(|| default_include.into_iter().map(|s| s.to_string()).collect())
        .into_iter()
        .filter(|s| !hardcoded_exclude.contains(&s.to_lowercase().as_str()))
        .collect();

    let mut target_excludes: Vec<String> = hardcoded_exclude.iter().map(|s| s.to_string()).collect();
    if let Some(user_excludes) = exclude {
        target_excludes.extend(user_excludes.into_iter().map(|s| s.to_lowercase()));
    }

    let max_size = max_file_size_mb.unwrap_or(8).min(8) * 1024 * 1024;

    let buf = Vec::new();
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(buf));
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    for folder_name in target_includes {
        let folder_path = real_base.join(&folder_name);
        if !folder_path.exists() || !folder_path.is_dir() {
            continue;
        }

        for entry in WalkDir::new(&folder_path).into_iter().filter_map(|e| e.ok()) {
            let path = entry.path();
            let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if file_name.starts_with('.') {
                continue;
            }

            if target_excludes.iter().any(|ex| file_name.eq_ignore_ascii_case(ex)) {
                continue;
            }

            if entry.file_type().is_file() {
                let has_ext = scannable_extensions.iter().any(|ext| file_name.to_lowercase().ends_with(ext));
                if !has_ext {
                    continue;
                }

                let meta = entry.metadata().ok();
                if let Some(m) = meta {
                    if m.len() > max_size {
                        continue;
                    }
                }

                if let Ok(rel) = path.strip_prefix(&real_base) {
                    let zip_name = rel.to_string_lossy().to_string();
                    if let Ok(content) = fs::read(path) {
                        let _ = zip.start_file(zip_name, options);
                        let _ = zip.write_all(&content);
                    }
                }
            }
        }
    }

    let cursor = zip.finish().map_err(|e| e.to_string())?;
    Ok(cursor.into_inner())
}
