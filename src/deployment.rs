use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};
use crate::client::{FluenceClient, VmResponse, VmStatusResponse, Deployment};

// =============================================================================
// DEPLOYMENT TYPES
// =============================================================================

#[derive(Debug)]
pub struct DeploymentResult {
    pub config_name: String,
    pub approach: String, // "bulk" or "decomposed"
    pub start_time: Instant,
    pub duration: Duration,
    pub success: bool,
    pub vm_responses: Option<Vec<VmResponse>>,
    pub vm_statuses: Option<Vec<VmStatusResponse>>,
    pub error_message: Option<String>,
    pub thread_id: thread::ThreadId,
    pub individual_timings: Option<Vec<Duration>>, // For decomposed approach
    pub vm_phase_timings: Option<std::collections::HashMap<String, VmPhaseTimings>>, // VM ID -> phase timings
}

impl DeploymentResult {
    #[allow(dead_code)]
    pub fn get_execution_time(&self) -> Duration {
        self.start_time.elapsed()
    }
}

#[derive(Debug)]
pub enum LogMessage {
    Info(String),
    Success(String),
    Error(String),
    Shutdown,
}

// =============================================================================
// CENTRALIZED VM STATUS POLLING - FIXED TIMEOUT
// =============================================================================

#[derive(Debug, Clone)]
pub struct VmPollingRequest {
    pub vm_id: String,
    pub vm_name: String,
    pub deployment_name: String,
    pub approach: String, // "bulk" or "decomposed"
    pub response_tx: mpsc::Sender<VmPollingResponse>,
}

#[derive(Debug, Clone)]
pub struct VmPollingResponse {
    pub vm_id: String,
    pub vm_name: String,
    pub deployment_name: String,
    pub approach: String,
    pub status: String,
    pub public_ip: Option<String>,
    pub provider: Option<serde_json::Value>,
    pub provider_location: Option<serde_json::Value>,
    pub datacenter: Option<serde_json::Value>,
    pub success: bool,
    pub error: Option<String>,
    pub phase_timings: VmPhaseTimings,
}

impl VmPollingResponse {
    #[allow(dead_code)]
    pub fn get_deployment_info(&self) -> (&str, &str) {
        (&self.deployment_name, &self.approach)
    }
}

#[derive(Debug, Clone)]
pub struct VmPhaseTimings {
    pub requested_time: Instant,
    pub launching_entered: Option<Instant>,
    pub ip_assigned: Option<Instant>,
    pub active_time: Option<Instant>,
    pub total_launch_duration: Option<Duration>,
    pub time_in_launching: Option<Duration>,
    pub stuck_in_launching: bool,
    pub launching_phase_details: Vec<LaunchingPhaseCheck>,
}

impl VmPhaseTimings {
    pub fn new() -> Self {
        Self {
            requested_time: Instant::now(),
            launching_entered: None,
            ip_assigned: None,
            active_time: None,
            total_launch_duration: None,
            time_in_launching: None,
            stuck_in_launching: false,
            launching_phase_details: Vec::new(),
        }
    }
    
    pub fn get_request_to_launch_duration(&self) -> Option<Duration> {
        self.launching_entered.map(|launch_time| launch_time.duration_since(self.requested_time))
    }
    
    pub fn get_launch_to_ip_duration(&self) -> Option<Duration> {
        if let (Some(launch_time), Some(ip_time)) = (self.launching_entered, self.ip_assigned) {
            Some(ip_time.duration_since(launch_time))
        } else {
            None
        }
    }
    
    pub fn get_ip_to_active_duration(&self) -> Option<Duration> {
        if let (Some(ip_time), Some(active_time)) = (self.ip_assigned, self.active_time) {
            Some(active_time.duration_since(ip_time))
        } else {
            None
        }
    }
    
    pub fn get_total_duration(&self) -> Option<Duration> {
        self.total_launch_duration.or_else(|| {
            self.active_time.map(|active_time| active_time.duration_since(self.requested_time))
        })
    }
    
    pub fn mark_ip_assigned(&mut self) {
        if self.ip_assigned.is_none() {
            self.ip_assigned = Some(Instant::now());
        }
    }
    
    pub fn mark_active(&mut self) {
        if self.active_time.is_none() {
            self.active_time = Some(Instant::now());
            self.total_launch_duration = Some(self.active_time.unwrap().duration_since(self.requested_time));
            
            if let Some(launching_time) = self.launching_entered {
                self.time_in_launching = Some(self.active_time.unwrap().duration_since(launching_time));
            }
        }
    }
    
    pub fn mark_launching_entered(&mut self) {
        if self.launching_entered.is_none() {
            self.launching_entered = Some(Instant::now());
        }
    }
}

#[derive(Debug, Clone)]
pub struct LaunchingPhaseCheck {
    pub timestamp: Instant,
    pub elapsed_since_launch: Duration,
    pub has_ip: bool,
    pub status: String,
}

#[allow(dead_code)]
impl LaunchingPhaseCheck {
    pub fn new(timestamp: Instant, elapsed_since_launch: Duration, has_ip: bool, status: String) -> Self {
        Self {
            timestamp,
            elapsed_since_launch,
            has_ip,
            status,
        }
    }
}

pub fn create_vm_polling_thread(
    client: FluenceClient,
    timeout_seconds: u64,
    poll_interval_seconds: u64,
    enable_ip_retrieval: bool,
    log_tx: mpsc::Sender<LogMessage>,
) -> mpsc::Sender<VmPollingRequest> {
    let (polling_tx, polling_rx) = mpsc::channel::<VmPollingRequest>();
    
    thread::spawn(move || {
        let mut active_polls: Vec<(VmPollingRequest, VmPhaseTimings)> = Vec::new();
        let mut last_poll_time = Instant::now();
        let poll_interval = Duration::from_secs(poll_interval_seconds);
        
        loop {
            // Collect new polling requests (non-blocking)
            while let Ok(new_request) = polling_rx.try_recv() {
                let _ = log_tx.send(LogMessage::Info(format!(
                    "[POLLING] Added VM {} ({}) to polling queue",
                    new_request.vm_name, new_request.vm_id
                )));
                
                let phase_timings = VmPhaseTimings::new();
                active_polls.push((new_request, phase_timings));
            }
            
            // If no VMs to poll, sleep briefly
            if active_polls.is_empty() {
                thread::sleep(Duration::from_millis(100));
                continue;
            }
            
            // Check if it's time to poll
            if last_poll_time.elapsed() >= poll_interval {
                let _ = log_tx.send(LogMessage::Info(format!(
                    "[POLLING] Checking status of {} VMs...",
                    active_polls.len()
                )));
                
                let mut completed_polls = Vec::new();
                
                for (index, (poll_request, phase_timings)) in active_polls.iter_mut().enumerate() {
                    // Check for timeout PER VM
                    if phase_timings.requested_time.elapsed() > Duration::from_secs(timeout_seconds) {
                        let _ = log_tx.send(LogMessage::Error(format!(
                            "[POLLING] VM {} ({}) TIMEOUT after {:.1}s - never launched",
                            poll_request.vm_name, poll_request.deployment_name, phase_timings.requested_time.elapsed().as_secs_f64()
                        )));
                        
                        let mut final_timings = phase_timings.clone();
                        final_timings.total_launch_duration = Some(phase_timings.requested_time.elapsed());
                        
                        let response = VmPollingResponse {
                            vm_id: poll_request.vm_id.clone(),
                            vm_name: poll_request.vm_name.clone(),
                            deployment_name: poll_request.deployment_name.clone(),
                            approach: poll_request.approach.clone(),
                            status: "Timeout".to_string(),
                            public_ip: None,
                            provider: None,
                            provider_location: None,
                            datacenter: None,
                            success: false,
                            error: Some(format!("Timeout after {:.1}s", phase_timings.requested_time.elapsed().as_secs_f64())),
                            phase_timings: final_timings,
                        };
                        
                        let _ = poll_request.response_tx.send(response);
                        completed_polls.push(index);
                        continue;
                    }
                    
                    match client.get_vm_status(&poll_request.vm_id) {
                        Ok(status) => {
                            match status.status.as_str() {
                                "Launching" => {
                                    phase_timings.mark_launching_entered();
                                    
                                    let has_ip = status.public_ip.is_some();
                                    
                                    if has_ip && phase_timings.ip_assigned.is_none() {
                                        phase_timings.mark_ip_assigned();
                                        let _ = log_tx.send(LogMessage::Success(format!(
                                            "[POLLING] VM {} ({}) IP ASSIGNED {} after {:.1}s in launching",
                                            poll_request.vm_name, poll_request.deployment_name,
                                            status.public_ip.as_ref().unwrap(),
                                            phase_timings.get_launch_to_ip_duration().unwrap_or(Duration::from_secs(0)).as_secs_f64()
                                        )));
                                    }
                                    
                                    let launching_elapsed = phase_timings.launching_entered
                                        .map(|t| Instant::now().duration_since(t))
                                        .unwrap_or(Duration::from_secs(0));
                                    
                                    let _ = log_tx.send(LogMessage::Info(format!(
                                        "[POLLING] VM {} ({}) launching {} IP ({:.1}s in launching, {:.1}s total)",
                                        poll_request.vm_name, poll_request.deployment_name,
                                        if has_ip { "WITH" } else { "WITHOUT" },
                                        launching_elapsed.as_secs_f64(),
                                        phase_timings.requested_time.elapsed().as_secs_f64()
                                    )));
                                    
                                    if launching_elapsed > Duration::from_secs(300) {
                                        phase_timings.stuck_in_launching = true;
                                        let _ = log_tx.send(LogMessage::Error(format!(
                                            "[POLLING] VM {} ({}) STUCK in launching phase for {:.1}s! IP allocated: {}",
                                            poll_request.vm_name, poll_request.deployment_name,
                                            launching_elapsed.as_secs_f64(), has_ip
                                        )));
                                    }
                                }
                                "Active" => {
                                    phase_timings.mark_active();
                                    
                                    if enable_ip_retrieval && status.public_ip.is_none() {
                                        let _ = log_tx.send(LogMessage::Error(format!(
                                            "[POLLING] VM {} ({}) is Active but has NO IP! This is unexpected.",
                                            poll_request.vm_name, poll_request.deployment_name
                                        )));
                                        continue;
                                    }
                                    
                                    let datacenter_str = status.datacenter.as_ref()
                                        .and_then(|v| v.as_str().or_else(|| v.get("name").and_then(|n| n.as_str())))
                                        .map(|s| s.to_string());
                                    
                                    let _ = log_tx.send(LogMessage::Success(format!(
                                        "[POLLING] VM {} ({}) FULLY LAUNCHED in {:.1}s{}{}",
                                        poll_request.vm_name, poll_request.deployment_name,
                                        phase_timings.get_total_duration().unwrap().as_secs_f64(),
                                        if let Some(ref ip) = status.public_ip {
                                            format!(" IP: {}", ip)
                                        } else {
                                            String::new()
                                        },
                                        if let Some(ref dc) = datacenter_str {
                                            format!(" Datacenter: {}", dc)
                                        } else {
                                            String::new()
                                        }
                                    )));
                                    
                                    let response = VmPollingResponse {
                                        vm_id: poll_request.vm_id.clone(),
                                        vm_name: poll_request.vm_name.clone(),
                                        deployment_name: poll_request.deployment_name.clone(),
                                        approach: poll_request.approach.clone(),
                                        status: status.status,
                                        public_ip: status.public_ip,
                                        provider: None,
                                        provider_location: None,
                                        datacenter: status.datacenter.clone(),
                                        success: true,
                                        error: None,
                                        phase_timings: phase_timings.clone(),
                                    };
                                    
                                    let _ = poll_request.response_tx.send(response);
                                    completed_polls.push(index);
                                }
                                other => {
                                    let _ = log_tx.send(LogMessage::Error(format!(
                                        "[POLLING] VM {} ({}) failed with status: {} after {:.1}s",
                                        poll_request.vm_name, poll_request.deployment_name, other, 
                                        phase_timings.requested_time.elapsed().as_secs_f64()
                                    )));
                                    
                                    let mut final_timings = phase_timings.clone();
                                    final_timings.total_launch_duration = Some(phase_timings.requested_time.elapsed());
                                    
                                    let response = VmPollingResponse {
                                        vm_id: poll_request.vm_id.clone(),
                                        vm_name: poll_request.vm_name.clone(),
                                        deployment_name: poll_request.deployment_name.clone(),
                                        approach: poll_request.approach.clone(),
                                        status: other.to_string(),
                                        public_ip: None,
                                        provider: None,
                                        provider_location: None,
                                        datacenter: None,
                                        success: false,
                                        error: Some(format!("Unexpected status: {}", other)),
                                        phase_timings: final_timings,
                                    };
                                    
                                    let _ = poll_request.response_tx.send(response);
                                    completed_polls.push(index);
                                }
                            }
                        }
                        Err(error) => {
                            if error.to_string().contains("429") || error.to_string().contains("Too Many Requests") {
                                let _ = log_tx.send(LogMessage::Info(format!(
                                    "[POLLING] Rate limited, will retry VM {} ({})",
                                    poll_request.vm_name, poll_request.deployment_name
                                )));
                                thread::sleep(Duration::from_secs(5));
                                continue;
                            } else {
                                let _ = log_tx.send(LogMessage::Error(format!(
                                    "[POLLING] Failed to get status for VM {} ({}) after {:.1}s: {}",
                                    poll_request.vm_name, poll_request.deployment_name, 
                                    phase_timings.requested_time.elapsed().as_secs_f64(), error
                                )));
                                
                                let mut final_timings = phase_timings.clone();
                                final_timings.total_launch_duration = Some(phase_timings.requested_time.elapsed());
                                
                                let response = VmPollingResponse {
                                    vm_id: poll_request.vm_id.clone(),
                                    vm_name: poll_request.vm_name.clone(),
                                    deployment_name: poll_request.deployment_name.clone(),
                                    approach: poll_request.approach.clone(),
                                    status: "Error".to_string(),
                                    public_ip: None,
                                    provider: None,
                                    provider_location: None,
                                    datacenter: None,
                                    success: false,
                                    error: Some(error.to_string()),
                                    phase_timings: final_timings,
                                };
                                
                                let _ = poll_request.response_tx.send(response);
                                completed_polls.push(index);
                            }
                        }
                    }
                }
                
                // Remove completed polls (in reverse order to maintain indices)
                for &index in completed_polls.iter().rev() {
                    active_polls.remove(index);
                }
                
                last_poll_time = Instant::now();
            }
            
            // Brief sleep to avoid busy waiting
            thread::sleep(Duration::from_millis(100));
        }
    });
    
    polling_tx
}

// =============================================================================
// DEPLOYMENT WORKER FUNCTIONS
// =============================================================================

pub fn deploy_vm_bulk_worker(
    config: Deployment,
    client: FluenceClient,
    polling_config: (bool, bool, u64, u64),
    vm_queue: Arc<std::sync::Mutex<Vec<String>>>,
    polling_tx: Option<mpsc::Sender<VmPollingRequest>>,
    result_tx: mpsc::Sender<DeploymentResult>,
    log_tx: mpsc::Sender<LogMessage>,
) {
    let thread_id = thread::current().id();
    let config_name = config.vm_config.name.clone();
    let (enable_status_polling, _enable_ip_retrieval, timeout_seconds, _poll_interval_seconds) = polling_config;
    
    let _ = log_tx.send(LogMessage::Info(format!(
        "[Thread {:?}] BULK: Starting deployment of config '{}' with {} instances", 
        thread_id, config.vm_config.name, config.instances
    )));

    let start = Instant::now();
    let api_result = client.deploy_vm(&config, Some(&log_tx));
    let api_duration = start.elapsed();

    let result = match api_result {
        Ok(vm_responses) => {
            let _ = log_tx.send(LogMessage::Success(format!(
                "[Thread {:?}] BULK: Config '{}' API request successful in {:.6}s. REQUESTED {} VMs",
                thread_id, config.vm_config.name, api_duration.as_secs_f64(), vm_responses.len()
            )));

            {
                let mut queue = vm_queue.lock().unwrap();
                for vm in &vm_responses {
                    queue.push(vm.vm_id.clone());
                }
            }

            let (vm_statuses, vm_timings) = if enable_status_polling {
                if let Some(ref polling_tx) = polling_tx {
                    let (response_tx, response_rx) = mpsc::channel::<VmPollingResponse>();
                    
                    for vm in &vm_responses {
                        let polling_request = VmPollingRequest {
                            vm_id: vm.vm_id.clone(),
                            vm_name: vm.vm_name.clone(),
                            deployment_name: config_name.clone(),
                            approach: "bulk".to_string(),
                            response_tx: response_tx.clone(),
                        };
                        let _ = polling_tx.send(polling_request);
                    }
                    
                    drop(response_tx);
                    
                    let mut statuses = Vec::new();
                    let mut timings = std::collections::HashMap::new();
                    let mut received_count = 0;
                    
                    while received_count < vm_responses.len() {
                        match response_rx.recv_timeout(Duration::from_secs(timeout_seconds + 60)) {
                            Ok(response) => {
                                received_count += 1;
                                
                                if response.success {
                                    let status = VmStatusResponse {
                                        id: response.vm_id.clone(),
                                        vm_name: response.vm_name.clone(),
                                        status: response.status,
                                        public_ip: response.public_ip,
                                        datacenter: response.datacenter,
                                        provider: response.provider,
                                        provider_location: response.provider_location,
                                    };
                                    statuses.push(status);
                                    timings.insert(response.vm_id.clone(), response.phase_timings);
                                }
                            }
                            Err(_) => break,
                        }
                    }
                    
                    (if statuses.is_empty() { None } else { Some(statuses) }, Some(timings))
                } else {
                    (None, None)
                }
            } else {
                (None, None)
            };

            DeploymentResult {
                config_name,
                approach: "bulk".to_string(),
                start_time: start,
                duration: start.elapsed(),
                success: true,
                vm_responses: Some(vm_responses),
                vm_statuses,
                error_message: None,
                thread_id,
                individual_timings: None,
                vm_phase_timings: vm_timings,
            }
        }
        Err(error) => {
            let _ = log_tx.send(LogMessage::Error(format!(
                "[Thread {:?}] BULK: Config '{}' failed after {:.6}s: {}",
                thread_id, config.vm_config.name, api_duration.as_secs_f64(), error
            )));

            DeploymentResult {
                config_name,
                approach: "bulk".to_string(),
                start_time: start,
                duration: api_duration,
                success: false,
                vm_responses: None,
                vm_statuses: None,
                error_message: Some(error.to_string()),
                thread_id,
                individual_timings: None,
                vm_phase_timings: None,
            }
        }
    };

    let _ = result_tx.send(result);
}

pub fn deploy_vm_decomposed_worker(
    config: Deployment,
    client: FluenceClient,
    polling_config: (bool, bool, u64, u64),
    vm_queue: Arc<std::sync::Mutex<Vec<String>>>,
    polling_tx: Option<mpsc::Sender<VmPollingRequest>>,
    result_tx: mpsc::Sender<DeploymentResult>,
    log_tx: mpsc::Sender<LogMessage>,
) {
    let thread_id = thread::current().id();
    let config_name = config.vm_config.name.clone();
    let instances = config.instances;
    let (enable_status_polling, _enable_ip_retrieval, timeout_seconds, _poll_interval_seconds) = polling_config;
    
    let _ = log_tx.send(LogMessage::Info(format!(
        "[Thread {:?}] DECOMPOSED: Starting deployment of config '{}' with {} individual API calls", 
        thread_id, config.vm_config.name, instances
    )));

    let overall_start = Instant::now();
    let mut all_vm_responses = Vec::new();
    let mut individual_timings = Vec::new();
    let mut success = true;
    let mut error_message = None;

    for i in 0..instances {
        let individual_config = Deployment {
            vm_config: crate::client::VmConfig {
                name: format!("{}-{}", config.vm_config.name, i + 1),
                basic_configuration: config.vm_config.basic_configuration.clone(),
                additional_storage: config.vm_config.additional_storage.clone(),
                open_ports: config.vm_config.open_ports.clone(),
                hostname: config.vm_config.hostname.clone(),
                os_image: config.vm_config.os_image.clone(),
                ssh_keys: config.vm_config.ssh_keys.clone(),
            },
            datacenter: config.datacenter.clone(),
            instances: 1,
            max_price_per_epoch_usd: config.max_price_per_epoch_usd,
        };

        let individual_start = Instant::now();
        match client.deploy_vm(&individual_config, Some(&log_tx)) {
            Ok(vm_responses) => {
                let individual_duration = individual_start.elapsed();
                individual_timings.push(individual_duration);
                all_vm_responses.extend(vm_responses.clone());
                
                let _ = log_tx.send(LogMessage::Success(format!(
                    "[Thread {:?}] DECOMPOSED: VM {}/{} '{}' REQUESTED in {:.6}s",
                    thread_id, i + 1, instances, individual_config.vm_config.name, individual_duration.as_secs_f64()
                )));

                {
                    let mut queue = vm_queue.lock().unwrap();
                    for vm in &vm_responses {
                        queue.push(vm.vm_id.clone());
                    }
                }
            }
            Err(error) => {
                let individual_duration = individual_start.elapsed();
                individual_timings.push(individual_duration);
                
                let _ = log_tx.send(LogMessage::Error(format!(
                    "[Thread {:?}] DECOMPOSED: VM {}/{} '{}' failed in {:.6}s: {} - CONTINUING",
                    thread_id, i + 1, instances, individual_config.vm_config.name, individual_duration.as_secs_f64(), error
                )));
                
                if all_vm_responses.is_empty() && i == instances - 1 {
                    success = false;
                    error_message = Some(error.to_string());
                }
            }
        }
    }

    let (vm_statuses, vm_timings) = if enable_status_polling && !all_vm_responses.is_empty() {
        if let Some(ref polling_tx) = polling_tx {
            let (response_tx, response_rx) = mpsc::channel::<VmPollingResponse>();
            
            for vm in &all_vm_responses {
                let polling_request = VmPollingRequest {
                    vm_id: vm.vm_id.clone(),
                    vm_name: vm.vm_name.clone(),
                    deployment_name: config_name.clone(),
                    approach: "decomposed".to_string(),
                    response_tx: response_tx.clone(),
                };
                let _ = polling_tx.send(polling_request);
            }
            
            drop(response_tx);
            
            let mut statuses = Vec::new();
            let mut timings = std::collections::HashMap::new();
            let mut received_count = 0;
            
            while received_count < all_vm_responses.len() {
                match response_rx.recv_timeout(Duration::from_secs(timeout_seconds + 60)) {
                    Ok(response) => {
                        received_count += 1;
                        
                        if response.success {
                            let status = VmStatusResponse {
                                id: response.vm_id.clone(),
                                vm_name: response.vm_name.clone(),
                                status: response.status,
                                public_ip: response.public_ip,
                                datacenter: response.datacenter,
                                provider: response.provider,
                                provider_location: response.provider_location,
                            };
                            statuses.push(status);
                            timings.insert(response.vm_id.clone(), response.phase_timings);
                        }
                    }
                    Err(_) => break,
                }
            }
            
            (if statuses.is_empty() { None } else { Some(statuses) }, Some(timings))
        } else {
            (None, None)
        }
    } else {
        (None, None)
    };

    let result = DeploymentResult {
        config_name,
        approach: "decomposed".to_string(),
        start_time: overall_start,
        duration: overall_start.elapsed(),
        success,
        vm_responses: if all_vm_responses.is_empty() { None } else { Some(all_vm_responses) },
        vm_statuses,
        error_message,
        thread_id,
        individual_timings: Some(individual_timings),
        vm_phase_timings: vm_timings,
    };

    let _ = result_tx.send(result);
}

// =============================================================================
// LOGGING
// =============================================================================

pub fn create_logger_thread(enable_console: bool) -> mpsc::Sender<LogMessage> {
    let (tx, rx) = mpsc::channel::<LogMessage>();
    
    thread::spawn(move || {
        let mut log_buffer = Vec::with_capacity(1024);
        
        loop {
            match rx.recv() {
                Ok(LogMessage::Shutdown) => {
                    flush_log_buffer(&mut log_buffer);
                    break;
                }
                Ok(msg) => {
                    let formatted = format_log_message(&msg);
                    
                    if enable_console {
                        println!("{}", formatted);
                    }
                    
                    log_buffer.push(formatted);
                    
                    if log_buffer.len() >= 10 || matches!(msg, LogMessage::Error(_)) {
                        flush_log_buffer(&mut log_buffer);
                    }
                }
                Err(_) => break,
            }
        }
    });
    
    tx
}

fn format_log_message(msg: &LogMessage) -> String {
    let timestamp = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC");
    match msg {
        LogMessage::Info(text) => format!("[{}] {}", timestamp, text),
        LogMessage::Success(text) => format!("[{}] SUCCESS: {}", timestamp, text),
        LogMessage::Error(text) => format!("[{}] ERROR: {}", timestamp, text),
        LogMessage::Shutdown => String::new(),
    }
}

fn flush_log_buffer(buffer: &mut Vec<String>) {
    if !buffer.is_empty() {
        let log_file_path = std::env::var("LOG_FILE_PATH")
            .unwrap_or_else(|_| "./vm_deployment.log".to_string());
            
        if !log_file_path.is_empty() {
            if let Ok(mut file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&log_file_path) 
            {
                use std::io::Write;
                for line in buffer.drain(..) {
                    let _ = writeln!(file, "{}", line);
                }
                let _ = file.flush();
            }
        }
        buffer.clear();
    }
}