use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};
use std::thread;
use crate::client::FluenceClient;
use crate::deployment::LogMessage;
use crate::config::Config;

pub fn emergency_cleanup(
    vm_queue: Arc<std::sync::Mutex<Vec<String>>>,
    client: &FluenceClient,
    log_tx: &mpsc::Sender<LogMessage>,
) -> Result<(), Box<dyn std::error::Error>> {
    let vm_ids_to_cleanup = {
        let queue = vm_queue.lock().unwrap();
        queue.clone()
    };

    if !vm_ids_to_cleanup.is_empty() {
        log_tx.send(LogMessage::Info("=".repeat(80)))?;
        log_tx.send(LogMessage::Info("🚨 EMERGENCY VM CLEANUP (Ctrl+C)".to_string()))?;
        log_tx.send(LogMessage::Info("=".repeat(80)))?;
        
        log_tx.send(LogMessage::Info(format!(
            "Emergency deleting {} VMs: {:?}",
            vm_ids_to_cleanup.len(), vm_ids_to_cleanup
        )))?;

        let deletion_start = Instant::now();
        match client.delete_vms(&vm_ids_to_cleanup) {
            Ok(()) => {
                let deletion_time = deletion_start.elapsed();
                log_tx.send(LogMessage::Success(format!(
                    "✅ Emergency deleted {} VMs in {:.6}s",
                    vm_ids_to_cleanup.len(), deletion_time.as_secs_f64()
                )))?;
            }
            Err(error) => {
                log_tx.send(LogMessage::Error(format!(
                    "❌ Failed emergency VM deletion: {}",
                    error
                )))?;
                log_tx.send(LogMessage::Error(format!(
                    "⚠️  WARNING: {} VMs may still be running and charging!",
                    vm_ids_to_cleanup.len()
                )))?;
            }
        }
    }

    log_tx.send(LogMessage::Info("🛑 Graceful shutdown completed".to_string()))?;
    Ok(())
}

pub fn normal_cleanup(
    config: &Config,
    vm_queue: Arc<std::sync::Mutex<Vec<String>>>,
    client: &FluenceClient,
    log_tx: &mpsc::Sender<LogMessage>,
) -> Result<(), Box<dyn std::error::Error>> {
    if config.batch_delete_enabled {
        let vm_ids_to_delete = {
            let queue = vm_queue.lock().unwrap();
            queue.clone()
        };

        if !vm_ids_to_delete.is_empty() {
            log_tx.send(LogMessage::Info("=".repeat(80)))?;
            log_tx.send(LogMessage::Info("VM CLEANUP".to_string()))?;
            log_tx.send(LogMessage::Info("=".repeat(80)))?;
            
            if config.delete_delay_seconds > 0 {
                log_tx.send(LogMessage::Info(format!(
                    "Waiting {} seconds before deleting {} VMs...",
                    config.delete_delay_seconds, vm_ids_to_delete.len()
                )))?;
                thread::sleep(Duration::from_secs(config.delete_delay_seconds));
            }

            log_tx.send(LogMessage::Info(format!(
                "Deleting {} VMs: {:?}",
                vm_ids_to_delete.len(), vm_ids_to_delete
            )))?;

            let deletion_start = Instant::now();
            match client.delete_vms(&vm_ids_to_delete) {
                Ok(()) => {
                    let deletion_time = deletion_start.elapsed();
                    log_tx.send(LogMessage::Success(format!(
                        "Successfully deleted {} VMs in {:.6}s",
                        vm_ids_to_delete.len(), deletion_time.as_secs_f64()
                    )))?;
                }
                Err(error) => {
                    log_tx.send(LogMessage::Error(format!(
                        "Failed to delete VMs: {}",
                        error
                    )))?;
                }
            }
        } else {
            log_tx.send(LogMessage::Info("No VMs to delete".to_string()))?;
        }
    } else {
        let vm_ids_count = {
            let queue = vm_queue.lock().unwrap();
            queue.len()
        };
        if vm_ids_count > 0 {
            log_tx.send(LogMessage::Info(format!(
                "Batch deletion disabled. {} VMs left running (will continue charging)",
                vm_ids_count
            )))?;
        }
    }

    Ok(())
}