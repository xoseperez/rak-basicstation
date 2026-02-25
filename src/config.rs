use std::collections::HashMap;
use std::time::Duration;
use std::{env, fs};

use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Configuration {
    pub logging: Logging,
    pub backend: Backend,
    pub lns: Lns,
    pub cups: Cups,
    pub metadata: Metadata,
}

impl Configuration {
    pub fn get(filenames: &[String]) -> Result<Configuration> {
        let mut content = String::new();

        for file_name in filenames {
            content.push_str(&fs::read_to_string(file_name)?);
        }

        // Replace environment variables in config.
        for (k, v) in env::vars() {
            content = content.replace(&format!("${}", k), &v);
        }

        let config: Configuration = toml::from_str(&content)?;
        Ok(config)
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Logging {
    pub level: String,
    pub log_to_syslog: bool,
}

impl Default for Logging {
    fn default() -> Self {
        Logging {
            level: "info".to_string(),
            log_to_syslog: false,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Backend {
    pub enabled: String,
    pub gateway_id: String,
    pub filters: Filters,
    pub concentratord: Concentratord,
    pub semtech_udp: SemtechUdp,
}

impl Default for Backend {
    fn default() -> Self {
        Backend {
            enabled: "concentratord".to_string(),
            gateway_id: "".into(),
            filters: Filters::default(),
            concentratord: Concentratord::default(),
            semtech_udp: SemtechUdp::default(),
        }
    }
}


#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Filters {
    pub forward_crc_ok: bool,
    pub forward_crc_invalid: bool,
    pub forward_crc_missing: bool,
}

impl Default for Filters {
    fn default() -> Self {
        Filters {
            forward_crc_ok: true,
            forward_crc_invalid: false,
            forward_crc_missing: false,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Concentratord {
    pub event_url: String,
    pub command_url: String,
    pub context_caching: bool,
}

impl Default for Concentratord {
    fn default() -> Self {
        Concentratord {
            event_url: "ipc:///tmp/concentratord_event".into(),
            command_url: "ipc:///tmp/concentratord_command".into(),
            context_caching: false,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SemtechUdp {
    pub bind: String,
    pub time_fallback_enabled: bool,
}

impl Default for SemtechUdp {
    fn default() -> Self {
        SemtechUdp {
            bind: "0.0.0.0:1700".to_string(),
            time_fallback_enabled: false,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Lns {
    pub server: String,
    pub discovery_endpoint: String,
    #[serde(with = "humantime_serde")]
    pub reconnect_interval: Duration,
    pub ca_cert: String,
    pub tls_cert: String,
    pub tls_key: String,
}

impl Default for Lns {
    fn default() -> Self {
        Lns {
            server: "wss://localhost:8887".into(),
            discovery_endpoint: "".into(),
            reconnect_interval: Duration::from_secs(5),
            ca_cert: "".into(),
            tls_cert: "".into(),
            tls_key: "".into(),
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Cups {
    pub enabled: bool,
    pub server: String,
    #[serde(with = "humantime_serde")]
    pub oksync_interval: Duration,
    #[serde(with = "humantime_serde")]
    pub resync_interval: Duration,
    pub ca_cert: String,
    pub tls_cert: String,
    pub tls_key: String,
    pub credentials_dir: String,
    pub sig_keys: Vec<String>,
}

impl Default for Cups {
    fn default() -> Self {
        Cups {
            enabled: false,
            server: "".into(),
            oksync_interval: Duration::from_secs(86400),
            resync_interval: Duration::from_secs(60),
            ca_cert: "".into(),
            tls_cert: "".into(),
            tls_key: "".into(),
            credentials_dir: "/var/lib/rak-basicstation/credentials".into(),
            sig_keys: vec![],
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Metadata {
    pub r#static: HashMap<String, String>,
    pub commands: HashMap<String, Vec<String>>,
    pub split_delimiter: String,
}

impl Default for Metadata {
    fn default() -> Self {
        Metadata {
            r#static: HashMap::default(),
            commands: HashMap::default(),
            split_delimiter: "=".to_string(),
        }
    }
}
