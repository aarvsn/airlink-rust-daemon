use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use std::collections::HashMap;
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;
use zip::write::SimpleFileOptions;

use crate::storage::get_file_category;
use crate::validation::validate_container_id;

pub struct FsManager {
    request_cache: Mutex<HashMap<String, RateData>>,
}

struct RateData {
    last_request: Instant,
    count: usize,
    cache: Option<Vec<DirectoryItem>>,
    path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectoryItem {
    pub name: String,
    #[serde(rename = "type")]
    pub item_type: String,
    pub extension: Option<String>,
    pub category: Option<String>,
    pub size: Option<u64>,
}

impl FsManager {
    pub fn new() -> Self {
        Self {
            request_cache: Mutex::new(HashMap::new()),
        }
    }

    pub fn sanitize_path(&self, container_id: &str, relative_path: &str) -> Result<PathBuf, String> {
        if !validate_container_id(container_id) {
            return Err("Invalid container ID format.".to_string());
        }

        let base_dir = std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join("volumes")
            .join(container_id);

        if !base_dir.exists() {
            fs::create_dir_all(&base_dir).map_err(|e| e.to_string())?;
        }

        let real_base = base_dir.canonicalize().map_err(|e| e.to_string())?;

        let clean_relative = relative_path.trim_start_matches('/').trim_start_matches('\\');
        let full_path = real_base.join(clean_relative);

        // Disallow path traversal
        let mut current = real_base.clone();
        for component in Path::new(clean_relative).components() {
            match component {
                std::path::Component::Normal(c) => {
                    current.push(c);
                    if current.is_symlink() {
                        return Err(format!("Invalid path: Symlinks are not allowed ({:?}).", current));
                    }
                }
                std::path::Component::ParentDir => {
                    return Err("Invalid path: Directory traversal is not allowed.".to_string());
                }
                _ => {}
            }
        }

        if full_path.exists() {
            let canonical = full_path.canonicalize().map_err(|e| e.to_string())?;
            if !canonical.starts_with(&real_base) {
                return Err("Invalid path: escapes base directory".to_string());
            }
            Ok(canonical)
        } else {
            // Check parent
            if let Some(parent) = full_path.parent() {
                if parent.exists() {
                    let parent_canon = parent.canonicalize().map_err(|e| e.to_string())?;
                    if !parent_canon.starts_with(&real_base) {
                        return Err("Invalid path: escapes base directory".to_string());
                    }
                }
            }
            Ok(full_path)
        }
    }

    pub fn get_directory_size(&self, path: &Path, depth: usize, max_depth: usize) -> u64 {
        if depth > max_depth || !path.exists() {
            return 0;
        }

        let mut total = 0;
        if let Ok(entries) = fs::read_dir(path) {
            for entry in entries.flatten() {
                if let Ok(ft) = entry.file_type() {
                    if ft.is_symlink() {
                        continue;
                    }
                    if entry.file_name() == "node_modules" {
                        continue;
                    }
                    if ft.is_dir() {
                        total += self.get_directory_size(&entry.path(), depth + 1, max_depth);
                    } else if ft.is_file() {
                        if let Ok(meta) = entry.metadata() {
                            total += meta.len();
                        }
                    }
                }
            }
        }
        total
    }

    pub fn list(&self, container_id: &str, relative_path: &str, filter: Option<&str>) -> Result<Vec<DirectoryItem>, String> {
        let now = Instant::now();
        {
            let mut cache_map = self.request_cache.lock().unwrap();
            let entry = cache_map.entry(container_id.to_string()).or_insert_with(|| RateData {
                last_request: now,
                count: 0,
                cache: None,
                path: relative_path.to_string(),
            });

            if let Some(ref cached_data) = entry.cache {
                if now.duration_since(entry.last_request) < Duration::from_secs(1) && entry.path == relative_path {
                    return Ok(cached_data.clone());
                }
            }

            if now.duration_since(entry.last_request) < Duration::from_secs(1) {
                entry.count += 1;
            } else {
                entry.count = 1;
            }

            entry.last_request = now;
            entry.path = relative_path.to_string();

            if entry.count > 5 {
                return Err("Too many requests, please wait 3 seconds.".to_string());
            }
        }

        let target_dir = self.sanitize_path(container_id, relative_path)?;
        if !target_dir.is_dir() {
            return Err("Target path is not a directory".to_string());
        }

        let mut results = Vec::new();
        let entries = fs::read_dir(&target_dir).map_err(|e| format!("Error listing directory: {}", e))?;

        for entry in entries.flatten() {
            let file_name = entry.file_name().to_string_lossy().to_string();
            let file_path = entry.path();
            let is_dir = file_path.is_dir();

            let ext = Path::new(&file_name)
                .extension()
                .map(|e| e.to_string_lossy().to_string());
            let category = ext.as_deref().and_then(get_file_category);

            let size = if is_dir {
                Some(self.get_directory_size(&file_path, 0, 20))
            } else {
                entry.metadata().ok().map(|m| m.len())
            };

            results.push(DirectoryItem {
                name: file_name,
                item_type: if is_dir { "directory".to_string() } else { "file".to_string() },
                extension: if is_dir { None } else { ext },
                category: if is_dir { None } else { category },
                size,
            });
        }

        if results.len() > 256 {
            results.truncate(256);
        }

        if let Some(f) = filter {
            results.retain(|item| item.name.contains(f));
        }

        let mut cache_map = self.request_cache.lock().unwrap();
        if let Some(entry) = cache_map.get_mut(container_id) {
            entry.cache = Some(results.clone());
        }

        Ok(results)
    }

    pub fn read_file_content(&self, container_id: &str, relative_path: &str) -> Result<String, String> {
        let full_path = self.sanitize_path(container_id, relative_path)?;
        if !full_path.is_file() {
            return Err("Path is not a file".to_string());
        }
        fs::read_to_string(&full_path).map_err(|e| e.to_string())
    }

    pub fn write_file_content(&self, container_id: &str, relative_path: &str, content: &[u8]) -> Result<(), String> {
        let full_path = self.sanitize_path(container_id, relative_path)?;
        if let Some(parent) = full_path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        fs::write(&full_path, content).map_err(|e| e.to_string())
    }

    pub fn append_file_content(&self, container_id: &str, relative_path: &str, content: &[u8]) -> Result<(), String> {
        let full_path = self.sanitize_path(container_id, relative_path)?;
        if let Some(parent) = full_path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&full_path)
            .map_err(|e| e.to_string())?;
        file.write_all(content).map_err(|e| e.to_string())
    }

    pub fn remove(&self, container_id: &str, relative_path: &str) -> Result<(), String> {
        let trimmed = relative_path.trim_matches('/');
        if trimmed.is_empty() {
            return Err("Root directory cannot be deleted.".to_string());
        }
        let full_path = self.sanitize_path(container_id, relative_path)?;
        if !full_path.exists() {
            return Err("Path does not exist".to_string());
        }
        if full_path.is_dir() {
            fs::remove_dir_all(&full_path).map_err(|e| e.to_string())
        } else if full_path.is_file() || full_path.is_symlink() {
            fs::remove_file(&full_path).map_err(|e| e.to_string())
        } else {
            Err("Path is neither a file nor a directory.".to_string())
        }
    }

    pub fn rename(&self, container_id: &str, old_path: &str, new_path: &str) -> Result<(), String> {
        let old_full = self.sanitize_path(container_id, old_path)?;
        let new_full = self.sanitize_path(container_id, new_path)?;

        if !old_full.exists() {
            return Err("Source file does not exist".to_string());
        }
        if new_full.exists() {
            let is_file = old_full.is_file();
            return Err(if is_file { "File already exists" } else { "Folder already exists" }.to_string());
        }

        if let Some(parent) = new_full.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }

        fs::rename(old_full, new_full).map_err(|e| e.to_string())
    }

    pub fn zip_files(&self, container_id: &str, file_paths: &[String], zipname: &str) -> Result<PathBuf, String> {
        let base_dir = self.sanitize_path(container_id, "/")?;
        let first_file_rel = file_paths.first().ok_or_else(|| "No files specified".to_string())?;
        let first_file_path = self.sanitize_path(container_id, first_file_rel)?;

        let first_dir = if first_file_path.is_dir() {
            first_file_path.parent().unwrap_or(&base_dir).to_path_buf()
        } else {
            first_file_path.parent().unwrap_or(&base_dir).to_path_buf()
        };

        let zip_path = first_dir.join(format!("{}.zip", zipname));
        let file = fs::File::create(&zip_path).map_err(|e| e.to_string())?;
        let mut zip = zip::ZipWriter::new(file);
        let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

        for path_str in file_paths {
            let full_p = self.sanitize_path(container_id, path_str)?;
            if !full_p.exists() {
                continue;
            }
            if full_p.is_file() {
                let name = path_str.trim_start_matches('/');
                zip.start_file(name, options).map_err(|e| e.to_string())?;
                let mut f = fs::File::open(&full_p).map_err(|e| e.to_string())?;
                io::copy(&mut f, &mut zip).map_err(|e| e.to_string())?;
            } else if full_p.is_dir() {
                let prefix = path_str.trim_start_matches('/');
                for entry in WalkDir::new(&full_p).into_iter().filter_map(|e| e.ok()) {
                    let p = entry.path();
                    let rel = p.strip_prefix(&full_p).map_err(|e| e.to_string())?;
                    let zip_entry_name = if rel.as_os_str().is_empty() {
                        prefix.to_string()
                    } else {
                        format!("{}/{}", prefix, rel.to_string_lossy())
                    };

                    if entry.file_type().is_dir() {
                        zip.add_directory(&zip_entry_name, options).map_err(|e| e.to_string())?;
                    } else if entry.file_type().is_file() {
                        zip.start_file(&zip_entry_name, options).map_err(|e| e.to_string())?;
                        let mut f = fs::File::open(p).map_err(|e| e.to_string())?;
                        io::copy(&mut f, &mut zip).map_err(|e| e.to_string())?;
                    }
                }
            }
        }

        zip.finish().map_err(|e| e.to_string())?;
        Ok(zip_path)
    }

    pub fn unzip_file(&self, container_id: &str, relative_path: &str, zipname: &str) -> Result<(), String> {
        let base_dir = self.sanitize_path(container_id, relative_path)?;
        let archive_path = base_dir.join(zipname);

        if !archive_path.exists() {
            return Err(format!("File not found: {:?}", archive_path));
        }

        let extract_path = archive_path.parent().unwrap_or(&base_dir);

        let file = fs::File::open(&archive_path).map_err(|e| e.to_string())?;
        let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("Error during unzip: {}", e))?;

        for i in 0..archive.len() {
            let mut file = archive.by_index(i).map_err(|e| e.to_string())?;
            let outpath = match file.enclosed_name() {
                Some(path) => extract_path.join(path),
                None => continue,
            };

            if file.name().ends_with('/') {
                fs::create_dir_all(&outpath).map_err(|e| e.to_string())?;
            } else {
                if let Some(p) = outpath.parent() {
                    if !p.exists() {
                        fs::create_dir_all(p).map_err(|e| e.to_string())?;
                    }
                }
                let mut outfile = fs::File::create(&outpath).map_err(|e| e.to_string())?;
                io::copy(&mut file, &mut outfile).map_err(|e| e.to_string())?;
            }
        }

        Ok(())
    }
}
