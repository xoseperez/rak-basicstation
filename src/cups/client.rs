use anyhow::Result;
use log::debug;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION};

use crate::config::Configuration;

/// Parsed CUPS update response.
#[derive(Debug, Default)]
pub struct CupsUpdateResponse {
    pub cups_uri: Option<String>,
    pub tc_uri: Option<String>,
    pub cups_cred: Option<Vec<u8>>,
    pub tc_cred: Option<Vec<u8>>,
    pub sig_key_crc: Option<u32>,
    pub signature: Option<Vec<u8>>,
    pub update_data: Option<Vec<u8>>,
}

/// Send CUPS update-info request.
#[allow(clippy::too_many_arguments)]
pub async fn post_update_info(
    cups_server: &str,
    router_id6: &str,
    cups_uri: &str,
    tc_uri: &str,
    cups_cred_crc: u32,
    tc_cred_crc: u32,
    sig_key_crcs: &[u32],
    conf: &Configuration,
) -> Result<Vec<u8>> {
    let url = format!("{}/update-info", cups_server.trim_end_matches('/'));
    debug!("CUPS POST to {}", url);

    let body = serde_json::json!({
        "router": router_id6,
        "cupsUri": cups_uri,
        "tcUri": tc_uri,
        "cupsCredCrc": cups_cred_crc,
        "tcCredCrc": tc_cred_crc,
        "station": "2.0.6(linux/std)",
        "model": std::env::consts::ARCH,
        "package": format!("rak-basicstation v{} ({} backend)", env!("CARGO_PKG_VERSION"), conf.backend.enabled),
        "keys": sig_key_crcs,
    });

    let client = build_client(conf)?;
    let resp = client
        .post(&url)
        .json(&body)
        .send()
        .await?
        .error_for_status()?;

    let bytes = resp.bytes().await?;
    Ok(bytes.to_vec())
}

/// Parse the binary CUPS response.
/// Format (little-endian):
///   1 byte:  cupsUriLen, N bytes: cupsUri
///   1 byte:  tcUriLen,   N bytes: tcUri
///   2 bytes: cupsCredLen, N bytes: cupsCred
///   2 bytes: tcCredLen,   N bytes: tcCred
///   4 bytes: sigLen (if > 0: 4 bytes keyCRC + (sigLen-4) bytes sig)
///   4 bytes: updLen, N bytes: updData
pub fn parse_response(data: &[u8]) -> Result<CupsUpdateResponse> {
    let mut resp = CupsUpdateResponse::default();
    let mut pos = 0;

    if data.len() < 14 {
        // Minimum: 1+1+2+2+4+4 = 14 bytes (all zero-length segments).
        return Err(anyhow!(
            "CUPS response too short: {} bytes (minimum 14)",
            data.len()
        ));
    }

    // CUPS URI (1-byte length prefix).
    let cups_uri_len = data[pos] as usize;
    pos += 1;
    if cups_uri_len > 0 {
        if pos + cups_uri_len > data.len() {
            return Err(anyhow!("CUPS response truncated at cupsUri"));
        }
        resp.cups_uri = Some(String::from_utf8_lossy(&data[pos..pos + cups_uri_len]).to_string());
        pos += cups_uri_len;
    }

    // TC URI (1-byte length prefix).
    let tc_uri_len = data[pos] as usize;
    pos += 1;
    if tc_uri_len > 0 {
        if pos + tc_uri_len > data.len() {
            return Err(anyhow!("CUPS response truncated at tcUri"));
        }
        resp.tc_uri = Some(String::from_utf8_lossy(&data[pos..pos + tc_uri_len]).to_string());
        pos += tc_uri_len;
    }

    // CUPS credentials (2-byte length prefix, little-endian).
    if pos + 2 > data.len() {
        return Err(anyhow!("CUPS response truncated at cupsCredLen"));
    }
    let cups_cred_len = u16::from_le_bytes([data[pos], data[pos + 1]]) as usize;
    pos += 2;
    if cups_cred_len > 0 {
        if pos + cups_cred_len > data.len() {
            return Err(anyhow!("CUPS response truncated at cupsCred"));
        }
        resp.cups_cred = Some(data[pos..pos + cups_cred_len].to_vec());
        pos += cups_cred_len;
    }

    // TC credentials (2-byte length prefix, little-endian).
    if pos + 2 > data.len() {
        return Err(anyhow!("CUPS response truncated at tcCredLen"));
    }
    let tc_cred_len = u16::from_le_bytes([data[pos], data[pos + 1]]) as usize;
    pos += 2;
    if tc_cred_len > 0 {
        if pos + tc_cred_len > data.len() {
            return Err(anyhow!("CUPS response truncated at tcCred"));
        }
        resp.tc_cred = Some(data[pos..pos + tc_cred_len].to_vec());
        pos += tc_cred_len;
    }

    // Signature (4-byte length prefix, includes keyCRC).
    if pos + 4 > data.len() {
        return Err(anyhow!("CUPS response truncated at sigLen"));
    }
    let sig_len = u32::from_le_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]])
        as usize;
    pos += 4;
    if sig_len > 0 {
        if sig_len < 4 || pos + sig_len > data.len() {
            return Err(anyhow!("CUPS response truncated at sig"));
        }
        let key_crc =
            u32::from_le_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]);
        resp.sig_key_crc = Some(key_crc);
        pos += 4;
        resp.signature = Some(data[pos..pos + sig_len - 4].to_vec());
        pos += sig_len - 4;
    }

    // Update data (4-byte length prefix).
    if pos + 4 > data.len() {
        return Err(anyhow!("CUPS response truncated at updLen"));
    }
    let upd_len = u32::from_le_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]])
        as usize;
    pos += 4;
    if upd_len > 0 {
        if pos + upd_len > data.len() {
            return Err(anyhow!("CUPS response truncated at updData"));
        }
        resp.update_data = Some(data[pos..pos + upd_len].to_vec());
    }

    Ok(resp)
}

fn build_client(conf: &Configuration) -> Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(30));

    if !conf.cups.ca_cert.is_empty() {
        let ca_data = std::fs::read(&conf.cups.ca_cert)?;
        let cert = reqwest::Certificate::from_pem(&ca_data)?;
        builder = builder.add_root_certificate(cert);
    }

    if !conf.cups.tls_cert.is_empty() && !conf.cups.tls_key.is_empty() {
        let cert_data = std::fs::read(&conf.cups.tls_cert)?;
        let key_data = std::fs::read(&conf.cups.tls_key)?;
        let mut identity_pem = cert_data;
        identity_pem.extend_from_slice(&key_data);
        let identity = reqwest::Identity::from_pem(&identity_pem)?;
        builder = builder.identity(identity);
    } else if conf.cups.tls_cert.is_empty() && !conf.cups.tls_key.is_empty() {
        let token = std::fs::read_to_string(&conf.cups.tls_key)?.trim().to_string();
        let mut headers = HeaderMap::new();
        let auth_value = HeaderValue::from_str(&format!("Bearer {}", token))?;
        headers.insert(AUTHORIZATION, auth_value);
        builder = builder.default_headers(headers);
    }

    Ok(builder.build()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_empty_response() {
        // 14 zero bytes = no updates.
        let data = vec![0u8; 14];
        let resp = parse_response(&data).unwrap();
        assert!(resp.cups_uri.is_none());
        assert!(resp.tc_uri.is_none());
        assert!(resp.cups_cred.is_none());
        assert!(resp.tc_cred.is_none());
        assert!(resp.signature.is_none());
        assert!(resp.update_data.is_none());
    }

    #[test]
    fn test_parse_response_with_uris() {
        let mut data = Vec::new();
        // CUPS URI: "https://a"
        let cups_uri = b"https://a";
        data.push(cups_uri.len() as u8);
        data.extend_from_slice(cups_uri);
        // TC URI: "wss://b"
        let tc_uri = b"wss://b";
        data.push(tc_uri.len() as u8);
        data.extend_from_slice(tc_uri);
        // No credentials (2+2 bytes).
        data.extend_from_slice(&[0, 0]);
        data.extend_from_slice(&[0, 0]);
        // No signature (4 bytes).
        data.extend_from_slice(&[0, 0, 0, 0]);
        // No update data (4 bytes).
        data.extend_from_slice(&[0, 0, 0, 0]);

        let resp = parse_response(&data).unwrap();
        assert_eq!(resp.cups_uri.as_deref(), Some("https://a"));
        assert_eq!(resp.tc_uri.as_deref(), Some("wss://b"));
    }
}
