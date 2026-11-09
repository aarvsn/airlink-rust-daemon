use std::collections::HashMap;
use std::fs;
use std::path::Path;
use clap::Parser;
use colored::Colorize;

#[derive(Parser, Debug)]
#[command(name = "configure", about = "Configure Airlink Daemon with Panel")]
struct Args {
    #[arg(short = 'p', long = "panel")]
    panel: String,

    #[arg(short = 'k', long = "key")]
    key: String,
}

async fn validate_panel_url(url: &str) -> bool {
    let check_url = format!("{}/", url.trim_end_matches('/'));
    if let Ok(resp) = reqwest::get(&check_url).await {
        resp.status().is_success()
    } else {
        false
    }
}

fn update_env_file(panel_url: &str, key: &str) -> Result<(), String> {
    let env_path = Path::new(".env");
    let content = fs::read_to_string(env_path).unwrap_or_default();

    let mut env_map = HashMap::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = trimmed.split_once('=') {
            let val = v.trim_matches('"').trim_matches('\'');
            env_map.insert(k.trim().to_string(), val.to_string());
        }
    }

    let remote_ip = panel_url
        .trim_start_matches("http://")
        .trim_start_matches("https://")
        .split(':')
        .next()
        .unwrap_or("127.0.0.1")
        .split('/')
        .next()
        .unwrap_or("127.0.0.1");

    env_map.insert("remote".to_string(), remote_ip.to_string());
    env_map.insert("key".to_string(), key.to_string());
    env_map.entry("version".to_string()).or_insert_with(|| "1.0.0".to_string());
    env_map.entry("port".to_string()).or_insert_with(|| "3002".to_string());

    let mut lines = Vec::new();
    for (k, v) in env_map {
        lines.push(format!("{}={}", k, v));
    }

    fs::write(env_path, lines.join("\n") + "\n").map_err(|e| e.to_string())
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    let panel_url = args.panel.trim_end_matches('/');

    println!("{}", "🔍 Validating panel URL...".blue());
    if !validate_panel_url(panel_url).await {
        eprintln!("{}", "❌ Invalid panel URL or panel is not responding".red());
        std::process::exit(1);
    }

    println!("{}", "✅ Panel URL is valid".green());
    println!("{}", "📝 Updating configuration...".blue());

    if let Err(e) = update_env_file(panel_url, &args.key) {
        eprintln!("{} {}", "❌ Error updating configuration:".red(), e);
        std::process::exit(1);
    }

    println!("{}", "✅ Configuration updated successfully".green());
    println!("{}", "ℹ️ New configuration:".blue());
    println!("{}", format!("Panel URL: {}", panel_url).cyan());
    println!("{}", format!("Daemon Key: {}", args.key).cyan());
}
