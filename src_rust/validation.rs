pub fn validate_container_id(id: &str) -> bool {
    if id.is_empty() || id.len() > 64 {
        return false;
    }
    id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

pub fn validate_path(relative_path: &str) -> bool {
    if relative_path.is_empty() {
        return false;
    }
    if relative_path.contains("..") || relative_path.contains('\\') {
        return false;
    }
    true
}

pub fn validate_file_name(file_name: &str) -> bool {
    if file_name.is_empty() {
        return false;
    }
    if file_name.contains("..") || file_name.chars().any(|c| matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*')) {
        return false;
    }
    let upper = file_name.to_ascii_uppercase();
    let reserved = ["CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9"];
    if reserved.contains(&upper.as_str()) {
        return false;
    }
    true
}
