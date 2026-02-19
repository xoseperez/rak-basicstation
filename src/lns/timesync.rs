use std::sync::{LazyLock, RwLock};

use anyhow::Result;
use log::debug;

use super::messages::TimesyncRequest;

/// GPS time offset: the difference between our xtime-derived time and the
/// LNS-provided gpstime, in microseconds.
static GPS_TIME_OFFSET: LazyLock<RwLock<Option<i64>>> = LazyLock::new(|| RwLock::new(None));

/// Update the GPS time offset from a timesync response.
pub fn update_gps_offset(xtime: i64, gpstime: i64) {
    let offset = gpstime - xtime;
    let mut off = GPS_TIME_OFFSET.write().unwrap();
    *off = Some(offset);
    debug!(
        "Updated GPS time offset, xtime: {}, gpstime: {}, offset: {}",
        xtime, gpstime, offset
    );
}

/// Get the current GPS time offset, if calibrated.
pub fn get_gps_offset() -> Option<i64> {
    let off = GPS_TIME_OFFSET.read().unwrap();
    *off
}

/// Build a timesync request message.
pub fn build_timesync_request(txtime: i64) -> Result<String> {
    let msg = TimesyncRequest {
        msgtype: "timesync".to_string(),
        txtime,
    };
    Ok(serde_json::to_string(&msg)?)
}
