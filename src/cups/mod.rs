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

    // Restore previously saved TC state so the LNS can connect immediately,
    // before the first CUPS check completes.
    let cred_dir = &conf.cups.credentials_dir;
    let saved_tc_uri = format!("{}/tc.uri", cred_dir);
    if let Ok(uri) = std::fs::read_to_string(&saved_tc_uri) {
        let uri = uri.trim().to_string();
        if !uri.is_empty() {
            debug!("Restored TC URI from {}", saved_tc_uri);
            crate::lns::set_cups_tc_uri(uri);
        }
    }
    let saved_tc_cred = format!("{}/tc.cred", cred_dir);
    if let Ok(data) = std::fs::read(&saved_tc_cred)
        && let Some((name, value)) = credentials::parse_token_from_cred(&data)
    {
        debug!("Restored TC auth token from {}", saved_tc_cred);
        crate::lns::set_cups_tc_auth_headers(vec![(name, value)]);
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

    // If CUPS previously provided a TC credential blob, use its CRC.
    // Otherwise fall back to the LNS credential files from config.
    let saved_tc_cred = format!("{}/tc.cred", conf.cups.credentials_dir);
    let tc_cred_crc = if std::path::Path::new(&saved_tc_cred).exists() {
        credentials::compute_cred_crc_from_file(&saved_tc_cred)?
    } else {
        credentials::compute_cred_crc(
            &conf.lns.ca_cert,
            &conf.lns.tls_cert,
            &conf.lns.tls_key,
        )?
    };

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
        crate::lns::set_cups_tc_uri(uri.clone());
        let tc_uri_path = format!("{}/tc.uri", conf.cups.credentials_dir);
        if let Err(e) = credentials::save_uri(&conf.cups.credentials_dir, "tc.uri", uri) {
            warn!("Failed to save TC URI: {}", e);
        } else {
            debug!("Saved TC URI to {}", tc_uri_path);
        }
    }

    if update.cups_cred.is_some() {
        info!("CUPS provided new CUPS credentials");
        if let Err(e) =
            credentials::save_credentials(&conf.cups.credentials_dir, "cups", &update.cups_cred)
        {
            warn!("Failed to save CUPS credentials: {}", e);
        }
    }

    if let Some(ref tc_cred) = update.tc_cred {
        info!("CUPS provided new TC credentials");

        // Save the raw blob to disk for CRC tracking on the next CUPS check.
        if let Err(e) =
            credentials::save_credentials(&conf.cups.credentials_dir, "tc", &update.tc_cred)
        {
            warn!("Failed to save TC credentials: {}", e);
        }

        // Parse the blob: in token mode the key field is a raw HTTP header line.
        if let Some((name, value)) = credentials::parse_token_from_cred(tc_cred) {
            info!("TC credential is in token mode, header: {}", name);
            crate::lns::set_cups_tc_auth_headers(vec![(name, value)]);
        } else {
            info!(
                "TC credential is a TLS certificate; \
                 set lns.tls_cert + lns.tls_key for mTLS"
            );
        }
    }

    if update.update_data.is_some() {
        info!("CUPS provided firmware update (not supported, ignoring)");
    }

    Ok(())
}
