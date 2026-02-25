use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, LazyLock, Mutex, RwLock,
};
use std::time::{Duration, Instant};

use anyhow::Result;
use chirpstack_api::gw;
use log::{debug, error, info};
use tokio::sync::mpsc;

use crate::config::{Configuration, Lns};

pub mod discovery;
pub mod downlink;
pub mod messages;
pub mod router_config;
pub mod timesync;
pub mod uplink;
pub mod websocket;

/// Sender for outgoing WebSocket text messages.
static WS_SENDER: LazyLock<RwLock<Option<mpsc::UnboundedSender<String>>>> =
    LazyLock::new(|| RwLock::new(None));

/// Current session counter, incremented on each reconnect.
static SESSION_COUNTER: LazyLock<RwLock<u8>> = LazyLock::new(|| RwLock::new(0));

/// Current router_config state (data rate table, filters, etc.).
static ROUTER_CONFIG: LazyLock<RwLock<Option<router_config::RouterConfigState>>> =
    LazyLock::new(|| RwLock::new(None));

/// Last MuxTime received from LNS (for RefTime echo).
static LAST_MUX_TIME: LazyLock<RwLock<Option<f64>>> = LazyLock::new(|| RwLock::new(None));

/// TC URI provided by CUPS (overrides conf.lns.server when set).
static CUPS_TC_URI: LazyLock<RwLock<Option<String>>> = LazyLock::new(|| RwLock::new(None));

/// Auth headers parsed from the TC credential blob provided by CUPS.
static CUPS_TC_AUTH_HEADERS: LazyLock<RwLock<Vec<(String, String)>>> =
    LazyLock::new(|| RwLock::new(Vec::new()));

/// Whether context caching is enabled (concentratord backend only).
static CONTEXT_CACHING_ENABLED: AtomicBool = AtomicBool::new(false);

/// TTL for cached rx_info contexts.
const CONTEXT_CACHE_TTL: Duration = Duration::from_secs(60);

/// Cache of full rx_info.context blobs, keyed by xtime.
#[allow(clippy::type_complexity)]
static CONTEXT_CACHE: LazyLock<Mutex<HashMap<i64, (Vec<u8>, Instant)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn set_cups_tc_uri(uri: String) {
    *CUPS_TC_URI.write().unwrap() = Some(uri);
}

pub fn set_cups_tc_auth_headers(headers: Vec<(String, String)>) {
    *CUPS_TC_AUTH_HEADERS.write().unwrap() = headers;
}

fn cache_context(xtime: i64, context: Vec<u8>) {
    CONTEXT_CACHE
        .lock()
        .unwrap()
        .insert(xtime, (context, Instant::now()));
}

pub fn get_cached_context(xtime: i64) -> Option<Vec<u8>> {
    if !CONTEXT_CACHING_ENABLED.load(Ordering::Relaxed) {
        return None;
    }
    CONTEXT_CACHE
        .lock()
        .unwrap()
        .get(&xtime)
        .map(|(ctx, _)| ctx.clone())
}

fn sweep_context_cache() {
    let now = Instant::now();
    CONTEXT_CACHE
        .lock()
        .unwrap()
        .retain(|_, (_, inserted)| now.duration_since(*inserted) < CONTEXT_CACHE_TTL);
}

pub async fn setup(conf: &Configuration) -> Result<()> {
    let gateway_id = crate::backend::get_gateway_id().await?;
    let conf = Arc::new(conf.clone());

    if conf.backend.concentratord.context_caching {
        CONTEXT_CACHING_ENABLED.store(true, Ordering::Relaxed);
        info!("Context caching enabled for concentratord backend");
        tokio::spawn(async {
            let mut ticker = tokio::time::interval(Duration::from_secs(30));
            loop {
                ticker.tick().await;
                sweep_context_cache();
            }
        });
    }

    tokio::spawn({
        let conf = conf.clone();
        let gateway_id = gateway_id.clone();
        async move {
            connection_loop(conf, gateway_id).await;
        }
    });

    Ok(())
}

async fn connection_loop(conf: Arc<Configuration>, gateway_id: String) {
    loop {
        // Increment session counter on each connection attempt.
        {
            let mut session = SESSION_COUNTER.write().unwrap();
            *session = session.wrapping_add(1);
        }

        // Resolve the MUXS URI via router discovery.
        // The BasicStation protocol always performs discovery first: the gateway
        // opens a WebSocket to <server>/router-info, sends {"router":"<id6>"},
        // and receives the actual MUXS WebSocket URI in response.
        // Priority: explicit discovery_endpoint > lns.server > CUPS-provided TC URI.
        let discovery_base = if !conf.lns.discovery_endpoint.is_empty() {
            conf.lns.discovery_endpoint.clone()
        } else if !conf.lns.server.is_empty() {
            conf.lns.server.clone()
        } else {
            CUPS_TC_URI.read().unwrap().clone().unwrap_or_default()
        };

        let auth_headers = match parse_auth_token(&conf.lns) {
            Ok(h) => h,
            Err(_e) => {
                error!("Failed to configure auth token: check tls_key config");
                tokio::time::sleep(conf.lns.reconnect_interval).await;
                continue;
            }
        };

        let tls_connector = match websocket::build_tls_connector(&conf) {
            Ok(c) => c,
            Err(e) => {
                error!("Failed to build TLS connector: {}", e);
                tokio::time::sleep(conf.lns.reconnect_interval).await;
                continue;
            }
        };

        let muxs_uri = if !discovery_base.is_empty() {
            info!("Performing router discovery, endpoint: {}", discovery_base);
            match discovery::discover(&discovery_base, &gateway_id, &auth_headers, tls_connector.clone()).await {
                Ok(uri) => {
                    info!("Router discovery succeeded, muxs_uri: {}", uri);
                    uri
                }
                Err(e) => {
                    error!("Router discovery failed: {}", e);
                    tokio::time::sleep(conf.lns.reconnect_interval).await;
                    continue;
                }
            }
        } else {
            String::new()
        };

        if muxs_uri.is_empty() {
            error!("No LNS server URI configured");
            tokio::time::sleep(conf.lns.reconnect_interval).await;
            continue;
        }

        info!("Connecting to LNS, uri: {}", muxs_uri);

        match websocket::run(&conf, &muxs_uri, &gateway_id, &auth_headers).await {
            Ok(()) => {
                info!("WebSocket connection closed");
            }
            Err(e) => {
                error!("WebSocket connection error: {}", e);
            }
        }

        // Clear WS sender on disconnect.
        {
            let mut sender = WS_SENDER.write().unwrap();
            *sender = None;
        }

        // Clear router config on disconnect.
        {
            let mut rc = ROUTER_CONFIG.write().unwrap();
            *rc = None;
        }

        info!(
            "Reconnecting in {:?}",
            conf.lns.reconnect_interval
        );
        tokio::time::sleep(conf.lns.reconnect_interval).await;
    }
}

/// Send an uplink frame to the LNS via the WebSocket connection.
pub async fn send_uplink(frame: &gw::UplinkFrame) -> Result<()> {
    let sender = {
        let s = WS_SENDER.read().unwrap();
        s.clone()
    };

    let sender = match sender {
        Some(s) => s,
        None => {
            debug!("WebSocket not connected, dropping uplink");
            return Ok(());
        }
    };

    let rc = {
        let rc = ROUTER_CONFIG.read().unwrap();
        rc.clone()
    };

    let rc = match rc {
        Some(rc) => rc,
        None => {
            debug!("No router_config received yet, dropping uplink");
            return Ok(());
        }
    };

    let ref_time = {
        let mt = LAST_MUX_TIME.read().unwrap();
        *mt
    };

    let session = {
        let s = SESSION_COUNTER.read().unwrap();
        *s
    };

    if CONTEXT_CACHING_ENABLED.load(Ordering::Relaxed)
        && let Some(rx_info) = &frame.rx_info
        && rx_info.context.len() >= 4
    {
        let count_us = u32::from_be_bytes([
            rx_info.context[0],
            rx_info.context[1],
            rx_info.context[2],
            rx_info.context[3],
        ]) as i64;
        let xtime = ((session as i64) << 48) | (count_us & 0x0000_FFFF_FFFF_FFFF);
        cache_context(xtime, rx_info.context.clone());
    }

    let msg = uplink::frame_to_json(frame, &rc, session, ref_time)?;

    // Clear MuxTime after using it.
    if ref_time.is_some() {
        let mut mt = LAST_MUX_TIME.write().unwrap();
        *mt = None;
    }

    sender
        .send(msg)
        .map_err(|e| anyhow!("Send WebSocket message error: {}", e))?;

    Ok(())
}

/// Send a text message to the LNS via the WebSocket connection.
pub fn send_ws_message(msg: String) -> Result<()> {
    let sender = {
        let s = WS_SENDER.read().unwrap();
        s.clone()
    };

    let sender = match sender {
        Some(s) => s,
        None => {
            debug!("WebSocket not connected, dropping message");
            return Ok(());
        }
    };

    sender
        .send(msg)
        .map_err(|e| anyhow!("Send WebSocket message error: {}", e))?;

    Ok(())
}

/// Parse auth token headers from the tls_key file.
///
/// In BasicStation's token auth mode (tls_cert empty, tls_key set), the tls_key
/// file contains HTTP headers (e.g. "Authorization: Bearer <token>") that are
/// sent on both discovery and WebSocket requests.
///
/// Returns a vec of (header_name, header_value) pairs.
fn parse_auth_token(lns: &Lns) -> Result<Vec<(String, String)>> {
    // Config-based token auth: tls_cert empty, tls_key set.
    // tls_key file contains just the raw API key (no "Bearer " prefix).
    if lns.tls_cert.is_empty() && !lns.tls_key.is_empty() {
        let token = std::fs::read_to_string(&lns.tls_key)?.trim().to_string();
        debug!("Using config-based auth token from {}", lns.tls_key);
        return Ok(vec![(
            "Authorization".to_string(),
            format!("Bearer {}", token),
        )]);
    }

    // CUPS-provided auth headers (parsed from the TC credential blob).
    let headers = CUPS_TC_AUTH_HEADERS.read().unwrap().clone();
    if !headers.is_empty() {
        debug!("Using CUPS-provided TC auth headers");
    }
    Ok(headers)
}
