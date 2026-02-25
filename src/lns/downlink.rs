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
    let context = super::get_cached_context(xtime)
        .unwrap_or_else(|| count_us.to_be_bytes().to_vec());

    debug!(
        "Class A downlink, xtime: {}, count_us: {}, rx_delay: {}, rctx: {:?}",
        xtime, count_us, rx_delay, msg.rctx
    );

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
                            seconds: rx_delay as i64,
                            nanos: 0,
                        }),
                    })),
                },
                context.clone(),
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
                            seconds: (rx_delay + 1) as i64,
                            nanos: 0,
                        }),
                    })),
                },
                context.clone(),
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
        let context = super::get_cached_context(xtime)
            .unwrap_or_else(|| count_us.to_be_bytes().to_vec());
        let rx_delay = msg.rx_delay.unwrap_or(1) as u32;

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
                                seconds: rx_delay as i64,
                                nanos: 0,
                            }),
                        })),
                    },
                    context.clone(),
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
                                seconds: (rx_delay + 1) as i64,
                                nanos: 0,
                            }),
                        })),
                    },
                    context.clone(),
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

fn build_downlink_item(
    phy_payload: &[u8],
    frequency: u32,
    sf: u32,
    bw_hz: u32,
    timing: gw::Timing,
    context: Vec<u8>,
) -> gw::DownlinkFrameItem {
    gw::DownlinkFrameItem {
        phy_payload: phy_payload.to_vec(),
        tx_info: Some(gw::DownlinkTxInfo {
            frequency,
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
            timing: Some(timing),
            context,
            ..Default::default()
        }),
        ..Default::default()
    }
}
