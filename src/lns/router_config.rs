use anyhow::Result;
use chirpstack_api::gw;
use log::{debug, info, warn};

use super::messages::RouterConfig;

/// Default TX power (dBm) when the region is unknown.
/// Matches the reference station's builtin default (s2e.c: `txpow = 14 * TXPOW_SCALE`).
const REGION_DEFAULT_DBM: i32 = 14;

/// Lowest TX power (dBm) a ChirpStack Gateway Mesh relay will accept.
/// `tx_power_to_index()` picks the highest configured entry <= the requested power and
/// errors when none matches, so anything below the table floor is rejected outright.
const MIN_TX_POWER_DBM: i32 = 12;

/// Region default TX power (dBm), mirroring the reference station's region table.
/// `EU863` is the obsolete alias ChirpStack still sends for `EU868`.
fn region_default_dbm(region: &str) -> i32 {
    match region.to_ascii_uppercase().as_str() {
        "EU868" | "EU863" => 16,
        _ => REGION_DEFAULT_DBM,
    }
}

/// Resolve the downlink TX power from the router_config.
///
/// The LNS-supplied `max_eirp` only ever *lowers* the region default (s2e.c applies it as a
/// cap), and is absent entirely on some LNS -- ChirpStack omits the field, which
/// `#[serde(default)]` renders as `0.0`. The result is clamped to the mesh relay's floor,
/// since a value below it would be rejected exactly like the legacy hardcoded `0`.
fn resolve_tx_power_dbm(region: &str, max_eirp: f64) -> i32 {
    let region_default = region_default_dbm(region);
    let capped = if max_eirp > 0.0 {
        region_default.min(max_eirp.round() as i32)
    } else {
        region_default
    };

    if capped < MIN_TX_POWER_DBM {
        // Deliberately NOT configurable: the floor belongs to the mesh relay's region
        // table, so a second copy here would silently drift out of sync. Raising the
        // power does mean exceeding an LNS-advertised limit, so say so loudly rather
        // than let it pass unnoticed.
        warn!(
            "Derived TX power {} dBm is below the mesh relay minimum {} dBm; raising it. \
             The LNS advertised max_eirp {} -- verify this is legal for region {}.",
            capped, MIN_TX_POWER_DBM, max_eirp, region
        );
    }

    capped.max(MIN_TX_POWER_DBM)
}

/// Parsed and stored router_config state.
#[derive(Debug, Clone)]
pub struct RouterConfigState {
    /// Data rate table from router_config.
    /// Index is DR number, value is (SF, BW_kHz).
    /// SF=0 means FSK. BW=0 means FSK.
    pub drs: Vec<(u32, u32)>,

    /// Network ID filter.
    pub net_ids: Vec<u32>,

    /// JoinEUI filter ranges.
    pub join_eui_ranges: Vec<(u64, u64)>,

    /// Frequency range (min, max) in Hz.
    pub freq_range: (u32, u32),

    /// Region string.
    pub region: String,

    /// Downlink TX power in dBm, derived from the region and `max_eirp`.
    pub tx_power_dbm: i32,
}

impl RouterConfigState {
    /// Create from a received router_config message.
    pub fn from_router_config(rc: &RouterConfig) -> Self {
        let drs: Vec<(u32, u32)> = rc
            .drs
            .iter()
            .map(|entry| {
                let sf = entry.first().copied().unwrap_or(0) as u32;
                let bw = entry.get(1).copied().unwrap_or(0) as u32;
                (sf, bw)
            })
            .collect();

        let join_eui_ranges: Vec<(u64, u64)> = rc
            .join_eui
            .iter()
            .filter(|range| range.len() == 2)
            .map(|range| (range[0], range[1]))
            .collect();

        let freq_range = if rc.freq_range.len() == 2 {
            (rc.freq_range[0], rc.freq_range[1])
        } else {
            (0, 0)
        };

        let tx_power_dbm = resolve_tx_power_dbm(&rc.region, rc.max_eirp);
        info!(
            "Resolved downlink TX power, region: {}, max_eirp: {}, tx_power: {} dBm",
            rc.region, rc.max_eirp, tx_power_dbm
        );

        RouterConfigState {
            drs,
            net_ids: rc.net_id.clone(),
            join_eui_ranges,
            freq_range,
            region: rc.region.clone(),
            tx_power_dbm,
        }
    }

    /// Look up DR index from SF and BW (kHz).
    /// Returns -1 if not found.
    pub fn sf_bw_to_dr(&self, sf: u32, bw_khz: u32) -> i32 {
        for (i, (dr_sf, dr_bw)) in self.drs.iter().enumerate() {
            if *dr_sf == sf && *dr_bw == bw_khz {
                return i as i32;
            }
        }
        -1
    }

    /// Look up DR index for FSK.
    /// Returns -1 if not found.
    pub fn fsk_to_dr(&self) -> i32 {
        for (i, (sf, _bw)) in self.drs.iter().enumerate() {
            if *sf == 0 {
                return i as i32;
            }
        }
        -1
    }

    /// Get SF and BW (Hz) for a DR index.
    pub fn dr_to_sf_bw(&self, dr: i32) -> Option<(u32, u32)> {
        if dr < 0 || dr as usize >= self.drs.len() {
            return None;
        }
        let (sf, bw_khz) = self.drs[dr as usize];
        Some((sf, bw_khz * 1000)) // Convert kHz to Hz
    }
}

/// Translate router_config sx1301_conf into a Concentratord GatewayConfiguration.
pub fn to_gateway_configuration(rc: &RouterConfig) -> Result<gw::GatewayConfiguration> {
    let mut channels = Vec::new();

    for (board_idx, board) in rc.sx1301_conf.iter().enumerate() {
        let radio_0_freq = board
            .radio_0
            .as_ref()
            .and_then(|r| r.freq)
            .unwrap_or(0);
        let radio_1_freq = board
            .radio_1
            .as_ref()
            .and_then(|r| r.freq)
            .unwrap_or(0);

        let get_radio_freq = |radio: u8| -> u32 {
            if radio == 0 {
                radio_0_freq
            } else {
                radio_1_freq
            }
        };

        // Multi-SF channels (8 per board).
        let multi_sf_chans = [
            &board.chan_multi_sf_0,
            &board.chan_multi_sf_1,
            &board.chan_multi_sf_2,
            &board.chan_multi_sf_3,
            &board.chan_multi_sf_4,
            &board.chan_multi_sf_5,
            &board.chan_multi_sf_6,
            &board.chan_multi_sf_7,
        ];

        for (ch_idx, chan) in multi_sf_chans.iter().enumerate() {
            if let Some(ch) = chan {
                if ch.enable != Some(true) {
                    continue;
                }
                let radio = ch.radio.unwrap_or(0);
                let if_freq = ch.if_freq.unwrap_or(0);
                let freq = (get_radio_freq(radio) as i64 + if_freq as i64) as u32;

                debug!(
                    "Board {}, multi-SF channel {}: freq={}",
                    board_idx, ch_idx, freq
                );

                channels.push(gw::ChannelConfiguration {
                    frequency: freq,
                    modulation_config: Some(
                        gw::channel_configuration::ModulationConfig::LoraModulationConfig(
                            gw::LoraModulationConfig {
                                bandwidth: 125000,
                                spreading_factors: vec![7, 8, 9, 10, 11, 12],
                                ..Default::default()
                            },
                        ),
                    ),
                    ..Default::default()
                });
            }
        }

        // Standard LoRa channel.
        if let Some(ch) = &board.chan_lora_std
            && ch.enable == Some(true) {
                let radio = ch.radio.unwrap_or(0);
                let if_freq = ch.if_freq.unwrap_or(0);
                let freq = (get_radio_freq(radio) as i64 + if_freq as i64) as u32;
                let bw = ch.bandwidth.unwrap_or(250000);
                let sf = ch.spread_factor.unwrap_or(7);

                debug!(
                    "Board {}, LoRa std channel: freq={}, bw={}, sf={}",
                    board_idx, freq, bw, sf
                );

                channels.push(gw::ChannelConfiguration {
                    frequency: freq,
                    modulation_config: Some(
                        gw::channel_configuration::ModulationConfig::LoraModulationConfig(
                            gw::LoraModulationConfig {
                                bandwidth: bw,
                                spreading_factors: vec![sf],
                                ..Default::default()
                            },
                        ),
                    ),
                    ..Default::default()
                });
            }

        // FSK channel.
        if let Some(ch) = &board.chan_fsk
            && ch.enable == Some(true) {
                let radio = ch.radio.unwrap_or(0);
                let if_freq = ch.if_freq.unwrap_or(0);
                let freq = (get_radio_freq(radio) as i64 + if_freq as i64) as u32;
                let bw = ch.bandwidth.unwrap_or(125000);
                let dr = ch.datarate.unwrap_or(50000);

                debug!(
                    "Board {}, FSK channel: freq={}, bw={}, datarate={}",
                    board_idx, freq, bw, dr
                );

                channels.push(gw::ChannelConfiguration {
                    frequency: freq,
                    modulation_config: Some(
                        gw::channel_configuration::ModulationConfig::FskModulationConfig(
                            gw::FskModulationConfig {
                                bandwidth: bw,
                                bitrate: dr,
                                ..Default::default()
                            },
                        ),
                    ),
                    ..Default::default()
                });
            }
    }

    info!(
        "Translated router_config to {} channel configurations",
        channels.len()
    );

    Ok(gw::GatewayConfiguration {
        gateway_id: String::new(),
        gateway_id_legacy: Vec::new(),
        version: "".to_string(),
        channels,
        stats_interval: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_region_default_eu868_and_obsolete_alias() {
        // ChirpStack reports the obsolete "EU863"; the reference station maps it to EU868.
        assert_eq!(region_default_dbm("EU868"), 16);
        assert_eq!(region_default_dbm("EU863"), 16);
        assert_eq!(region_default_dbm("eu863"), 16);
    }

    #[test]
    fn test_region_default_unknown_falls_back() {
        assert_eq!(region_default_dbm("US915"), REGION_DEFAULT_DBM);
        assert_eq!(region_default_dbm(""), REGION_DEFAULT_DBM);
    }

    #[test]
    fn test_absent_max_eirp_uses_region_default() {
        // ChirpStack omits max_eirp entirely -> serde default 0.0. Using it directly
        // would yield power 0, which the mesh relay rejects.
        assert_eq!(resolve_tx_power_dbm("EU863", 0.0), 16);
    }

    #[test]
    fn test_max_eirp_only_lowers() {
        // A cap below the region default lowers it...
        assert_eq!(resolve_tx_power_dbm("EU868", 14.0), 14);
        // ...but never raises it.
        assert_eq!(resolve_tx_power_dbm("EU868", 27.0), 16);
    }

    #[test]
    fn test_result_is_clamped_to_mesh_floor() {
        // Below the relay's table floor the downlink would be rejected exactly like the
        // legacy hardcoded 0, so clamp up.
        assert_eq!(resolve_tx_power_dbm("EU868", 5.0), MIN_TX_POWER_DBM);
        assert_eq!(resolve_tx_power_dbm("US915", 1.0), MIN_TX_POWER_DBM);
    }
}
