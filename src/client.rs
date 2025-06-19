use serde::{Deserialize, Serialize};
use serde_json::json;
use std::time::Duration;
use std::sync::mpsc;
use crate::deployment::LogMessage;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Deployment {
    #[serde(rename = "vmConfig")]
    pub vm_config: VmConfig,
    pub datacenter: Option<DatacenterConstraints>,
    pub instances: u32,
    #[serde(rename = "maxPricePerEpochUsd")]
    pub max_price_per_epoch_usd: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VmConfig {
    pub name: String,
    #[serde(rename = "basicConfiguration")]
    pub basic_configuration: String,
    #[serde(rename = "additionalStorage")]
    pub additional_storage: Option<Vec<StorageResource>>,
    #[serde(rename = "openPorts")]
    pub open_ports: Vec<PortConfig>,
    pub hostname: Option<String>,
    #[serde(rename = "osImage")]
    pub os_image: String,
    #[serde(rename = "sshKeys")]
    pub ssh_keys: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageResource {
    #[serde(rename = "type")]
    pub storage_type: String,
    pub supply: u32,
    pub units: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatacenterConstraints {
    pub countries: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortConfig {
    pub port: u16,
    pub protocol: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct VmResponse {
    #[serde(rename = "vmId")]
    pub vm_id: String,
    #[serde(rename = "vmName")]
    pub vm_name: String,
}

#[derive(Debug, Deserialize)]
pub struct VmStatusResponse {
    pub id: String,
    #[serde(rename = "vmName")]
    pub vm_name: String,
    pub status: String,
    #[serde(rename = "publicIp")]
    pub public_ip: Option<String>,
    pub datacenter: Option<serde_json::Value>,
    pub provider: Option<serde_json::Value>,
    pub provider_location: Option<serde_json::Value>,
}

impl VmStatusResponse {
    #[allow(dead_code)]
    pub fn get_provider_info(&self) -> (&Option<serde_json::Value>, &Option<serde_json::Value>) {
        (&self.provider, &self.provider_location)
    }
    
    #[allow(dead_code)]
    pub fn get_full_name(&self) -> &str {
        &self.vm_name
    }
}

pub struct FluenceClient {
    client: reqwest::blocking::Client,
    api_key: String,
    base_url: String,
}

impl FluenceClient {
    pub fn new(api_key: &str, base_url: &str, timeout_seconds: u64) -> Self {
        FluenceClient {
            client: reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(timeout_seconds))
                .build()
                .unwrap(),
            api_key: api_key.to_string(),
            base_url: base_url.to_string(),
        }
    }

    pub fn deploy_vm(&self, config: &Deployment, log_tx: Option<&mpsc::Sender<LogMessage>>) -> Result<Vec<VmResponse>, Box<dyn std::error::Error + Send + Sync>> {
        let url = format!("{}/vms/v3", self.base_url);
        
        let mut constraints = json!({
            "basicConfiguration": config.vm_config.basic_configuration
        });
        
        if let Some(price) = config.max_price_per_epoch_usd {
            constraints["maxTotalPricePerEpochUsd"] = json!(price.to_string());
        }
        
        if let Some(ref datacenter) = config.datacenter {
            if !datacenter.countries.is_empty() {
                if let Some(log_tx) = log_tx {
                    let _ = log_tx.send(LogMessage::Info(format!("DEBUG: Adding datacenter constraints: {:?}", datacenter.countries)));
                }
                constraints["datacenter"] = json!({
                    "countries": datacenter.countries
                });
            }
        }
        
        let mut vm_configuration = json!({
            "name": config.vm_config.name,
            "osImage": config.vm_config.os_image
        });
        
        if let Some(ref hostname) = config.vm_config.hostname {
            vm_configuration["hostname"] = json!(hostname);
        }
        
        if !config.vm_config.open_ports.is_empty() {
            vm_configuration["openPorts"] = json!(config.vm_config.open_ports);
        }
        
        if !config.vm_config.ssh_keys.is_empty() {
            vm_configuration["sshKeys"] = json!(config.vm_config.ssh_keys);
        }
        
        let request_body = json!({
            "constraints": constraints,
            "instances": config.instances,
            "vmConfiguration": vm_configuration
        });

        if let Some(log_tx) = log_tx {
            let _ = log_tx.send(LogMessage::Info(format!("DEBUG REQUEST JSON:\n{}", serde_json::to_string_pretty(&request_body).unwrap())));
        }

        let response = self.client
            .post(&url)
            .header("Content-Type", "application/json")
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&request_body)
            .send()?;

        let status = response.status();
        let response_text = response.text()?;
        
        if let Some(log_tx) = log_tx {
            let _ = log_tx.send(LogMessage::Info(format!("DEBUG RESPONSE: {} - {}", status, response_text)));
            
            // Add constraint summary for failures
            if !status.is_success() {
                let constraint_summary = format!(
                    "FAILED CONSTRAINTS: instances={}, config={}, price=${}, countries={:?}",
                    config.instances,
                    config.vm_config.basic_configuration,
                    config.max_price_per_epoch_usd.unwrap_or(0.0),
                    config.datacenter.as_ref().map(|dc| &dc.countries).unwrap_or(&vec!["ANY".to_string()])
                );
                let _ = log_tx.send(LogMessage::Error(constraint_summary));
            }
        }

        if status.is_success() {
            let vm_responses: Vec<VmResponse> = serde_json::from_str(&response_text)?;
            Ok(vm_responses)
        } else {
            let error_response: Result<serde_json::Value, _> = serde_json::from_str(&response_text);
            let error_message = match error_response {
                Ok(err) => err.get("error").and_then(|e| e.as_str()).unwrap_or(&response_text).to_string(),
                Err(_) => format!("HTTP {} - {}", status, response_text),
            };
            Err(error_message.into())
        }
    }

    pub fn get_vm_status(&self, vm_id: &str) -> Result<VmStatusResponse, Box<dyn std::error::Error + Send + Sync>> {
        let url = format!("{}/vms/v3", self.base_url);
        
        let response = self.client
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .send()?;

        let status = response.status();
        let response_text = response.text()?;

        if status.is_success() {
            let vms: Vec<VmStatusResponse> = serde_json::from_str(&response_text)?;
            for vm in vms {
                if vm.id == vm_id {
                    return Ok(vm);
                }
            }
            Err("VM not found".into())
        } else {
            Err(format!("API Error: {} - {}", status, response_text).into())
        }
    }

    pub fn delete_vms(&self, vm_ids: &[String]) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let url = format!("{}/vms/v3", self.base_url);
        
        let request_body = json!({"vmIds": vm_ids});

        let response = self.client
            .delete(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&request_body)
            .send()?;

        if response.status().is_success() {
            Ok(())
        } else {
            Err(format!("API Error: {}", response.text()?).into())
        }
    }
}

impl Clone for FluenceClient {
    fn clone(&self) -> Self {
        FluenceClient {
            client: self.client.clone(),
            api_key: self.api_key.clone(),
            base_url: self.base_url.clone(),
        }
    }
}