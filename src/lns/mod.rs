use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicBool, AtomicI64, Ordering},
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
pub(crate) static CONTEXT_CACHING_ENABLED: AtomicBool = AtomicBool::new(false);

/// Serializes tests that mutate the shared cache statics.
#[cfg(test)]
pub(crate) static CACHE_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Clears the context cache; used by tests to establish a known-empty state.
#[cfg(test)]
pub(crate) fn clear_context_cache() {
    CONTEXT_CACHE.lock().unwrap().clear();
}

/// TTL for cached rx_info contexts.
const CONTEXT_CACHE_TTL: Duration = Duration::from_secs(60);

/// ChirpStack Gateway Mesh replaces `rx_info.context` on a relayed uplink with
/// `[1,2,3] + relay_id(4) + uplink_id(2)`.
const MESH_CTX_PREFIX: [u8; 3] = [1, 2, 3];
const MESH_CTX_LEN: usize = MESH_CTX_PREFIX.len() + 6;

pub(crate) fn is_mesh_context(context: &[u8]) -> bool {
    context.len() == MESH_CTX_LEN && context[..MESH_CTX_PREFIX.len()] == MESH_CTX_PREFIX
}

/// Discriminator bit for synthesized counters. A real concentrator `count_us` is a `u32`
/// and so never reaches bit 47, which keeps the two numbering spaces disjoint.
const SYNTHETIC_XTIME_FLAG: i64 = 1 << 47;

/// Monotonic source for synthesized counters, in microseconds since process start.
static XTIME_EPOCH: LazyLock<Instant> = LazyLock::new(Instant::now);
/// Last synthesized counter, to guarantee strict monotonicity.
static LAST_SYNTHETIC_US: AtomicI64 = AtomicI64::new(0);

/// Reconstruct the `xtime` for an uplink.
///
/// For a plain concentratord context the first four bytes really are a big-endian
/// `count_us`, and that value is used unchanged -- direct (non-mesh) deployments keep
/// their existing on-the-wire behaviour exactly.
///
/// A mesh-relayed context carries no timestamp at all: the mesh proxy overwrote it, and
/// `UplinkRxInfo` has no other counter field. Deriving `count_us` from those bytes yields
/// `[1,2,3,relay_id[0]]`, a constant, so every uplink collapses onto one cache key. Instead
/// synthesize a strictly increasing microsecond counter. This is safe because `xtime` is an
/// opaque correlation token to the LNS and downlinks are scheduled with `Delay` timing
/// relative to the context, never from `xtime`.
pub(crate) fn uplink_xtime(session: u8, context: &[u8]) -> i64 {
    let counter = if is_mesh_context(context) || context.len() < 4 {
        let now_us = XTIME_EPOCH.elapsed().as_micros() as i64 & 0x0000_7FFF_FFFF_FFFF;
        // Strictly increasing even if two uplinks land in the same microsecond.
        let unique = LAST_SYNTHETIC_US
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |prev| {
                Some(if now_us > prev { now_us } else { prev + 1 })
            })
            .map(|prev| if now_us > prev { now_us } else { prev + 1 })
            .unwrap_or(now_us);
        SYNTHETIC_XTIME_FLAG | unique
    } else {
        u32::from_be_bytes([context[0], context[1], context[2], context[3]]) as i64
    };

    ((session as i64) << 48) | (counter & 0x0000_FFFF_FFFF_FFFF)
}

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

/// Stores a context blob and returns the resulting number of cached entries.
pub(crate) fn cache_context(xtime: i64, context: Vec<u8>) -> usize {
    let mut cache = CONTEXT_CACHE.lock().unwrap();
    cache.insert(xtime, (context, Instant::now()));
    cache.len()
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
    let mut cache = CONTEXT_CACHE.lock().unwrap();
    let before = cache.len();
    cache.retain(|_, (_, inserted)| now.duration_since(*inserted) < CONTEXT_CACHE_TTL);
    let expired = before - cache.len();

    if expired > 0 {
        debug!(
            "Swept context cache, expired: {}, entries: {}",
            expired,
            cache.len()
        );
    }
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

    // Single source of truth: the same xtime is cached as the key and reported to the LNS,
    // so the insert key and the value the LNS echoes back can never diverge.
    let context: &[u8] = frame
        .rx_info
        .as_ref()
        .map(|rx| rx.context.as_slice())
        .unwrap_or(&[]);
    let xtime = uplink_xtime(session, context);

    if CONTEXT_CACHING_ENABLED.load(Ordering::Relaxed)
        && let Some(rx_info) = &frame.rx_info
        && !rx_info.context.is_empty()
    {
        let entries = cache_context(xtime, rx_info.context.clone());

        debug!(
            "Cached uplink context, xtime: {}, synthetic: {}, context: {}, len: {}, entries: {}",
            xtime,
            is_mesh_context(&rx_info.context),
            hex::encode(&rx_info.context),
            rx_info.context.len(),
            entries
        );
    }

    let msg = uplink::frame_to_json(frame, &rc, session, ref_time, xtime)?;

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

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    /// Reset the shared statics to a defined state before each test.
    fn setup(caching: bool) -> std::sync::MutexGuard<'static, ()> {
        let guard = CACHE_TEST_LOCK.lock().unwrap();
        clear_context_cache();
        CONTEXT_CACHING_ENABLED.store(caching, Ordering::Relaxed);
        guard
    }

    // -----------------------------------------------------------------------
    // Cache primitive tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_cache_hit() {
        let _g = setup(true);
        let xtime: i64 = 0x0001_AABB_CCDD_EE01;
        let ctx = vec![0x01, 0x02, 0x03, 0x04, 0xAA, 0xBB];
        cache_context(xtime, ctx.clone());
        assert_eq!(get_cached_context(xtime), Some(ctx));
    }

    #[test]
    fn test_cache_miss() {
        let _g = setup(true);
        assert_eq!(get_cached_context(0x0001_DEAD_BEEF_0000), None);
    }

    #[test]
    fn test_caching_disabled_returns_none() {
        let _g = setup(false);
        let xtime: i64 = 0x0001_0000_0000_0042;
        // Insert directly so the entry exists in the map.
        CONTEXT_CACHE
            .lock()
            .unwrap()
            .insert(xtime, (vec![0xAA, 0xBB, 0xCC, 0xDD], Instant::now()));
        // The disabled flag must suppress the lookup.
        assert_eq!(get_cached_context(xtime), None);
    }

    #[test]
    fn test_sweep_removes_expired_entry() {
        let _g = setup(true);
        let xtime: i64 = 0x0001_0000_DEAD_0001;
        // Insert with a timestamp already past the TTL.
        CONTEXT_CACHE.lock().unwrap().insert(
            xtime,
            (
                vec![1, 2, 3, 4],
                Instant::now() - CONTEXT_CACHE_TTL - Duration::from_secs(1),
            ),
        );
        sweep_context_cache();
        assert_eq!(get_cached_context(xtime), None);
    }

    #[test]
    fn test_sweep_retains_fresh_entry() {
        let _g = setup(true);
        let xtime: i64 = 0x0001_0000_CAFE_0001;
        let ctx = vec![5, 6, 7, 8];
        cache_context(xtime, ctx.clone());
        sweep_context_cache();
        assert_eq!(get_cached_context(xtime), Some(ctx));
    }

    // -----------------------------------------------------------------------
    // Defect 1: distinct xtime per uplink (RQ-003)
    // -----------------------------------------------------------------------

    /// A mesh-relayed context: [1,2,3] + relay_id(4) + uplink_id(2).
    fn mesh_ctx(uplink_id: u16) -> Vec<u8> {
        let mut c = vec![1, 2, 3, 0xf1, 0x0c, 0xab, 0x4e];
        c.extend_from_slice(&uplink_id.to_be_bytes());
        c
    }

    /// The pre-fix derivation, kept verbatim so the regression is expressed as a
    /// behavioural difference rather than a compile error.
    fn legacy_xtime(session: u8, context: &[u8]) -> i64 {
        let count_us =
            u32::from_be_bytes([context[0], context[1], context[2], context[3]]) as i64;
        ((session as i64) << 48) | (count_us & 0x0000_FFFF_FFFF_FFFF)
    }

    #[test]
    fn test_legacy_derivation_collapses_mesh_uplinks_onto_one_key() {
        let _g = setup(true);
        // Every relayed uplink from one relay parsed to [1,2,3,relay_id[0]] = 0x010203F1.
        for id in [0x02ee_u16, 0x02ef, 0x02f0, 0x02f1] {
            let ctx = mesh_ctx(id);
            cache_context(legacy_xtime(1, &ctx), ctx);
        }
        assert_eq!(
            CONTEXT_CACHE.lock().unwrap().len(),
            1,
            "pre-fix behaviour: all mesh uplinks collide on a single cache key"
        );
    }

    #[test]
    fn test_mesh_uplinks_get_distinct_keys() {
        let _g = setup(true);
        let ids = [0x02ee_u16, 0x02ef, 0x02f0, 0x02f1];
        for id in ids {
            let ctx = mesh_ctx(id);
            cache_context(uplink_xtime(1, &ctx), ctx);
        }
        assert_eq!(
            CONTEXT_CACHE.lock().unwrap().len(),
            ids.len(),
            "each mesh uplink must occupy its own cache entry"
        );
    }

    #[test]
    fn test_mesh_xtime_is_flagged_and_monotonic() {
        let a = uplink_xtime(1, &mesh_ctx(1));
        let b = uplink_xtime(1, &mesh_ctx(2));
        assert!(a & SYNTHETIC_XTIME_FLAG != 0, "synthetic values carry bit 47");
        assert!(b > a, "synthesized counters strictly increase");
    }

    #[test]
    fn test_direct_context_keeps_real_count_us() {
        // A plain 4-byte concentratord context must round-trip unchanged, so direct
        // deployments keep their existing on-the-wire xtime.
        let ctx = vec![0x0f, 0xcd, 0x2e, 0x3c];
        let xtime = uplink_xtime(1, &ctx);
        assert_eq!(xtime, legacy_xtime(1, &ctx));
        assert_eq!(xtime & SYNTHETIC_XTIME_FLAG, 0, "not flagged as synthetic");
    }

    #[test]
    fn test_synthetic_and_real_spaces_cannot_collide() {
        // Real count_us is a u32, so it can never reach bit 47.
        let real = uplink_xtime(1, &[0xff, 0xff, 0xff, 0xff]);
        let synthetic = uplink_xtime(1, &mesh_ctx(1));
        assert_eq!(real & SYNTHETIC_XTIME_FLAG, 0);
        assert!(synthetic & SYNTHETIC_XTIME_FLAG != 0);
        assert_ne!(real, synthetic);
    }

    // -----------------------------------------------------------------------
    // xtime encoding round-trip
    // -----------------------------------------------------------------------

    /// The xtime built in send_uplink() must round-trip through the mask used
    /// in build_class_a_downlink() to recover count_us.
    #[test]
    fn test_xtime_encoding_roundtrip() {
        let session: u8 = 0xAB;
        let count_us: u32 = 0x00CA_FE12;
        let xtime = ((session as i64) << 48) | (count_us as i64 & 0x0000_FFFF_FFFF_FFFF);
        let recovered = (xtime & 0x0000_FFFF_FFFF_FFFF) as u32;
        assert_eq!(recovered, count_us);
    }

    /// Session counter in bits [55:48] must not bleed into count_us bits [47:0].
    #[test]
    fn test_xtime_session_bits_are_isolated() {
        for session in [0u8, 1, 127, 255] {
            let count_us: u32 = 0xFFFF_FFFF;
            let xtime = ((session as i64) << 48) | (count_us as i64 & 0x0000_FFFF_FFFF_FFFF);
            let recovered_count = (xtime & 0x0000_FFFF_FFFF_FFFF) as u32;
            let recovered_session = ((xtime >> 48) & 0xFF) as u8;
            assert_eq!(recovered_count, count_us);
            assert_eq!(recovered_session, session);
        }
    }
}
