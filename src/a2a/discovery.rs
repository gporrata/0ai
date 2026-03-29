use anyhow::Result;
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const SERVICE_TYPE: &str = "_0ai._tcp.local.";

#[derive(Debug, Clone)]
pub struct DiscoveredAgent {
    pub name: String,
    pub host: String,
    pub ip: String,
    pub port: u16,
    pub model: Option<String>,
    pub identity: Option<String>,
}

pub struct MdnsDiscovery {
    daemon: Arc<ServiceDaemon>,
    discovered: Arc<Mutex<HashMap<String, DiscoveredAgent>>>,
    advertising: Arc<Mutex<bool>>,
    instance_name: Option<String>,
}

impl MdnsDiscovery {
    pub fn new() -> Result<Self> {
        let daemon = ServiceDaemon::new().map_err(|e| anyhow::anyhow!("mDNS error: {}", e))?;
        Ok(Self {
            daemon: Arc::new(daemon),
            discovered: Arc::new(Mutex::new(HashMap::new())),
            advertising: Arc::new(Mutex::new(false)),
            instance_name: None,
        })
    }

    pub fn start_advertising(
        &mut self,
        agent_name: &str,
        port: u16,
        model: Option<&str>,
    ) -> Result<()> {
        let instance_name = format!("{}.{}", agent_name, SERVICE_TYPE);
        let hostname = format!("{}.local.", gethostname());

        let mut properties = HashMap::new();
        properties.insert("agent".to_string(), agent_name.to_string());
        if let Some(m) = model {
            properties.insert("model".to_string(), m.to_string());
        }
        properties.insert("version".to_string(), "0.1.0".to_string());

        let service = ServiceInfo::new(
            SERVICE_TYPE,
            agent_name,
            &hostname,
            "",
            port,
            properties,
        )
        .map_err(|e| anyhow::anyhow!("Failed to create mDNS service: {}", e))?;

        self.daemon
            .register(service)
            .map_err(|e| anyhow::anyhow!("Failed to register mDNS service: {}", e))?;

        *self.advertising.lock().unwrap() = true;
        self.instance_name = Some(instance_name);
        Ok(())
    }

    pub fn stop_advertising(&mut self) -> Result<()> {
        if let Some(ref name) = self.instance_name.clone() {
            // Extract just the instance name (before service type)
            let instance = name.trim_end_matches(SERVICE_TYPE).trim_end_matches('.');
            self.daemon
                .unregister(instance)
                .map_err(|e| anyhow::anyhow!("Failed to unregister mDNS: {}", e))?;
        }
        *self.advertising.lock().unwrap() = false;
        self.instance_name = None;
        Ok(())
    }

    pub fn is_advertising(&self) -> bool {
        *self.advertising.lock().unwrap()
    }

    pub fn start_browsing(&self) -> Result<()> {
        let receiver = self
            .daemon
            .browse(SERVICE_TYPE)
            .map_err(|e| anyhow::anyhow!("Failed to browse mDNS: {}", e))?;

        let discovered = Arc::clone(&self.discovered);

        tokio::spawn(async move {
            loop {
                match receiver.recv_async().await {
                    Ok(event) => match event {
                        ServiceEvent::ServiceResolved(info) => {
                            let name = info.get_fullname().to_string();
                            let host = info.get_hostname().to_string();
                            let port = info.get_port();
                            let properties = info.get_properties();

                            let ip = info
                                .get_addresses()
                                .iter()
                                .next()
                                .map(|a| a.to_string())
                                .unwrap_or_else(|| host.clone());

                            let agent_name = properties
                                .get("agent")
                                .map(|v| v.val_str().to_string())
                                .unwrap_or_else(|| name.clone());

                            let model = properties
                                .get("model")
                                .map(|v| v.val_str().to_string());

                            let agent = DiscoveredAgent {
                                name: agent_name,
                                host,
                                ip,
                                port,
                                model,
                                identity: None,
                            };

                            discovered.lock().unwrap().insert(name, agent);
                        }
                        ServiceEvent::ServiceRemoved(_, fullname) => {
                            discovered.lock().unwrap().remove(&fullname);
                        }
                        _ => {}
                    },
                    Err(_) => break,
                }
            }
        });

        Ok(())
    }

    pub fn discovered_agents(&self) -> Vec<DiscoveredAgent> {
        self.discovered
            .lock()
            .unwrap()
            .values()
            .cloned()
            .collect()
    }
}

fn gethostname() -> String {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "localhost".to_string())
}

impl Drop for MdnsDiscovery {
    fn drop(&mut self) {
        if self.is_advertising() {
            let _ = self.stop_advertising();
        }
        // Give daemon time to send goodbye packets
        std::thread::sleep(Duration::from_millis(100));
    }
}
