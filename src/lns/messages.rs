use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

/// Deserialize a Vec field that may be null in JSON as an empty Vec.
fn null_as_empty_vec<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<Vec<T>>::deserialize(deserializer).map(|v| v.unwrap_or_default())
}

// -- Downstream messages (LNS → Gateway) --

/// Router discovery response.
#[derive(Debug, Deserialize)]
pub struct RouterInfoResponse {
    pub router: Option<String>,
    pub muxs: Option<String>,
    pub uri: Option<String>,
    pub error: Option<String>,
}

/// router_config message from LNS.
#[derive(Debug, Clone, Deserialize)]
pub struct RouterConfig {
    pub msgtype: String,

    /// Network ID filter array.
    #[serde(rename = "NetID", default, deserialize_with = "null_as_empty_vec")]
    pub net_id: Vec<u32>,

    /// JoinEUI filter ranges: [[start, end], ...].
    #[serde(rename = "JoinEui", default, deserialize_with = "null_as_empty_vec")]
    pub join_eui: Vec<Vec<u64>>,

    /// Region string (e.g., "EU868").
    #[serde(default)]
    pub region: String,

    /// Maximum EIRP in dBm.
    #[serde(default)]
    pub max_eirp: f64,

    /// Hardware spec string (e.g., "sx1301/1").
    #[serde(default)]
    pub hwspec: String,

    /// Frequency range [min_hz, max_hz].
    #[serde(default, deserialize_with = "null_as_empty_vec")]
    pub freq_range: Vec<u32>,

    /// Data rate definitions: [[SF, BW, DNONLY], ...].
    /// SF: 7-12 for LoRa, 0 for FSK.
    /// BW: 125, 250, 500 kHz (0 for FSK).
    /// DNONLY: 1 if downlink-only.
    #[serde(rename = "DRs", default, deserialize_with = "null_as_empty_vec")]
    pub drs: Vec<Vec<i32>>,

    /// SX1301/SX1302 concentrator configurations.
    #[serde(rename = "sx1301_conf", default, deserialize_with = "null_as_empty_vec")]
    pub sx1301_conf: Vec<Sx1301Conf>,

    /// Beacon configuration.
    #[serde(default)]
    pub bcning: Option<BeaconConfig>,

    /// Disable clear-channel assessment (debug).
    #[serde(default)]
    pub nocca: bool,

    /// Disable duty cycle (debug).
    #[serde(default)]
    pub nodc: bool,

    /// Disable dwell time (debug).
    #[serde(default)]
    pub nodwell: bool,

    /// MuxTime for round-trip monitoring.
    #[serde(rename = "MuxTime", default)]
    pub mux_time: Option<f64>,
}

/// SX1301/SX1302 concentrator configuration from router_config.
#[derive(Debug, Clone, Deserialize)]
pub struct Sx1301Conf {
    pub radio_0: Option<RadioConf>,
    pub radio_1: Option<RadioConf>,

    #[serde(rename = "chan_multiSF_0")]
    pub chan_multi_sf_0: Option<ChannelConf>,
    #[serde(rename = "chan_multiSF_1")]
    pub chan_multi_sf_1: Option<ChannelConf>,
    #[serde(rename = "chan_multiSF_2")]
    pub chan_multi_sf_2: Option<ChannelConf>,
    #[serde(rename = "chan_multiSF_3")]
    pub chan_multi_sf_3: Option<ChannelConf>,
    #[serde(rename = "chan_multiSF_4")]
    pub chan_multi_sf_4: Option<ChannelConf>,
    #[serde(rename = "chan_multiSF_5")]
    pub chan_multi_sf_5: Option<ChannelConf>,
    #[serde(rename = "chan_multiSF_6")]
    pub chan_multi_sf_6: Option<ChannelConf>,
    #[serde(rename = "chan_multiSF_7")]
    pub chan_multi_sf_7: Option<ChannelConf>,

    #[serde(rename = "chan_Lora_std")]
    pub chan_lora_std: Option<LoraStdChannelConf>,

    #[serde(rename = "chan_FSK")]
    pub chan_fsk: Option<FskChannelConf>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RadioConf {
    pub enable: Option<bool>,
    pub freq: Option<u32>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChannelConf {
    pub enable: Option<bool>,
    pub radio: Option<u8>,
    #[serde(rename = "if")]
    pub if_freq: Option<i32>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LoraStdChannelConf {
    pub enable: Option<bool>,
    pub radio: Option<u8>,
    #[serde(rename = "if")]
    pub if_freq: Option<i32>,
    pub bandwidth: Option<u32>,
    pub spread_factor: Option<u32>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FskChannelConf {
    pub enable: Option<bool>,
    pub radio: Option<u8>,
    #[serde(rename = "if")]
    pub if_freq: Option<i32>,
    pub bandwidth: Option<u32>,
    pub datarate: Option<u32>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BeaconConfig {
    #[serde(rename = "DR")]
    pub dr: Option<u32>,
    pub layout: Option<Vec<u32>>,
    pub freqs: Option<Vec<u32>>,
}

/// Downlink message from LNS.
#[derive(Debug, Deserialize)]
pub struct DownlinkMessage {
    pub msgtype: String,

    /// Device EUI.
    #[serde(rename = "DevEui")]
    pub dev_eui: Option<String>,

    /// Downlink class: 0=A, 1=B, 2=C.
    #[serde(rename = "dC")]
    pub dc: Option<u8>,

    /// Device interaction ID.
    pub diid: Option<i64>,

    /// PHY payload (hex encoded).
    pub pdu: Option<String>,

    /// RX delay (Class A).
    #[serde(rename = "RxDelay")]
    pub rx_delay: Option<u8>,

    /// RX1 data rate index.
    #[serde(rename = "RX1DR")]
    pub rx1_dr: Option<i32>,

    /// RX1 frequency in Hz.
    #[serde(rename = "RX1Freq")]
    pub rx1_freq: Option<u32>,

    /// RX2 data rate index.
    #[serde(rename = "RX2DR")]
    pub rx2_dr: Option<i32>,

    /// RX2 frequency in Hz.
    #[serde(rename = "RX2Freq")]
    pub rx2_freq: Option<u32>,

    /// Data rate (Class B/C).
    #[serde(rename = "DR")]
    pub dr: Option<i32>,

    /// Frequency in Hz (Class B/C).
    #[serde(rename = "Freq")]
    pub freq: Option<u32>,

    /// Priority (0-255).
    pub priority: Option<u8>,

    /// Extended time from original uplink.
    pub xtime: Option<i64>,

    /// Radio context from original uplink.
    pub rctx: Option<i64>,

    /// GPS time in microseconds.
    pub gpstime: Option<i64>,

    /// MuxTime for round-trip monitoring.
    #[serde(rename = "MuxTime")]
    pub mux_time: Option<f64>,
}

/// Scheduled downlink message from LNS.
#[derive(Debug, Deserialize)]
pub struct DownlinkSchedule {
    pub msgtype: String,
    pub schedule: Vec<ScheduleEntry>,

    #[serde(rename = "MuxTime")]
    pub mux_time: Option<f64>,
}

#[derive(Debug, Deserialize)]
pub struct ScheduleEntry {
    pub pdu: Option<String>,

    #[serde(rename = "DR")]
    pub dr: Option<i32>,

    #[serde(rename = "Freq")]
    pub freq: Option<u32>,

    pub priority: Option<u8>,
    pub gpstime: Option<i64>,
    pub rctx: Option<i64>,
}

/// Timesync message from LNS.
#[derive(Debug, Deserialize)]
pub struct TimesyncResponse {
    pub msgtype: String,
    pub txtime: Option<i64>,
    pub gpstime: Option<i64>,
    pub xtime: Option<i64>,

    #[serde(rename = "MuxTime")]
    pub mux_time: Option<f64>,
}

// -- Upstream messages (Gateway → LNS) --

/// Version message sent on connection.
#[derive(Debug, Serialize)]
pub struct VersionMessage {
    pub msgtype: String,
    pub station: String,
    pub firmware: String,
    pub package: String,
    pub model: String,
    pub protocol: u32,
    pub features: String,
}

/// Uplink info attached to all uplink messages.
#[derive(Debug, Serialize)]
pub struct UpInfo {
    pub rctx: i64,
    pub xtime: i64,
    pub gpstime: i64,
    pub rssi: f64,
    pub snr: f64,
}

/// Join request uplink.
#[derive(Debug, Serialize)]
pub struct JoinRequestMessage {
    pub msgtype: String,
    #[serde(rename = "MHdr")]
    pub mhdr: u8,
    #[serde(rename = "JoinEui")]
    pub join_eui: String,
    #[serde(rename = "DevEui")]
    pub dev_eui: String,
    #[serde(rename = "DevNonce")]
    pub dev_nonce: u16,
    #[serde(rename = "MIC")]
    pub mic: i32,
    #[serde(rename = "DR")]
    pub dr: i32,
    #[serde(rename = "Freq")]
    pub freq: u32,
    pub upinfo: UpInfo,
    #[serde(rename = "RefTime", skip_serializing_if = "Option::is_none")]
    pub ref_time: Option<f64>,
}

/// Data frame uplink.
#[derive(Debug, Serialize)]
pub struct DataFrameMessage {
    pub msgtype: String,
    #[serde(rename = "MHdr")]
    pub mhdr: u8,
    #[serde(rename = "DevAddr")]
    pub dev_addr: i32,
    #[serde(rename = "FCtrl")]
    pub fctrl: u8,
    #[serde(rename = "FCnt")]
    pub fcnt: u16,
    #[serde(rename = "FOpts")]
    pub fopts: String,
    #[serde(rename = "FPort")]
    pub fport: i32,
    #[serde(rename = "FRMPayload")]
    pub frm_payload: String,
    #[serde(rename = "MIC")]
    pub mic: i32,
    #[serde(rename = "DR")]
    pub dr: i32,
    #[serde(rename = "Freq")]
    pub freq: u32,
    pub upinfo: UpInfo,
    #[serde(rename = "RefTime", skip_serializing_if = "Option::is_none")]
    pub ref_time: Option<f64>,
}

/// Proprietary frame uplink.
#[derive(Debug, Serialize)]
pub struct PropDataFrameMessage {
    pub msgtype: String,
    #[serde(rename = "FRMPayload")]
    pub frm_payload: String,
    #[serde(rename = "DR")]
    pub dr: i32,
    #[serde(rename = "Freq")]
    pub freq: u32,
    pub upinfo: UpInfo,
    #[serde(rename = "RefTime", skip_serializing_if = "Option::is_none")]
    pub ref_time: Option<f64>,
}

/// Downlink transmitted confirmation.
#[derive(Debug, Serialize)]
pub struct DnTxedMessage {
    pub msgtype: String,
    pub diid: i64,
    #[serde(rename = "DevEui")]
    pub dev_eui: String,
    pub rctx: i64,
    pub xtime: i64,
    pub txtime: f64,
    pub gpstime: i64,
}

/// Timesync request (upstream).
#[derive(Debug, Serialize)]
pub struct TimesyncRequest {
    pub msgtype: String,
    pub txtime: i64,
}

/// Generic message wrapper for detecting msgtype.
#[derive(Debug, Deserialize)]
pub struct GenericMessage {
    pub msgtype: String,

    #[serde(flatten)]
    pub extra: std::collections::HashMap<String, Value>,
}
