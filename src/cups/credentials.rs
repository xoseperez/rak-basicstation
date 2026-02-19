use std::fs;
use std::path::Path;

use anyhow::Result;
use crc32fast::Hasher;
use log::debug;

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
    fs::write(&path, data)?;

    Ok(())
}
