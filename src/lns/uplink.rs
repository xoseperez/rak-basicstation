use anyhow::Result;
use chirpstack_api::gw;
use log::debug;

use super::messages::{
    DataFrameMessage, JoinRequestMessage, PropDataFrameMessage, UpInfo,
};
use super::router_config::RouterConfigState;

/// Convert a Concentratord UplinkFrame to a BasicStation JSON message string.
pub fn frame_to_json(
    frame: &gw::UplinkFrame,
    rc: &RouterConfigState,
    session: u8,
    ref_time: Option<f64>,
) -> Result<String> {
    let phy = &frame.phy_payload;
    if phy.is_empty() {
        return Err(anyhow!("Empty PHY payload"));
    }

    let mhdr = phy[0];
    let mtype = (mhdr >> 5) & 0x07;

    let tx_info = frame
        .tx_info
        .as_ref()
        .ok_or_else(|| anyhow!("Missing tx_info"))?;
    let rx_info = frame
        .rx_info
        .as_ref()
        .ok_or_else(|| anyhow!("Missing rx_info"))?;

    let freq = tx_info.frequency;
    let dr = modulation_to_dr(tx_info, rc)?;
    let upinfo = build_upinfo(rx_info, session)?;

    match mtype {
        // Join Request (MType 0b000)
        0b000 => {
            if phy.len() < 23 {
                return Err(anyhow!(
                    "Join request PHY payload too short: {} bytes",
                    phy.len()
                ));
            }
            let join_eui = eui_from_le_bytes(&phy[1..9]);
            let dev_eui = eui_from_le_bytes(&phy[9..17]);
            let dev_nonce = u16::from_le_bytes([phy[17], phy[18]]);
            let mic = i32::from_le_bytes([phy[19], phy[20], phy[21], phy[22]]);

            let msg = JoinRequestMessage {
                msgtype: "jreq".to_string(),
                mhdr,
                join_eui,
                dev_eui,
                dev_nonce,
                mic,
                dr,
                freq,
                upinfo,
                ref_time,
            };

            debug!("Sending jreq uplink, DR: {}, Freq: {}", dr, freq);
            Ok(serde_json::to_string(&msg)?)
        }

        // Unconfirmed Data Up (0b010) or Confirmed Data Up (0b100)
        0b010 | 0b100 => {
            if phy.len() < 12 {
                return Err(anyhow!(
                    "Data frame PHY payload too short: {} bytes",
                    phy.len()
                ));
            }
            let dev_addr = i32::from_le_bytes([phy[1], phy[2], phy[3], phy[4]]);
            let fctrl = phy[5];
            let fopts_len = (fctrl & 0x0F) as usize;
            let fcnt = u16::from_le_bytes([phy[6], phy[7]]);

            // FOpts starts at byte 8.
            let fopts = if fopts_len > 0 && phy.len() >= 8 + fopts_len {
                hex::encode(&phy[8..8 + fopts_len])
            } else {
                String::new()
            };

            let fhdr_len = 8 + fopts_len;
            let mic_start = phy.len() - 4;

            // FPort and FRMPayload.
            let (fport, frm_payload) = if fhdr_len < mic_start {
                let fport = phy[fhdr_len] as i32;
                let payload = if fhdr_len + 1 < mic_start {
                    hex::encode(&phy[fhdr_len + 1..mic_start])
                } else {
                    String::new()
                };
                (fport, payload)
            } else {
                (-1, String::new())
            };

            let mic = i32::from_le_bytes([
                phy[mic_start],
                phy[mic_start + 1],
                phy[mic_start + 2],
                phy[mic_start + 3],
            ]);

            let msg = DataFrameMessage {
                msgtype: "updf".to_string(),
                mhdr,
                dev_addr,
                fctrl,
                fcnt,
                fopts,
                fport,
                frm_payload,
                mic,
                dr,
                freq,
                upinfo,
                ref_time,
            };

            debug!(
                "Sending updf uplink, DevAddr: {:08x}, DR: {}, Freq: {}",
                dev_addr, dr, freq
            );
            Ok(serde_json::to_string(&msg)?)
        }

        // Proprietary (MType 0b111)
        0b111 => {
            let frm_payload = if phy.len() > 1 {
                hex::encode(&phy[1..])
            } else {
                String::new()
            };

            let msg = PropDataFrameMessage {
                msgtype: "propdf".to_string(),
                frm_payload,
                dr,
                freq,
                upinfo,
                ref_time,
            };

            debug!("Sending propdf uplink, DR: {}, Freq: {}", dr, freq);
            Ok(serde_json::to_string(&msg)?)
        }

        _ => {
            debug!("Unsupported MType: {}, dropping frame", mtype);
            Err(anyhow!("Unsupported MType: {}", mtype))
        }
    }
}

/// Extract the data rate index from UplinkTxInfo modulation.
fn modulation_to_dr(tx_info: &gw::UplinkTxInfo, rc: &RouterConfigState) -> Result<i32> {
    let modulation = tx_info
        .modulation
        .as_ref()
        .ok_or_else(|| anyhow!("Missing modulation info"))?;

    match &modulation.parameters {
        Some(gw::modulation::Parameters::Lora(lora)) => {
            let sf = lora.spreading_factor;
            let bw_khz = lora.bandwidth / 1000;
            let dr = rc.sf_bw_to_dr(sf, bw_khz);
            if dr < 0 {
                return Err(anyhow!("No DR mapping for SF{} BW{}kHz", sf, bw_khz));
            }
            Ok(dr)
        }
        Some(gw::modulation::Parameters::Fsk(_fsk)) => {
            let dr = rc.fsk_to_dr();
            if dr < 0 {
                return Err(anyhow!("No DR mapping for FSK"));
            }
            Ok(dr)
        }
        _ => Err(anyhow!("Unknown modulation type")),
    }
}

/// Build an UpInfo struct from UplinkRxInfo.
fn build_upinfo(rx_info: &gw::UplinkRxInfo, session: u8) -> Result<UpInfo> {
    // Construct xtime from concentrator context.
    // context is a 4-byte big-endian count_us value.
    let count_us = if rx_info.context.len() >= 4 {
        u32::from_be_bytes([
            rx_info.context[0],
            rx_info.context[1],
            rx_info.context[2],
            rx_info.context[3],
        ]) as i64
    } else {
        0
    };

    // xtime format: bits[62:56]=radio_unit(0), bits[55:48]=session, bits[47:0]=count_us
    let xtime = ((session as i64) << 48) | (count_us & 0x0000_FFFF_FFFF_FFFF);

    // rctx: opaque context from concentratord, passed back as-is in downlinks.
    let rctx = if rx_info.context.len() >= 4 {
        u32::from_be_bytes([
            rx_info.context[0],
            rx_info.context[1],
            rx_info.context[2],
            rx_info.context[3],
        ]) as i64
    } else {
        0
    };

    // GPS time in microseconds since GPS epoch.
    let gpstime = rx_info
        .time_since_gps_epoch
        .as_ref()
        .map(|d| {
            let secs = d.seconds;
            let nanos = d.nanos as i64;
            secs * 1_000_000 + nanos / 1_000
        })
        .unwrap_or(0);

    Ok(UpInfo {
        rctx,
        xtime,
        gpstime,
        rssi: rx_info.rssi as f64,
        snr: rx_info.snr as f64,
    })
}

/// Convert a little-endian byte slice to a BasicStation EUI string (big-endian,
/// dash-separated uppercase hex: "XX-XX-XX-XX-XX-XX-XX-XX").
fn eui_from_le_bytes(bytes: &[u8]) -> String {
    bytes
        .iter()
        .rev()
        .map(|b| format!("{:02X}", b))
        .collect::<Vec<_>>()
        .join("-")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_eui_from_le_bytes() {
        let bytes = [0x35, 0xA2, 0x10, 0xFF, 0x01, 0xC0, 0x16, 0x00];
        assert_eq!(eui_from_le_bytes(&bytes), "00-16-C0-01-FF-10-A2-35");
    }
}
