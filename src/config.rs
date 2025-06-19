use std::fs::File;
use std::io::Read;
use std::env;
use serde::{Deserialize, Serialize};
use crate::client::Deployment;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(rename = "apiBaseUrl")]
    pub api_base_url: String,
    #[serde(rename = "configFilePath")]
    pub config_file_path: String,
    #[serde(rename = "enableConsoleOutput")]
    pub enable_console_output: bool,
    
    #[serde(rename = "deploymentTimeoutSeconds")]
    pub deployment_timeout_seconds: u64,
    #[serde(rename = "pollIntervalSeconds")]
    pub poll_interval_seconds: u64,
    #[serde(rename = "enableStatusPolling")]
    pub enable_status_polling: bool,
    #[serde(rename = "enableIpRetrieval")]
    pub enable_ip_retrieval: bool,
    
    #[serde(rename = "batchDeleteEnabled")]
    pub batch_delete_enabled: bool,
    #[serde(rename = "deleteDelaySeconds")]
    pub delete_delay_seconds: u64,
    
    #[serde(rename = "enableBulkApproach")]
    pub enable_bulk_approach: bool,
    #[serde(rename = "enableDecomposedApproach")]
    pub enable_decomposed_approach: bool,
    
    #[serde(rename = "httpTimeoutSeconds")]
    pub http_timeout_seconds: u64,
    
    pub deployments: Vec<Deployment>,
}

pub fn load_config() -> Result<Config, Box<dyn std::error::Error>> {
    let config_file_path = env::var("CONFIG_FILE_PATH")
        .unwrap_or_else(|_| "./deployment_config.yaml".to_string());
        
    let mut file = File::open(&config_file_path)?;
    let mut contents = String::new();
    file.read_to_string(&mut contents)?;
    let config: Config = serde_yaml::from_str(&contents)?;
    Ok(config)
}

pub fn load_env_config() -> Result<(String, Vec<String>, String), Box<dyn std::error::Error>> {
    if let Ok(env_content) = std::fs::read_to_string(".env") {
        for line in env_content.lines() {
            if let Some((key, value)) = line.split_once('=') {
                unsafe {
                    std::env::set_var(key.trim(), value.trim());
                }
            }
        }
    }
    
    let api_key = env::var("FLUENCE_API_KEY")
        .map_err(|_| "FLUENCE_API_KEY not found in environment or .env file")?;
    
    let ssh_keys_str = env::var("SSH_KEYS")
        .map_err(|_| "SSH_KEYS not found in environment or .env file")?;
    
    let ssh_keys: Vec<String> = ssh_keys_str
        .split(',')
        .map(|key| key.trim().to_string())
        .filter(|key| !key.is_empty())
        .collect();
    
    if ssh_keys.is_empty() {
        return Err("No valid SSH keys found in SSH_KEYS".into());
    }
    
    let log_file_path = env::var("LOG_FILE_PATH")
        .unwrap_or_else(|_| "./logs/vm_deployment.log".to_string());
    
    Ok((api_key, ssh_keys, log_file_path))
}

pub fn setup_logging(_config: &Config) -> (String, chrono::DateTime<chrono::Utc>) {
    let run_timestamp = chrono::Utc::now();
    let run_id = run_timestamp.format("%Y%m%d_%H%M%S");
    
    let log_file_path = std::env::var("LOG_FILE_PATH")
        .unwrap_or_else(|_| "./logs/vm_deployment.log".to_string());
    
    // Create logs directory if it doesn't exist
    if let Some(parent_dir) = std::path::Path::new(&log_file_path).parent() {
        if let Err(e) = std::fs::create_dir_all(parent_dir) {
            eprintln!("Warning: Could not create logs directory: {}", e);
        }
    }
    
    let log_file_with_run = if log_file_path.ends_with(".log") {
        log_file_path.replace(".log", &format!("_{}.log", run_id))
    } else {
        format!("{}_{}.log", log_file_path, run_id)
    };
    
    unsafe {
        std::env::set_var("LOG_FILE_PATH", &log_file_with_run);
    }
    
    (log_file_with_run, run_timestamp)
}

pub fn validate_config(config: &Config) -> Result<(), String> {
    if config.deployments.is_empty() {
        return Err("No deployments found in config file".to_string());
    }
    
    if !config.enable_bulk_approach && !config.enable_decomposed_approach {
        return Err("Both bulk and decomposed approaches are disabled! Enable at least one in config.".to_string());
    }
    
    Ok(())
}

pub fn get_approach_info(config: &Config) -> &'static str {
    match (config.enable_bulk_approach, config.enable_decomposed_approach) {
        (true, true) => "BOTH BULK and DECOMPOSED approaches",
        (true, false) => "BULK approach only",
        (false, true) => "DECOMPOSED approach only",
        (false, false) => unreachable!(),
    }
}