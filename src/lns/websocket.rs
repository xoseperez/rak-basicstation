use std::fs;
use std::sync::Arc;

use anyhow::Result;
use rustls_pki_types::pem::PemObject;
use futures_util::{SinkExt, StreamExt};
use log::{debug, error, info, warn};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::{HeaderName, HeaderValue};
use tokio_tungstenite::tungstenite::Message;

use super::messages::{
    DownlinkMessage, DownlinkSchedule, GenericMessage, RouterConfig, TimesyncResponse,
    VersionMessage,
};
use super::{downlink, router_config, timesync, LAST_MUX_TIME, ROUTER_CONFIG, WS_SENDER};
use crate::config::Configuration;

/// Run the WebSocket connection to the LNS MUXS server.
pub async fn run(
    conf: &Configuration,
    muxs_uri: &str,
    gateway_id: &str,
    auth_headers: &[(String, String)],
) -> Result<()> {
    let connector = build_tls_connector(conf)?;

    let mut request = muxs_uri.into_client_request()?;
    for (name, value) in auth_headers {
        request.headers_mut().insert(
            HeaderName::from_bytes(name.as_bytes())?,
            HeaderValue::from_str(value)?,
        );
    }

    let (ws_stream, _resp) = tokio_tungstenite::connect_async_tls_with_config(
        request,
        None,
        false,
        Some(tokio_tungstenite::Connector::Rustls(connector)),
    )
    .await?;

    info!("WebSocket connected to {}", muxs_uri);

    let (mut ws_write, mut ws_read) = ws_stream.split();

    // Create a channel for outgoing messages.
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();

    // Store the sender globally.
    {
        let mut sender = WS_SENDER.write().unwrap();
        *sender = Some(tx.clone());
    }

    // Send version message.
    let version_msg = VersionMessage {
        msgtype: "version".to_string(),
        station: "2.0.6(linux/std)".to_string(),
        firmware: format!("rak-basicstation v{} ({} backend)", env!("CARGO_PKG_VERSION"), conf.backend.enabled),
        package: String::new(),
        model: std::env::consts::ARCH.to_string(),
        protocol: 2,
        features: String::new(),
    };
    let version_json = serde_json::to_string(&version_msg)?;
    debug!("Sending version message: {}", version_json);
    ws_write.send(Message::Text(version_json.into())).await?;

    let session = {
        let s = super::SESSION_COUNTER.read().unwrap();
        *s
    };

    // Spawn task to forward outgoing messages.
    let write_handle = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if let Err(e) = ws_write.send(Message::Text(msg.into())).await {
                error!("WebSocket send error: {}", e);
                break;
            }
        }
    });

    // Read incoming messages.
    while let Some(msg) = ws_read.next().await {
        let msg = match msg {
            Ok(m) => m,
            Err(e) => {
                error!("WebSocket read error: {}", e);
                break;
            }
        };

        match msg {
            Message::Text(text) => {
                if let Err(e) =
                    handle_text_message(&text, &tx, gateway_id, session, conf).await
                {
                    error!("Handle text message error: {}", e);
                }
            }
            Message::Close(_) => {
                info!("WebSocket close received");
                break;
            }
            Message::Ping(data) => {
                // Pong is handled by tungstenite automatically.
                debug!("Ping received (auto-pong), data_len: {}", data.len());
            }
            _ => {
                debug!("Ignoring non-text WebSocket message");
            }
        }
    }

    write_handle.abort();
    Ok(())
}

async fn handle_text_message(
    text: &str,
    tx: &mpsc::UnboundedSender<String>,
    _gateway_id: &str,
    session: u8,
    _conf: &Configuration,
) -> Result<()> {
    let generic: GenericMessage = serde_json::from_str(text)?;
    debug!("Received message, msgtype: {}", generic.msgtype);

    match generic.msgtype.as_str() {
        "router_config" => {
            let rc: RouterConfig = serde_json::from_str(text)?;
            info!(
                "Received router_config, region: {}, channels: {}",
                rc.region,
                rc.sx1301_conf.len()
            );

            // Store MuxTime if present.
            if let Some(mt) = rc.mux_time {
                let mut mux_time = LAST_MUX_TIME.write().unwrap();
                *mux_time = Some(mt);
            }

            // Build and store router config state.
            let state = router_config::RouterConfigState::from_router_config(&rc);
            {
                let mut config_state = ROUTER_CONFIG.write().unwrap();
                *config_state = Some(state);
            }

            // Translate to Concentratord configuration and apply.
            let gw_config = router_config::to_gateway_configuration(&rc)?;
            if let Err(e) = crate::backend::send_configuration_command(gw_config).await {
                error!("Failed to send configuration to concentratord: {}", e);
            }
        }

        "dnmsg" => {
            let msg: DownlinkMessage = serde_json::from_str(text)?;

            // Store MuxTime if present.
            if let Some(mt) = msg.mux_time {
                let mut mux_time = LAST_MUX_TIME.write().unwrap();
                *mux_time = Some(mt);
            }

            let rc = {
                let rc = ROUTER_CONFIG.read().unwrap();
                rc.clone()
            };

            let rc = match rc {
                Some(rc) => rc,
                None => {
                    warn!("No router_config, dropping dnmsg");
                    return Ok(());
                }
            };

            match downlink::handle_dnmsg(&msg, &rc, session).await {
                Ok(Some(dntxed_json)) => {
                    tx.send(dntxed_json)
                        .map_err(|e| anyhow!("Send dntxed error: {}", e))?;
                }
                Ok(None) => {}
                Err(e) => {
                    error!("Handle dnmsg error: {}", e);
                }
            }
        }

        "dnsched" => {
            let msg: DownlinkSchedule = serde_json::from_str(text)?;

            if let Some(mt) = msg.mux_time {
                let mut mux_time = LAST_MUX_TIME.write().unwrap();
                *mux_time = Some(mt);
            }

            let rc = {
                let rc = ROUTER_CONFIG.read().unwrap();
                rc.clone()
            };

            let rc = match rc {
                Some(rc) => rc,
                None => {
                    warn!("No router_config, dropping dnsched");
                    return Ok(());
                }
            };

            if let Err(e) = downlink::handle_dnsched(&msg, &rc, session).await {
                error!("Handle dnsched error: {}", e);
            }
        }

        "timesync" => {
            let msg: TimesyncResponse = serde_json::from_str(text)?;

            if let Some(mt) = msg.mux_time {
                let mut mux_time = LAST_MUX_TIME.write().unwrap();
                *mux_time = Some(mt);
            }

            // If server provides xtime + gpstime, update our GPS offset.
            if let (Some(xtime), Some(gpstime)) = (msg.xtime, msg.gpstime) {
                timesync::update_gps_offset(xtime, gpstime);
            }
        }

        "error" => {
            if let Some(msg) = generic.extra.get("error") {
                error!("LNS error: {}", msg);
            } else {
                error!("LNS error (no detail)");
            }
        }

        other => {
            debug!("Unhandled message type: {}", other);
        }
    }

    Ok(())
}

pub fn build_tls_connector(conf: &Configuration) -> Result<Arc<rustls::ClientConfig>> {
    let mut root_store = rustls::RootCertStore::empty();

    // Load system root certificates.
    let native_certs = rustls_native_certs::load_native_certs()
        .certs;
    for cert in native_certs {
        root_store.add(cert)?;
    }

    // Load custom CA cert if configured.
    if !conf.lns.ca_cert.is_empty() {
        let ca_data = fs::read(&conf.lns.ca_cert)?;
        let ca_certs = rustls_pki_types::CertificateDer::pem_slice_iter(ca_data.as_slice())
            .filter_map(|r| r.ok())
            .collect::<Vec<_>>();
        for cert in ca_certs {
            root_store.add(cert)?;
        }
    }

    let builder = rustls::ClientConfig::builder().with_root_certificates(root_store);

    let config = if !conf.lns.tls_cert.is_empty() && !conf.lns.tls_key.is_empty() {
        // Mutual TLS.
        let cert_data = fs::read(&conf.lns.tls_cert)?;
        let key_data = fs::read(&conf.lns.tls_key)?;

        let certs: Vec<_> = rustls_pki_types::CertificateDer::pem_slice_iter(cert_data.as_slice())
            .filter_map(|r| r.ok())
            .collect();
        let key = rustls_pki_types::PrivateKeyDer::from_pem_slice(key_data.as_slice())
            .map_err(|_| anyhow!("No private key found in TLS key file"))?;

        builder.with_client_auth_cert(certs, key)?
    } else {
        builder.with_no_client_auth()
    };

    Ok(Arc::new(config))
}
