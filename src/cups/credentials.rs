use std::fs;
use std::io::Write;
use std::path::Path;

use anyhow::Result;
use crc32fast::Hasher;
use log::debug;

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

/// Compute CRC32 of the concatenation of the credential files.
/// Files that are empty or don't exist contribute nothing.
pub fn compute_cred_crc(ca_cert: &str, tls_cert: &str, tls_key: &str) -> Result<u32> {
    let mut hasher = Hasher::new();

    for path in &[ca_cert, tls_cert, tls_key] {
        if !path.is_empty()
            && let Ok(data) = fs::read(path) {
                hasher.update(&data);
            }
    }

    Ok(hasher.finalize())
}

/// Compute CRC32 for each signature key file.
pub fn compute_sig_key_crcs(sig_keys: &[String]) -> Result<Vec<u32>> {
    let mut crcs = Vec::new();

    for path in sig_keys {
        if path.is_empty() {
            continue;
        }
        let data = fs::read(path)?;
        let mut hasher = Hasher::new();
        hasher.update(&data);
        crcs.push(hasher.finalize());
    }

    Ok(crcs)
}

/// Compute the CRC32 of a single credential file (e.g. a CUPS-saved blob).
pub fn compute_cred_crc_from_file(path: &str) -> Result<u32> {
    if path.is_empty() {
        return Ok(0);
    }
    let mut hasher = Hasher::new();
    if let Ok(data) = fs::read(path) {
        hasher.update(&data);
    }
    Ok(hasher.finalize())
}

/// Parse the authorization token from a CUPS credential blob.
///
/// The blob layout is (no length prefixes between fields — DER self-delimiting):
///   [DER SEQUENCE: trust/CA cert]
///   [DER SEQUENCE: client cert]  — OR —  [0x00 0x00 0x00 0x00]  (token mode sentinel)
///   [DER SEQUENCE: private key]  — OR —  raw "Name: Value\r\n" bytes (token mode)
///
/// Returns `(header_name, header_value)` when the blob is in token mode,
/// e.g. `("Authorization", "Bearer NNSXS0...")`.
pub fn parse_token_from_cred(data: &[u8]) -> Option<(String, String)> {
    if data.len() < 2 || data[0] != 0x30 {
        return None; // Does not start with a DER SEQUENCE tag.
    }

    // Skip trust field (DER SEQUENCE).
    let trust_len = asn1_seq_total_len(data)?;
    let co = trust_len;

    if co + 4 > data.len() {
        return None;
    }

    // Token mode sentinel: cert field is exactly 4 zero bytes.
    if data[co] != 0x00 {
        return None; // X.509 mode — cert is a DER SEQUENCE, not a token.
    }

    let ko = co + 4; // token starts right after the 4-byte sentinel
    if ko >= data.len() {
        return None;
    }

    // Token bytes are the raw HTTP header line "Name: Value\r\n".
    let header_line = std::str::from_utf8(&data[ko..]).ok()?.trim();
    let (name, value) = header_line.split_once(": ")?;
    Some((name.trim().to_string(), value.trim().to_string()))
}

/// Returns the total byte length (tag + length octets + content) of the DER
/// SEQUENCE starting at `data[0]`.  Handles short-form and the 2-byte
/// long-form (0x82) used by BasicStation.
fn asn1_seq_total_len(data: &[u8]) -> Option<usize> {
    if data.len() < 2 || data[0] != 0x30 {
        return None;
    }
    if data[1] & 0x80 == 0 {
        // Short form: single length byte.
        Some(2 + data[1] as usize)
    } else {
        // Long form: only the 2-byte case (0x82) is supported by BasicStation.
        let num_len_bytes = (data[1] & 0x7f) as usize;
        if num_len_bytes != 2 || data.len() < 4 {
            return None;
        }
        let content_len = ((data[2] as usize) << 8) | (data[3] as usize);
        Some(4 + content_len)
    }
}

/// Save a plain-text value (e.g. a URI) to the credentials directory.
pub fn save_uri(credentials_dir: &str, filename: &str, value: &str) -> Result<()> {
    let dir = Path::new(credentials_dir);
    if !dir.exists() {
        fs::create_dir_all(dir)?;
    }
    let path = dir.join(filename);
    debug!("Saving URI to {:?}", path);
    _write_restricted(&path, value.as_bytes())?;
    Ok(())
}

/// Save credential blob to the credentials directory.
pub fn save_credentials(
    credentials_dir: &str,
    prefix: &str,
    data: &Option<Vec<u8>>,
) -> Result<()> {
    let data = match data {
        Some(d) => d,
        None => return Ok(()),
    };

    let dir = Path::new(credentials_dir);
    if !dir.exists() {
        fs::create_dir_all(dir)?;
    }

    let path = dir.join(format!("{}.cred", prefix));
    debug!("Saving credentials to {:?}", path);
    _write_restricted(&path, data)?;

    Ok(())
}

/// Write data to a file with restricted permissions (0o600 on Unix).
fn _write_restricted(path: &Path, data: &[u8]) -> Result<()> {
    let mut opts = fs::OpenOptions::new();
    opts.create(true).write(true).truncate(true);
    #[cfg(unix)]
    opts.mode(0o600);
    let mut f = opts.open(path)?;
    f.write_all(data)?;
    Ok(())
}
