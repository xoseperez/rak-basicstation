use anyhow::Result;
use chirpstack_api::{gw, pbjson_types};
use log::{debug, info, warn};

use super::messages::{DnTxedMessage, DownlinkMessage, DownlinkSchedule};
use super::router_config::RouterConfigState;

/// Process a dnmsg from the LNS.
/// Returns a dntxed JSON string on success.
pub async fn handle_dnmsg(
    msg: &DownlinkMessage,
    rc: &RouterConfigState,
    _session: u8,
) -> Result<Option<String>> {
    let dc = msg.dc.unwrap_or(0);
    let diid = msg.diid.unwrap_or(0);
    let dev_eui = msg.dev_eui.clone().unwrap_or_default();
    let pdu = msg
        .pdu
        .as_ref()
        .ok_or_else(|| anyhow!("dnmsg missing pdu"))?;
    let phy_payload = hex::decode(pdu)?;

    info!(
        "Received dnmsg, dC: {}, diid: {}, DevEui: {}",
        dc, diid, dev_eui
    );

    let downlink_frame = match dc {
        // Class A
        0 => build_class_a_downlink(msg, rc, &phy_payload)?,
        // Class B
        1 => build_class_b_downlink(msg, rc, &phy_payload)?,
        // Class C
        2 => build_class_c_downlink(msg, rc, &phy_payload)?,
        _ => {
            warn!("Unknown downlink class: {}", dc);
            return Ok(None);
        }
    };

    let _tx_ack = crate::backend::send_downlink_frame(downlink_frame).await?;

    // Build dntxed confirmation.
    let xtime = msg.xtime.unwrap_or(0);
    let rctx = msg.rctx.unwrap_or(0);
    let gpstime = msg.gpstime.unwrap_or(0);

    let dntxed = DnTxedMessage {
        msgtype: "dntxed".to_string(),
        diid,
        dev_eui,
        rctx,
        xtime,
        txtime: 0.0,
        gpstime,
    };

    Ok(Some(serde_json::to_string(&dntxed)?))
}

/// Process a dnsched from the LNS (multicast/beacon schedule).
pub async fn handle_dnsched(
    msg: &DownlinkSchedule,
    rc: &RouterConfigState,
    _session: u8,
) -> Result<()> {
    for entry in &msg.schedule {
        let pdu = match &entry.pdu {
            Some(p) => hex::decode(p)?,
            None => continue,
        };
        let dr = entry.dr.unwrap_or(0);
        let freq = entry.freq.unwrap_or(0);
        let gpstime = entry.gpstime.unwrap_or(0);

        let (sf, bw_hz) = rc
            .dr_to_sf_bw(dr)
            .ok_or_else(|| anyhow!("Unknown DR: {}", dr))?;

        let mut items = Vec::new();

        if sf > 0 {
            // LoRa
            items.push(gw::DownlinkFrameItem {
                phy_payload: pdu,
                tx_info: Some(gw::DownlinkTxInfo {
                    frequency: freq,
                    power: 0,
                    modulation: Some(gw::Modulation {
                        parameters: Some(gw::modulation::Parameters::Lora(
                            gw::LoraModulationInfo {
                                bandwidth: bw_hz,
                                spreading_factor: sf,
                                code_rate: gw::CodeRate::Cr45.into(),
                                polarization_inversion: true,
                                ..Default::default()
                            },
                        )),
                    }),
                    timing: Some(gw::Timing {
                        parameters: Some(gw::timing::Parameters::GpsEpoch(
                            gw::GpsEpochTimingInfo {
                                time_since_gps_epoch: Some(pbjson_types::Duration {
                                    seconds: gpstime / 1_000_000,
                                    nanos: ((gpstime % 1_000_000) * 1000) as i32,
                                }),
                            },
                        )),
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            });
        }

        if !items.is_empty() {
            let dl = gw::DownlinkFrame {
                downlink_id: 0,
                items,
                ..Default::default()
            };

            debug!("Sending scheduled downlink, freq: {}, DR: {}", freq, dr);
            crate::backend::send_downlink_frame(dl).await?;
        }
    }

    Ok(())
}

/// ChirpStack Gateway Mesh tags relayed uplinks with `[1,2,3] + relay_id(4) + uplink_id(2)`.
/// It relays a downlink only when the context has exactly that shape; anything else is
/// transmitted locally without a TX power lookup.
const MESH_CTX_PREFIX: [u8; 3] = [1, 2, 3];
const MESH_CTX_LEN: usize = MESH_CTX_PREFIX.len() + 6;

fn is_mesh_context(context: &[u8]) -> bool {
    context.len() == MESH_CTX_LEN && context[..MESH_CTX_PREFIX.len()] == MESH_CTX_PREFIX
}

/// TX power (dBm) for a downlink item.
///
/// Only mesh-relayed downlinks need a real value: the relay resolves the requested power
/// against its configured table and rejects anything below the floor, which is why the
/// legacy hardcoded `0` failed with "No TX Power equal or lower than: 0". The direct
/// concentratord path works with `0` today and is deliberately left untouched.
fn downlink_power(rc: &RouterConfigState, context: &[u8]) -> i32 {
    if is_mesh_context(context) {
        rc.tx_power_dbm
    } else {
        0
    }
}

/// Largest whole-second offset probed between the uplink `xtime` and the one echoed by
/// the LNS. LoRaWAN caps RxDelay at 15 s, so no legitimate offset exceeds this.
const MAX_XTIME_OFFSET_SECS: i64 = 15;

/// Outcome of matching a downlink back to the uplink that caused it.
struct ResolvedContext {
    /// Context to attach: the cached blob on a hit, else the legacy 4-byte count_us.
    context: Vec<u8>,
    /// Whole seconds the LNS added to the uplink `xtime`. `0` on a verbatim echo and on
    /// a miss. Must be added to `RxDelay` to get the true RX1 delay.
    offset_secs: i64,
}

/// Resolve the concentrator context for a downlink referencing `xtime`.
///
/// Not every LNS echoes the uplink `xtime` verbatim. ChirpStack does, and puts the whole
/// window in `RxDelay`. TTN instead returns `uplink_xtime + 4s` alongside `RxDelay: 1`,
/// so the *sum* is the RX1 delay -- which is what the reference station computes
/// (`s2e.c:1487`: `txjob->xtime += rxdelay * 1000000`).
///
/// So the lookup probes whole-second offsets. The matched offset is returned because it is
/// also needed for scheduling: transmitting at `RxDelay` alone would be 4 s early on TTN.
///
/// If more than one cached uplink matches a candidate offset the match is **refused**:
/// restoring the wrong relay context (and scheduling against the wrong uplink) is worse
/// than falling back, because it misroutes silently instead of failing visibly.
fn resolve_context(xtime: i64, count_us: u32, rx_delay: u32, class: &str) -> ResolvedContext {
    let fallback = ResolvedContext {
        context: count_us.to_be_bytes().to_vec(),
        offset_secs: 0,
    };

    // Offsets that could still yield a legal total window, smallest first.
    let max_offset = MAX_XTIME_OFFSET_SECS.saturating_sub(rx_delay as i64).max(0);
    let mut matches: Vec<(i64, Vec<u8>)> = Vec::new();
    for offset in 0..=max_offset {
        let candidate = xtime - offset * 1_000_000;
        if let Some(context) = super::get_cached_context(candidate) {
            matches.push((offset, context));
        }
    }

    match matches.len() {
        0 => fallback,
        1 => {
            let (offset_secs, context) = matches.into_iter().next().unwrap();
            debug!(
                "Class {} downlink, context cache hit, xtime: {}, offset: {}s, rx_delay: {}s, context: {}, len: {}",
                class,
                xtime,
                offset_secs,
                rx_delay,
                hex::encode(&context),
                context.len()
            );
            ResolvedContext {
                context,
                offset_secs,
            }
        }
        n => {
            // Two uplinks fell inside the probe window; we cannot tell which caused this
            // downlink. Decline rather than guess.
            warn!(
                "Class {} downlink, context cache ambiguous ({} candidates), xtime: {}, falling back",
                class, n, xtime
            );
            fallback
        }
    }
}

fn build_class_a_downlink(
    msg: &DownlinkMessage,
    rc: &RouterConfigState,
    phy_payload: &[u8],
) -> Result<gw::DownlinkFrame> {
    let rx_delay = msg.rx_delay.unwrap_or(1) as u32;

    // Extract concentrator counter from xtime (bits 47-0 contain count_us).
    // BasicStation protocol uses xtime for timing, not rctx.
    let xtime = msg
        .xtime
        .ok_or_else(|| anyhow!("Class A dnmsg missing xtime"))?;
    let count_us = (xtime & 0x0000_FFFF_FFFF_FFFF) as u32;

    debug!(
        "Class A downlink, xtime: {}, count_us: {}, rx_delay: {}, rctx: {:?}",
        xtime, count_us, rx_delay, msg.rctx
    );

    let resolved = resolve_context(xtime, count_us, rx_delay, "A");
    let context = resolved.context;
    let power = downlink_power(rc, &context);
    // True RX1 delay = whatever the LNS folded into xtime + the RxDelay it sent.
    let rx1_delay = rx_delay as i64 + resolved.offset_secs;

    let mut items = Vec::new();

    // RX1 window.
    if let (Some(rx1_dr), Some(rx1_freq)) = (msg.rx1_dr, msg.rx1_freq)
        && let Some((sf, bw_hz)) = rc.dr_to_sf_bw(rx1_dr) {
            items.push(build_downlink_item(
                phy_payload,
                rx1_freq,
                sf,
                bw_hz,
                gw::Timing {
                    parameters: Some(gw::timing::Parameters::Delay(gw::DelayTimingInfo {
                        delay: Some(pbjson_types::Duration {
                            seconds: rx1_delay,
                            nanos: 0,
                        }),
                    })),
                },
                context.clone(),
                power,
            ));
        }

    // RX2 window.
    if let (Some(rx2_dr), Some(rx2_freq)) = (msg.rx2_dr, msg.rx2_freq)
        && let Some((sf, bw_hz)) = rc.dr_to_sf_bw(rx2_dr) {
            items.push(build_downlink_item(
                phy_payload,
                rx2_freq,
                sf,
                bw_hz,
                gw::Timing {
                    parameters: Some(gw::timing::Parameters::Delay(gw::DelayTimingInfo {
                        delay: Some(pbjson_types::Duration {
                            seconds: rx1_delay + 1,
                            nanos: 0,
                        }),
                    })),
                },
                context.clone(),
                power,
            ));
        }

    Ok(gw::DownlinkFrame {
        downlink_id: msg.diid.unwrap_or(0) as u32,
        items,
        ..Default::default()
    })
}

fn build_class_b_downlink(
    msg: &DownlinkMessage,
    rc: &RouterConfigState,
    phy_payload: &[u8],
) -> Result<gw::DownlinkFrame> {
    let dr = msg.dr.unwrap_or(0);
    let freq = msg.freq.unwrap_or(0);
    let gpstime = msg.gpstime.unwrap_or(0);

    let (sf, bw_hz) = rc
        .dr_to_sf_bw(dr)
        .ok_or_else(|| anyhow!("Unknown DR: {}", dr))?;

    let items = vec![gw::DownlinkFrameItem {
        phy_payload: phy_payload.to_vec(),
        tx_info: Some(gw::DownlinkTxInfo {
            frequency: freq,
            power: 0,
            modulation: Some(gw::Modulation {
                parameters: Some(gw::modulation::Parameters::Lora(
                    gw::LoraModulationInfo {
                        bandwidth: bw_hz,
                        spreading_factor: sf,
                        code_rate: gw::CodeRate::Cr45.into(),
                        polarization_inversion: true,
                        ..Default::default()
                    },
                )),
            }),
            timing: Some(gw::Timing {
                parameters: Some(gw::timing::Parameters::GpsEpoch(
                    gw::GpsEpochTimingInfo {
                        time_since_gps_epoch: Some(pbjson_types::Duration {
                            seconds: gpstime / 1_000_000,
                            nanos: ((gpstime % 1_000_000) * 1000) as i32,
                        }),
                    },
                )),
            }),
            ..Default::default()
        }),
        ..Default::default()
    }];

    Ok(gw::DownlinkFrame {
        downlink_id: msg.diid.unwrap_or(0) as u32,
        items,
        ..Default::default()
    })
}

fn build_class_c_downlink(
    msg: &DownlinkMessage,
    rc: &RouterConfigState,
    phy_payload: &[u8],
) -> Result<gw::DownlinkFrame> {
    let mut items = Vec::new();

    // If xtime is present, this is a Class C response to an uplink (schedule like Class A).
    if let Some(xtime) = msg.xtime {
        let count_us = (xtime & 0x0000_FFFF_FFFF_FFFF) as u32;
        let rx_delay = msg.rx_delay.unwrap_or(1) as u32;

        debug!(
            "Class C downlink, xtime: {}, count_us: {}, rx_delay: {}, rctx: {:?}",
            xtime, count_us, rx_delay, msg.rctx
        );

        let resolved = resolve_context(xtime, count_us, rx_delay, "C");
        let context = resolved.context;
        let power = downlink_power(rc, &context);
        let rx1_delay = rx_delay as i64 + resolved.offset_secs;

        // RX1 window.
        if let (Some(rx1_dr), Some(rx1_freq)) = (msg.rx1_dr, msg.rx1_freq)
            && let Some((sf, bw_hz)) = rc.dr_to_sf_bw(rx1_dr) {
                items.push(build_downlink_item(
                    phy_payload,
                    rx1_freq,
                    sf,
                    bw_hz,
                    gw::Timing {
                        parameters: Some(gw::timing::Parameters::Delay(gw::DelayTimingInfo {
                            delay: Some(pbjson_types::Duration {
                                seconds: rx1_delay,
                                nanos: 0,
                            }),
                        })),
                    },
                    context.clone(),
                    power,
                ));
            }

        // RX2 window.
        if let (Some(rx2_dr), Some(rx2_freq)) = (msg.rx2_dr, msg.rx2_freq)
            && let Some((sf, bw_hz)) = rc.dr_to_sf_bw(rx2_dr) {
                items.push(build_downlink_item(
                    phy_payload,
                    rx2_freq,
                    sf,
                    bw_hz,
                    gw::Timing {
                        parameters: Some(gw::timing::Parameters::Delay(gw::DelayTimingInfo {
                            delay: Some(pbjson_types::Duration {
                                seconds: rx1_delay + 1,
                                nanos: 0,
                            }),
                        })),
                    },
                    context.clone(),
                    power,
                ));
            }
    } else {
        // Unsolicited Class C: immediate transmission on RX2.
        if let (Some(rx2_dr), Some(rx2_freq)) = (msg.rx2_dr, msg.rx2_freq)
            && let Some((sf, bw_hz)) = rc.dr_to_sf_bw(rx2_dr) {
                items.push(gw::DownlinkFrameItem {
                    phy_payload: phy_payload.to_vec(),
                    tx_info: Some(gw::DownlinkTxInfo {
                        frequency: rx2_freq,
                        power: 0,
                        modulation: Some(gw::Modulation {
                            parameters: Some(gw::modulation::Parameters::Lora(
                                gw::LoraModulationInfo {
                                    bandwidth: bw_hz,
                                    spreading_factor: sf,
                                    code_rate: gw::CodeRate::Cr45.into(),
                                    polarization_inversion: true,
                                    ..Default::default()
                                },
                            )),
                        }),
                        timing: Some(gw::Timing {
                            parameters: Some(gw::timing::Parameters::Immediately(
                                gw::ImmediatelyTimingInfo {},
                            )),
                        }),
                        ..Default::default()
                    }),
                    ..Default::default()
                });
            }
    }

    Ok(gw::DownlinkFrame {
        downlink_id: msg.diid.unwrap_or(0) as u32,
        items,
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lns::router_config::RouterConfigState;

    // EU868-like DR table: DR0=SF12/125kHz … DR5=SF7/125kHz, DR6=SF7/250kHz.
    fn eu868_rc() -> RouterConfigState {
        RouterConfigState {
            // (SF, BW_kHz) — dr_to_sf_bw() multiplies BW by 1000.
            drs: vec![
                (12, 125),
                (11, 125),
                (10, 125),
                (9, 125),
                (8, 125),
                (7, 125),
                (7, 250),
            ],
            net_ids: vec![],
            join_eui_ranges: vec![],
            freq_range: (863_000_000, 870_000_000),
            region: "EU868".to_string(),
            tx_power_dbm: 16,
        }
    }

    fn make_class_a_msg(xtime: i64) -> DownlinkMessage {
        serde_json::from_str(&format!(
            r#"{{
                "msgtype": "dnmsg",
                "DevEui": "0101010101010101",
                "dC": 0,
                "diid": 1,
                "pdu": "DEADBEEF",
                "RxDelay": 1,
                "RX1DR": 0,
                "RX1Freq": 868100000,
                "RX2DR": 0,
                "RX2Freq": 869525000,
                "xtime": {}
            }}"#,
            xtime
        ))
        .unwrap()
    }

    fn make_class_c_msg_with_xtime(xtime: i64) -> DownlinkMessage {
        serde_json::from_str(&format!(
            r#"{{
                "msgtype": "dnmsg",
                "DevEui": "0101010101010101",
                "dC": 2,
                "diid": 2,
                "pdu": "DEADBEEF",
                "RxDelay": 1,
                "RX1DR": 0,
                "RX1Freq": 868100000,
                "RX2DR": 0,
                "RX2Freq": 869525000,
                "xtime": {}
            }}"#,
            xtime
        ))
        .unwrap()
    }

    // -----------------------------------------------------------------------
    // Class A
    // -----------------------------------------------------------------------

    #[test]
    fn test_class_a_uses_cached_context() {
        let _g = crate::lns::CACHE_TEST_LOCK.lock().unwrap();
        crate::lns::clear_context_cache();
        crate::lns::CONTEXT_CACHING_ENABLED
            .store(true, std::sync::atomic::Ordering::Relaxed);

        let session: u8 = 0x01;
        let count_us: u32 = 0x0000_1234;
        let xtime = ((session as i64) << 48) | count_us as i64;
        let full_ctx = vec![0x00, 0x00, 0x12, 0x34, 0xAA, 0xBB, 0xCC, 0xDD];
        super::super::cache_context(xtime, full_ctx.clone());

        let msg = make_class_a_msg(xtime);
        let rc = eu868_rc();
        let frame = build_class_a_downlink(&msg, &rc, &[0xDE, 0xAD, 0xBE, 0xEF]).unwrap();

        for item in &frame.items {
            let ctx = item.tx_info.as_ref().unwrap().context.clone();
            assert_eq!(ctx, full_ctx, "expected full cached context in tx_info");
        }
    }

    #[test]
    fn test_class_a_falls_back_to_count_us_on_cache_miss() {
        let _g = crate::lns::CACHE_TEST_LOCK.lock().unwrap();
        crate::lns::clear_context_cache();
        crate::lns::CONTEXT_CACHING_ENABLED
            .store(true, std::sync::atomic::Ordering::Relaxed);

        let session: u8 = 0x02;
        let count_us: u32 = 0x0000_5678;
        let xtime = ((session as i64) << 48) | count_us as i64;
        // No cache entry for this xtime.

        let msg = make_class_a_msg(xtime);
        let rc = eu868_rc();
        let frame = build_class_a_downlink(&msg, &rc, &[0xDE, 0xAD, 0xBE, 0xEF]).unwrap();

        let expected_fallback = count_us.to_be_bytes().to_vec();
        for item in &frame.items {
            let ctx = item.tx_info.as_ref().unwrap().context.clone();
            assert_eq!(ctx, expected_fallback, "expected 4-byte count_us fallback");
        }
    }

    // -----------------------------------------------------------------------
    // Class C (with xtime)
    // -----------------------------------------------------------------------

    #[test]
    fn test_class_c_uses_cached_context() {
        let _g = crate::lns::CACHE_TEST_LOCK.lock().unwrap();
        crate::lns::clear_context_cache();
        crate::lns::CONTEXT_CACHING_ENABLED
            .store(true, std::sync::atomic::Ordering::Relaxed);

        let session: u8 = 0x03;
        let count_us: u32 = 0x0000_ABCD;
        let xtime = ((session as i64) << 48) | count_us as i64;
        let full_ctx = vec![0x00, 0x00, 0xAB, 0xCD, 0x11, 0x22, 0x33, 0x44];
        super::super::cache_context(xtime, full_ctx.clone());

        let msg = make_class_c_msg_with_xtime(xtime);
        let rc = eu868_rc();
        let frame = build_class_c_downlink(&msg, &rc, &[0xDE, 0xAD, 0xBE, 0xEF]).unwrap();

        for item in &frame.items {
            let ctx = item.tx_info.as_ref().unwrap().context.clone();
            assert_eq!(ctx, full_ctx, "expected full cached context in tx_info");
        }
    }

    #[test]
    fn test_class_c_falls_back_to_count_us_on_cache_miss() {
        let _g = crate::lns::CACHE_TEST_LOCK.lock().unwrap();
        crate::lns::clear_context_cache();
        crate::lns::CONTEXT_CACHING_ENABLED
            .store(true, std::sync::atomic::Ordering::Relaxed);

        let session: u8 = 0x04;
        let count_us: u32 = 0x0000_EF01;
        let xtime = ((session as i64) << 48) | count_us as i64;

        let msg = make_class_c_msg_with_xtime(xtime);
        let rc = eu868_rc();
        let frame = build_class_c_downlink(&msg, &rc, &[0xDE, 0xAD, 0xBE, 0xEF]).unwrap();

        let expected_fallback = count_us.to_be_bytes().to_vec();
        for item in &frame.items {
            let ctx = item.tx_info.as_ref().unwrap().context.clone();
            assert_eq!(ctx, expected_fallback, "expected 4-byte count_us fallback");
        }
    }
}

fn build_downlink_item(
    phy_payload: &[u8],
    frequency: u32,
    sf: u32,
    bw_hz: u32,
    timing: gw::Timing,
    context: Vec<u8>,
    power: i32,
) -> gw::DownlinkFrameItem {
    gw::DownlinkFrameItem {
        phy_payload: phy_payload.to_vec(),
        tx_info: Some(gw::DownlinkTxInfo {
            frequency,
            power,
            modulation: Some(gw::Modulation {
                parameters: Some(gw::modulation::Parameters::Lora(
                    gw::LoraModulationInfo {
                        bandwidth: bw_hz,
                        spreading_factor: sf,
                        code_rate: gw::CodeRate::Cr45.into(),
                        polarization_inversion: true,
                        ..Default::default()
                    },
                )),
            }),
            timing: Some(timing),
            context,
            ..Default::default()
        }),
        ..Default::default()
    }
}

#[cfg(test)]
mod power_tests {
    use super::*;
    use crate::lns::router_config::RouterConfigState;

    fn rc_with_power(tx_power_dbm: i32) -> RouterConfigState {
        RouterConfigState {
            drs: vec![],
            net_ids: vec![],
            join_eui_ranges: vec![],
            freq_range: (863_000_000, 870_000_000),
            region: "EU868".to_string(),
            tx_power_dbm,
        }
    }

    /// A mesh-relayed context: [1,2,3] + relay_id(4) + uplink_id(2).
    fn mesh_ctx() -> Vec<u8> {
        vec![1, 2, 3, 0xf1, 0x0c, 0xab, 0x4e, 0x02, 0xee]
    }

    #[test]
    fn test_mesh_context_detected() {
        assert!(is_mesh_context(&mesh_ctx()));
    }

    #[test]
    fn test_non_mesh_contexts_rejected() {
        // The 4-byte count_us fallback used by the direct concentratord path.
        assert!(!is_mesh_context(&[0x01, 0x02, 0x03, 0xf1]));
        // Right prefix, wrong length.
        assert!(!is_mesh_context(&[1, 2, 3, 4, 5]));
        // Right length, wrong prefix.
        assert!(!is_mesh_context(&[9, 9, 9, 0, 0, 0, 0, 0, 0]));
        assert!(!is_mesh_context(&[]));
    }

    #[test]
    fn test_mesh_downlink_gets_real_power() {
        assert_eq!(downlink_power(&rc_with_power(16), &mesh_ctx()), 16);
    }

    #[test]
    fn test_direct_path_power_unchanged() {
        // RQ-003d: the direct concentratord path keeps emitting 0 regardless of the
        // resolved region power.
        assert_eq!(downlink_power(&rc_with_power(16), &[0x01, 0x02, 0x03, 0xf1]), 0);
    }
}

#[cfg(test)]
mod offset_tests {
    use super::*;
    use crate::lns::{cache_context, clear_context_cache, CACHE_TEST_LOCK};
    use std::sync::atomic::Ordering;

    fn setup() -> std::sync::MutexGuard<'static, ()> {
        let g = CACHE_TEST_LOCK.lock().unwrap();
        clear_context_cache();
        crate::lns::CONTEXT_CACHING_ENABLED.store(true, Ordering::Relaxed);
        g
    }

    fn ctx(tag: u8) -> Vec<u8> {
        vec![1, 2, 3, 0xf1, 0x0c, 0xab, 0x4e, 0x02, tag]
    }

    /// ChirpStack: verbatim echo, whole window in RxDelay.
    #[test]
    fn test_verbatim_echo_resolves_with_zero_offset() {
        let _g = setup();
        let uplink = 1_000_000_000i64;
        cache_context(uplink, ctx(0xaa));
        let r = resolve_context(uplink, 0, 5, "A");
        assert_eq!(r.context, ctx(0xaa));
        assert_eq!(r.offset_secs, 0);
        // RX1 delay stays 5s -- unchanged from pre-fix behaviour.
        assert_eq!(5 + r.offset_secs, 5);
    }

    /// TTN: xtime offset by +4s, RxDelay 1 -> the sum is the real 5s join window.
    #[test]
    fn test_offset_echo_resolves_and_restores_timing() {
        let _g = setup();
        let uplink = 2_000_000_000i64;
        cache_context(uplink, ctx(0xbb));
        let echoed = uplink + 4 * 1_000_000;
        let r = resolve_context(echoed, 0, 1, "A");
        assert_eq!(r.context, ctx(0xbb), "must find the uplink 4s back");
        assert_eq!(r.offset_secs, 4);
        // Without this the downlink would go out at 1s -- 4s before the device listens.
        assert_eq!(1 + r.offset_secs, 5, "reconstructed JOIN_ACCEPT_DELAY1");
    }

    #[test]
    fn test_miss_falls_back_to_count_us_and_zero_offset() {
        let _g = setup();
        let r = resolve_context(9_999_999_999, 0x0102_0304, 1, "A");
        assert_eq!(r.context, 0x0102_0304u32.to_be_bytes().to_vec());
        assert_eq!(r.offset_secs, 0, "fallback must not shift timing");
    }

    /// Two uplinks inside the probe window: refuse rather than misroute.
    #[test]
    fn test_ambiguous_match_is_refused() {
        let _g = setup();
        let a = 3_000_000_000i64;
        cache_context(a, ctx(0xcc));
        cache_context(a + 3 * 1_000_000, ctx(0xdd)); // 3s later
        // A downlink echoed 4s after `a` also sits 1s after the second uplink.
        let r = resolve_context(a + 4 * 1_000_000, 0x0102_0304, 1, "A");
        assert_eq!(
            r.context,
            0x0102_0304u32.to_be_bytes().to_vec(),
            "ambiguous match must fall back, never pick one arbitrarily"
        );
        assert_eq!(r.offset_secs, 0);
    }

    #[test]
    fn test_probe_window_bounded_by_rx_delay() {
        let _g = setup();
        let uplink = 4_000_000_000i64;
        cache_context(uplink, ctx(0xee));
        // offset 15 with RxDelay 1 would imply a 16s window -- beyond LoRaWAN's max.
        let r = resolve_context(uplink + 15 * 1_000_000, 0x0102_0304, 1, "A");
        assert_eq!(r.context, 0x0102_0304u32.to_be_bytes().to_vec());
    }
}
