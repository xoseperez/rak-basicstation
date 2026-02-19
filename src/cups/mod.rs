use std::sync::Arc;

use anyhow::Result;
use log::{debug, error, info, warn};

use crate::config::Configuration;
use crate::lns::discovery::gateway_id_to_id6;

pub mod client;
pub mod credentials;

pub async fn setup(conf: &Configuration) -> Result<()> {
    if !conf.cups.enabled {
        info!("CUPS is disabled");
        return Ok(());
    }

    let gateway_id = crate::backend::get_gateway_id().await?;
    let conf = Arc::new(conf.clone());

    tokio::spawn(async move {
        update_loop(conf, gateway_id).await;
    });

    Ok(())
}

async fn update_loop(conf: Arc<Configuration>, gateway_id: String) {
    loop {
        let interval = match run_update(&conf, &gateway_id).await {
            Ok(()) => {
                info!("CUPS update check succeeded");
                conf.cups.oksync_interval
            }
            Err(e) => {
                error!("CUPS update check failed: {}", e);
                conf.cups.resync_interval
            }
        };

        debug!("Next CUPS check in {:?}", interval);
        tokio::time::sleep(interval).await;
    }
}

async fn run_update(conf: &Configuration, gateway_id: &str) -> Result<()> {
    let id6 = gateway_id_to_id6(gateway_id)?;

    let cups_cred_crc = credentials::compute_cred_crc(
        &conf.cups.ca_cert,
        &conf.cups.tls_cert,
        &conf.cups.tls_key,
    )?;

    let tc_cred_crc = credentials::compute_cred_crc(
        &conf.lns.ca_cert,
        &conf.lns.tls_cert,
        &conf.lns.tls_key,
    )?;

    let sig_key_crcs = credentials::compute_sig_key_crcs(&conf.cups.sig_keys)?;

    let resp = client::post_update_info(
        &conf.cups.server,
        &id6,
        &conf.cups.server,
        &conf.lns.server,
        cups_cred_crc,
        tc_cred_crc,
        &sig_key_crcs,
        conf,
    )
    .await?;

    // Parse binary response.
    let update = client::parse_response(&resp)?;

    if let Some(ref uri) = update.cups_uri {
        info!("CUPS provided new CUPS URI: {}", uri);
    }

    if let Some(ref uri) = update.tc_uri {
        info!("CUPS provided new TC/LNS URI: {}", uri);
    }

    if update.cups_cred.is_some() {
        info!("CUPS provided new CUPS credentials");
        if let Err(e) =
            credentials::save_credentials(&conf.cups.credentials_dir, "cups", &update.cups_cred)
        {
            warn!("Failed to save CUPS credentials: {}", e);
        }
    }

    if update.tc_cred.is_some() {
        info!("CUPS provided new TC credentials");
        if let Err(e) =
            credentials::save_credentials(&conf.cups.credentials_dir, "tc", &update.tc_cred)
        {
            warn!("Failed to save TC credentials: {}", e);
        }
    }

    if update.update_data.is_some() {
        info!("CUPS provided firmware update (not supported, ignoring)");
    }

    Ok(())
}
