#[cfg(test)]
mod tests {
    use crate::validation::*;
    use crate::storage::*;

    #[test]
    fn test_validate_container_id() {
        assert!(validate_container_id("test-container-1"));
        assert!(validate_container_id("abcdef1234567890"));
        assert!(validate_container_id("my_server"));
        assert!(!validate_container_id(""));
        assert!(!validate_container_id("invalid/id"));
        assert!(!validate_container_id("invalid..id"));
        assert!(!validate_container_id("invalid$id"));
    }

    #[test]
    fn test_validate_path() {
        assert!(validate_path("/"));
        assert!(validate_path("server/plugins"));
        assert!(!validate_path(""));
        assert!(!validate_path("../traversal"));
        assert!(!validate_path("windows\\path"));
    }

    #[test]
    fn test_validate_file_name() {
        assert!(validate_file_name("server.jar"));
        assert!(validate_file_name("config.yml"));
        assert!(!validate_file_name(""));
        assert!(!validate_file_name(".."));
        assert!(!validate_file_name("con"));
        assert!(!validate_file_name("nul"));
        assert!(!validate_file_name("file?.txt"));
    }

    #[test]
    fn test_file_category() {
        assert_eq!(get_file_category("rs"), Some("Code".to_string()));
        assert_eq!(get_file_category("json"), Some("Configuration Files".to_string()));
        assert_eq!(get_file_category("unknown_ext_xyz"), None);
    }
}
