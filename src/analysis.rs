use std::sync::mpsc;
use std::time::Duration;
use crate::deployment::{DeploymentResult, LogMessage};

pub struct AnalysisResult<'a> {
    pub launched_vms: Vec<(String, String, String, Option<String>, String)>,
    pub failed_vms: Vec<(String, String, String)>,
    pub total_requested: usize,
    pub bulk_results: Vec<&'a DeploymentResult>,
    pub decomposed_results: Vec<&'a DeploymentResult>,
}

pub fn analyze_results(results: &[DeploymentResult]) -> AnalysisResult {
    let mut bulk_results = Vec::new();
    let mut decomposed_results = Vec::new();

    for result in results {
        if result.approach == "bulk" {
            bulk_results.push(result);
        } else {
            decomposed_results.push(result);
        }
    }
    
    let mut launched_vms = Vec::new();
    let mut failed_vms = Vec::new();

    for result in results {
        if let Some(ref vm_responses) = result.vm_responses {
            for vm in vm_responses {
                let (status_info, _) = if let Some(ref statuses) = result.vm_statuses {
                    if let Some(status) = statuses.iter().find(|s| s.id == vm.vm_id) {
                        ((true, status.status.clone(), status.public_ip.clone()), ())
                    } else {
                        ((false, "TIMEOUT/FAILED".to_string(), None), ())
                    }
                } else {
                    ((false, "NO_POLLING".to_string(), None), ())
                };
                
                let (launched, _status, ip) = status_info;
                
                if launched {
                    let total_time_str = if let Some(ref vm_timings) = result.vm_phase_timings {
                        if let Some(timing) = vm_timings.get(&vm.vm_id) {
                            timing.get_total_duration()
                                .map(|d| format!(" - {:.1}s total", d.as_secs_f64()))
                                .unwrap_or_else(|| String::new())
                        } else {
                            String::new()
                        }
                    } else {
                        String::new()
                    };
                    
                    launched_vms.push((vm.vm_id.clone(), vm.vm_name.clone(), result.approach.clone(), ip.clone(), total_time_str));
                } else {
                    failed_vms.push((vm.vm_id.clone(), vm.vm_name.clone(), result.approach.clone()));
                }
            }
        }
    }
    
    let total_requested = calculate_total_requested(results);

    AnalysisResult {
        launched_vms,
        failed_vms,
        total_requested,
        bulk_results,
        decomposed_results,
    }
}

fn calculate_total_requested(results: &[DeploymentResult]) -> usize {
    results.iter()
        .map(|r| {
            if r.vm_responses.is_some() {
                r.vm_responses.as_ref().unwrap().len()
            } else {
                if r.approach == "decomposed" {
                    1
                } else {
                    3 // bulk default
                }
            }
        })
        .sum::<usize>()
}

pub fn log_vm_status_report(_analysis: &AnalysisResult, results: &[DeploymentResult], log_tx: &mpsc::Sender<LogMessage>) -> Result<(), Box<dyn std::error::Error>> {
    log_tx.send(LogMessage::Info("=".repeat(80)))?;
    log_tx.send(LogMessage::Info("DEPLOYMENT RESULTS".to_string()))?;
    log_tx.send(LogMessage::Info("=".repeat(80)))?;
    
    // Show each thread result clearly
    for result in results {
        let status_symbol = if result.success { "✅" } else { "❌" };
        let intended = if result.approach == "bulk" { 3 } else { 1 };
        let actual = result.vm_responses.as_ref().map(|v| v.len()).unwrap_or(0);
        let launched = result.vm_statuses.as_ref().map(|v| v.len()).unwrap_or(0);
        
        log_tx.send(LogMessage::Info(format!(
            "{} {} [Thread {:?}] - {:.3}s - Intended: {} | Requested: {} | Launched: {}",
            status_symbol,
            result.approach.to_uppercase(),
            result.thread_id,
            result.duration.as_secs_f64(),
            intended,
            actual,
            launched
        )))?;
        
        if !result.success {
            log_tx.send(LogMessage::Error(format!(
                "   ERROR: {}",
                result.error_message.as_ref().unwrap_or(&"Unknown error".to_string())
            )))?;
        }
        
        // Show individual VMs if any launched
        if let Some(ref vm_responses) = result.vm_responses {
            for vm in vm_responses {
                if let Some(ref statuses) = result.vm_statuses {
                    if let Some(status) = statuses.iter().find(|s| s.id == vm.vm_id) {
                        let datacenter = status.datacenter.as_ref()
                            .and_then(|dc| dc.as_str().or_else(|| dc.get("name").and_then(|n| n.as_str())))
                            .unwrap_or("Unknown");
                        
                        let timing = if let Some(ref vm_timings) = result.vm_phase_timings {
                            if let Some(timing) = vm_timings.get(&vm.vm_id) {
                                timing.get_total_duration()
                                    .map(|d| format!(" ({:.1}s)", d.as_secs_f64()))
                                    .unwrap_or_else(|| String::new())
                            } else {
                                String::new()
                            }
                        } else {
                            String::new()
                        };
                        
                        log_tx.send(LogMessage::Success(format!(
                            "   ✓ {} [{}] - {} - {}{}",
                            vm.vm_name,
                            vm.vm_id,
                            status.public_ip.as_ref().unwrap_or(&"No IP".to_string()),
                            datacenter,
                            timing
                        )))?;
                    }
                }
            }
        }
        
        log_tx.send(LogMessage::Info("".to_string()))?;
    }
    
    Ok(())
}

pub fn log_summary(analysis: &AnalysisResult, _results: &[DeploymentResult], log_tx: &mpsc::Sender<LogMessage>) -> Result<(), Box<dyn std::error::Error>> {
    log_tx.send(LogMessage::Info("=".repeat(80)))?;
    log_tx.send(LogMessage::Info("SUMMARY".to_string()))?;
    log_tx.send(LogMessage::Info("=".repeat(80)))?;
    
    let success_rate = if analysis.total_requested > 0 {
        (analysis.launched_vms.len() as f64 / analysis.total_requested as f64) * 100.0
    } else {
        0.0
    };
    
    log_tx.send(LogMessage::Info(format!(
        "VMs Launched: {}/{} ({:.1}% success rate)",
        analysis.launched_vms.len(),
        analysis.total_requested,
        success_rate
    )))?;
    
    if !analysis.launched_vms.is_empty() {
        log_tx.send(LogMessage::Info("".to_string()))?;
        log_tx.send(LogMessage::Success("SUCCESSFUL DEPLOYMENTS:".to_string()))?;
        for (vm_id, vm_name, approach, ip, _) in &analysis.launched_vms {
            log_tx.send(LogMessage::Success(format!(
                "  {} [{}] ({}) - {}",
                vm_name,
                vm_id,
                approach.to_uppercase(),
                ip.as_ref().unwrap_or(&"No IP".to_string())
            )))?;
        }
    }
    
    if analysis.launched_vms.len() < analysis.total_requested {
        let failed_count = analysis.total_requested - analysis.launched_vms.len();
        log_tx.send(LogMessage::Info("".to_string()))?;
        log_tx.send(LogMessage::Error(format!("FAILED DEPLOYMENTS: {}", failed_count)))?;
        
        if analysis.bulk_results.iter().any(|r| !r.success) ||
           analysis.decomposed_results.iter().any(|r| !r.success) {
            log_tx.send(LogMessage::Error("💡 TROUBLESHOOTING TIPS:".to_string()))?;
            log_tx.send(LogMessage::Error("   - Try removing country restrictions (remove 'datacenter' section)".to_string()))?;
            log_tx.send(LogMessage::Error("   - Increase maxPricePerEpochUsd (try 5.0 or 10.0)".to_string()))?;
            log_tx.send(LogMessage::Error("   - Use smaller VM size (cpu-1-ram-2gb-storage-25gb)".to_string()))?;
            log_tx.send(LogMessage::Error("   - Try different regions (DE, US, CA instead of PL)".to_string()))?;
        }
    }
    
    Ok(())
}

pub fn log_approach_results(_analysis: &AnalysisResult, _log_tx: &mpsc::Sender<LogMessage>) -> Result<(), Box<dyn std::error::Error>> {
    // Skip this - already covered in main report
    Ok(())
}

pub fn log_performance_comparison(analysis: &AnalysisResult, total_deployment_time: Duration, log_tx: &mpsc::Sender<LogMessage>) -> Result<(), Box<dyn std::error::Error>> {
    log_tx.send(LogMessage::Info("".to_string()))?;
    log_tx.send(LogMessage::Info(format!(
        "Total deployment time: {:.3} seconds",
        total_deployment_time.as_secs_f64()
    )))?;
    
    let bulk_count = analysis.bulk_results.len();
    let decomposed_count = analysis.decomposed_results.len();
    
    if bulk_count > 0 && decomposed_count > 0 {
        let bulk_time = analysis.bulk_results.iter()
            .map(|r| r.duration.as_secs_f64())
            .sum::<f64>() / bulk_count as f64;
            
        let decomposed_time = analysis.decomposed_results.iter()
            .map(|r| r.duration.as_secs_f64())
            .sum::<f64>() / decomposed_count as f64;
        
        log_tx.send(LogMessage::Info(format!(
            "Average time - Bulk: {:.3}s | Decomposed: {:.3}s",
            bulk_time,
            decomposed_time
        )))?;
    }
    
    Ok(())
}