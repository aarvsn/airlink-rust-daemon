use std::env;

#[derive(Clone, Debug)]
pub struct Config {
    pub remote: String,
    pub key: String,
    pub port: u16,
    pub debug: bool,
    pub environment: String,
    pub version: String,
    pub stats_interval: u64,
    pub require_hmac: bool,
    pub allowed_ips: Vec<String>,
}

impl Config {
    pub fn load() -> Self {
        let _ = dotenvy::dotenv();

        let remote = env::var("remote").unwrap_or_else(|_| "127.0.0.1".to_string());
        let key = env::var("key").unwrap_or_else(|_| "00000000000000000000000000000000".to_string());
        let port = env::var("port")
            .ok()
            .and_then(|p| p.parse::<u16>().ok())
            .unwrap_or(3002);
        let debug = env::var("DEBUG")
            .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
            .unwrap_or(false);
        let environment = env::var("environment").unwrap_or_else(|_| "development".to_string());
        let version = env::var("version").unwrap_or_else(|_| "1.0.0".to_string());
        let stats_interval = env::var("STATS_INTERVAL")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(10000);
        let require_hmac = env::var("REQUIRE_HMAC")
            .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
            .unwrap_or(false);

        let allowed_ips = env::var("ALLOWED_IPS")
            .unwrap_or_default()
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        Self {
            remote,
            key,
            port,
            debug,
            environment,
            version,
            stats_interval,
            require_hmac,
            allowed_ips,
        }
    }
}
