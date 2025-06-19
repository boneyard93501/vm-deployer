mod client;
mod deployment;
mod config;
mod analysis;
mod cleanup;

use std::sync::{Arc, mpsc, atomic::{AtomicBool, Ordering}};
use std::thread;
use std::time::{Duration, Instant};

use client::FluenceClient;
use deployment::{deploy_vm_bulk_worker, deploy_vm_decomposed_worker, create_logger_thread, create_vm_polling_thread, DeploymentResult, LogMessage};
use config::{load_config, load_env_config, setup_logging, validate_config, get_approach_info};
use analysis::{analyze_results, log_vm_status_report, log_summary, log_approach_results, log_performance_comparison};
use cleanup::{emergency_cleanup, normal_cleanup};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Setup signal handling
    let shutdown_flag = Arc::new(AtomicBool::new(false));
    let shutdown_flag_clone = Arc::clone(&shutdown_flag);
    
    ctrlc::set_handler(move || {
        println!("\n🛑 Ctrl+C detected! Initiating graceful shutdown...");
        shutdown_flag_clone.store(true, Ordering::SeqCst);
    })?;

    // Load configuration
    let (api_key, default_ssh_keys, _log_file_path) = match load_env_config() {
        Ok(config) => config,
        Err(e) => {
            eprintln!("Error: {}", e);
            eprintln!("Create a .env file with:");
            eprintln!("FLUENCE_API_KEY=your_api_key_here");
            eprintln!("SSH_KEYS=ssh-rsa AAAAB3... user@host,ssh-ed25519 AAAAC3... user@host2");
            eprintln!("LOG_FILE_PATH=./vm_deployment.log  # optional");
            return Ok(());
        }
    };

    println!("DEBUG: Loaded {} SSH keys from .env", default_ssh_keys.len());
    println!("DEBUG: First SSH key preview: {}", 
        if default_ssh_keys.is_empty() { 
            "NONE!".to_string() 
        } else { 
            default_ssh_keys[0].chars().take(20).collect::<String>() + "..." 
        });
    
    if default_ssh_keys.is_empty() {
        eprintln!("ERROR: No SSH keys found in .env file!");
        return Ok(());
    }

    let mut config = match load_config() {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("Error: Failed to load config file: {}", e);
            eprintln!("Create your own deployment_config.yaml file");
            return Ok(());
        }
    };

    // Validate configuration
    if let Err(e) = validate_config(&config) {
        eprintln!("Configuration error: {}", e);
        return Ok(());
    }

    // Setup logging
    let log_tx = create_logger_thread(config.enable_console_output);
    let (log_file_with_run, run_timestamp) = setup_logging(&config);
    
    // Log configuration
    log_configuration(&config, &log_file_with_run, &run_timestamp, &log_tx)?;
    
    // Inject SSH keys
    for deployment in &mut config.deployments {
        if deployment.vm_config.ssh_keys.is_empty() {
            deployment.vm_config.ssh_keys = default_ssh_keys.clone();
            log_tx.send(LogMessage::Info(format!(
                "Using default SSH keys from .env for deployment '{}'", 
                deployment.vm_config.name
            )))?;
        }
    }

    println!("DEBUG: Config after SSH key injection: {:#?}", config.deployments[0]);
    
    // Start deployment
    let approach_info = get_approach_info(&config);
    log_tx.send(LogMessage::Info(format!("Running: {} deployments with {}", config.deployments.len(), approach_info)))?;
    
    let deployment_start = Instant::now();
    let client = FluenceClient::new(&api_key, &config.api_base_url, config.http_timeout_seconds);
    let (result_tx, result_rx) = mpsc::channel::<DeploymentResult>();
    
    let polling_tx = if config.enable_status_polling {
        Some(create_vm_polling_thread(
            client.clone(),
            config.deployment_timeout_seconds,
            config.poll_interval_seconds,
            config.enable_ip_retrieval,
            log_tx.clone(),
        ))
    } else {
        None
    };
    
    let vm_deletion_queue: Arc<std::sync::Mutex<Vec<String>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
    let vm_queue_for_cleanup = Arc::clone(&vm_deletion_queue);
    
    // Launch deployment threads
    let handles = launch_deployment_threads(&config, &client, &vm_deletion_queue, &polling_tx, &result_tx, &log_tx);
    drop(result_tx);
    
    // Collect results
    let results = collect_results(result_rx, &config, &shutdown_flag, &log_tx)?;
    
    // Wait for threads to finish
    if shutdown_flag.load(Ordering::SeqCst) {
        log_tx.send(LogMessage::Info("Waiting for threads to finish...".to_string()))?;
    }
    
    for handle in handles {
        let _ = handle.join();
    }
    
    // Handle shutdown
    if shutdown_flag.load(Ordering::SeqCst) {
        emergency_cleanup(vm_queue_for_cleanup, &client, &log_tx)?;
        log_tx.send(LogMessage::Shutdown)?;
        thread::sleep(Duration::from_millis(100));
        return Ok(());
    }
    
    // Analyze and report results
    let total_deployment_time = deployment_start.elapsed();
    let analysis = analyze_results(&results);
    
    log_tx.send(LogMessage::Info("=".repeat(80)))?;
    log_tx.send(LogMessage::Info("DEPLOYMENT ANALYSIS".to_string()))?;
    log_tx.send(LogMessage::Info("=".repeat(80)))?;
    
    log_vm_status_report(&analysis, &results, &log_tx)?;
    log_summary(&analysis, &results, &log_tx)?;
    log_approach_results(&analysis, &log_tx)?;
    log_performance_comparison(&analysis, total_deployment_time, &log_tx)?;

    // Cleanup VMs
    normal_cleanup(&config, vm_deletion_queue, &client, &log_tx)?;

    // Finish
    log_tx.send(LogMessage::Info("=".repeat(80)))?;
    log_tx.send(LogMessage::Info(format!("FLUENCE VM DEPLOYMENT COMPLETED - {}", chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC"))))?;
    log_tx.send(LogMessage::Info("=".repeat(80)))?;

    log_tx.send(LogMessage::Shutdown)?;
    thread::sleep(Duration::from_millis(100));

    Ok(())
}

fn log_configuration(
    config: &config::Config,
    log_file_with_run: &str,
    run_timestamp: &chrono::DateTime<chrono::Utc>,
    log_tx: &mpsc::Sender<LogMessage>,
) -> Result<(), Box<dyn std::error::Error>> {
    log_tx.send(LogMessage::Info("=".repeat(120)))?;
    log_tx.send(LogMessage::Info(format!("FLUENCE VM DEPLOYMENT RUN - {}", run_timestamp.format("%Y-%m-%d %H:%M:%S UTC"))))?;
    log_tx.send(LogMessage::Info(format!("LOG FILE: {}", log_file_with_run)))?;
    log_tx.send(LogMessage::Info("=".repeat(120)))?;
    log_tx.send(LogMessage::Info("CONFIGURATION:".to_string()))?;
    log_tx.send(LogMessage::Info(format!("  API URL: {}", config.api_base_url)))?;
    log_tx.send(LogMessage::Info(format!("  Timeout: {}s", config.deployment_timeout_seconds)))?;
    log_tx.send(LogMessage::Info(format!("  Poll Interval: {}s", config.poll_interval_seconds)))?;
    log_tx.send(LogMessage::Info(format!("  Status Polling: {}", config.enable_status_polling)))?;
    log_tx.send(LogMessage::Info(format!("  IP Retrieval: {}", config.enable_ip_retrieval)))?;
    log_tx.send(LogMessage::Info(format!("  Batch Delete: {}", config.batch_delete_enabled)))?;
    log_tx.send(LogMessage::Info(format!("  Bulk Approach: {}", config.enable_bulk_approach)))?;
    log_tx.send(LogMessage::Info(format!("  Decomposed Approach: {}", config.enable_decomposed_approach)))?;
    log_tx.send(LogMessage::Info("".to_string()))?;
    
    for (i, deployment) in config.deployments.iter().enumerate() {
        log_tx.send(LogMessage::Info(format!("  Deployment {}:", i + 1)))?;
        log_tx.send(LogMessage::Info(format!("    Name: {}", deployment.vm_config.name)))?;
        log_tx.send(LogMessage::Info(format!("    Instances: {}", deployment.instances)))?;
        log_tx.send(LogMessage::Info(format!("    Config: {}", deployment.vm_config.basic_configuration)))?;
        log_tx.send(LogMessage::Info(format!("    OS Image: {}", deployment.vm_config.os_image)))?;
        log_tx.send(LogMessage::Info(format!("    SSH Keys: {} keys", deployment.vm_config.ssh_keys.len())))?;
        if let Some(ref datacenter) = deployment.datacenter {
            log_tx.send(LogMessage::Info(format!("    Countries: {:?}", datacenter.countries)))?;
        }
        if let Some(price) = deployment.max_price_per_epoch_usd {
            log_tx.send(LogMessage::Info(format!("    Max Price: ${:.2}", price)))?;
        }
        log_tx.send(LogMessage::Info("".to_string()))?;
    }
    
    log_tx.send(LogMessage::Info("=".repeat(80)))?;
    log_tx.send(LogMessage::Info("DEPLOYMENT STARTED".to_string()))?;
    log_tx.send(LogMessage::Info("=".repeat(80)))?;

    Ok(())
}

fn launch_deployment_threads(
    config: &config::Config,
    client: &FluenceClient,
    vm_deletion_queue: &Arc<std::sync::Mutex<Vec<String>>>,
    polling_tx: &Option<mpsc::Sender<deployment::VmPollingRequest>>,
    result_tx: &mpsc::Sender<DeploymentResult>,
    log_tx: &mpsc::Sender<LogMessage>,
) -> Vec<thread::JoinHandle<()>> {
    let mut handles = Vec::with_capacity(config.deployments.len() * 2);
    
    for deployment_config in config.deployments.clone() {
        let polling_config = (
            config.enable_status_polling,
            config.enable_ip_retrieval,
            config.deployment_timeout_seconds,
            config.poll_interval_seconds,
        );
        
        if config.enable_bulk_approach {
            let client_clone = client.clone();
            let result_tx_clone = result_tx.clone();
            let log_tx_clone = log_tx.clone();
            let vm_queue_clone = Arc::clone(vm_deletion_queue);
            let config_clone = deployment_config.clone();
            let polling_tx_clone = polling_tx.clone();
            
            let handle = thread::spawn(move || {
                deploy_vm_bulk_worker(
                    config_clone, 
                    client_clone, 
                    polling_config,
                    vm_queue_clone,
                    polling_tx_clone,
                    result_tx_clone, 
                    log_tx_clone
                );
            });
            
            handles.push(handle);
        }
        
        if config.enable_decomposed_approach {
            let client_clone = client.clone();
            let result_tx_clone = result_tx.clone();
            let log_tx_clone = log_tx.clone();
            let vm_queue_clone = Arc::clone(vm_deletion_queue);
            let config_clone = deployment_config.clone();
            let polling_tx_clone = polling_tx.clone();
            
            let handle = thread::spawn(move || {
                deploy_vm_decomposed_worker(
                    config_clone, 
                    client_clone, 
                    polling_config,
                    vm_queue_clone,
                    polling_tx_clone,
                    result_tx_clone, 
                    log_tx_clone
                );
            });
            
            handles.push(handle);
        }
    }
    
    handles
}

fn collect_results(
    result_rx: mpsc::Receiver<DeploymentResult>,
    config: &config::Config,
    shutdown_flag: &Arc<AtomicBool>,
    log_tx: &mpsc::Sender<LogMessage>,
) -> Result<Vec<DeploymentResult>, Box<dyn std::error::Error>> {
    let mut results = Vec::with_capacity(config.deployments.len() * 2);
    let expected_results = if config.enable_bulk_approach && config.enable_decomposed_approach {
        config.deployments.len() * 2
    } else {
        config.deployments.len()
    };
    
    while results.len() < expected_results {
        if shutdown_flag.load(Ordering::SeqCst) {
            log_tx.send(LogMessage::Error("🛑 SHUTDOWN SIGNAL RECEIVED - Stopping deployment...".to_string()))?;
            break;
        }
        
        match result_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(result) => results.push(result),
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    
    Ok(results)
}