# Plan: Address Code Review Findings (REVANS01)

## Context

An external code review (`docs/REVANS01.md`) identified 14 confirmed issues across
security, correctness, and code quality. All findings have been validated against the
source. This plan addresses 12 of them in 3 phases (CUPS signature verification and
Semtech UDP source pinning are deferred). The env var substitution will be a breaking
change: switching from `$VAR` to `${VAR}` syntax only.

---

## Phase 1: Bug Fixes (no breaking changes, no config changes)

### 1a. Replace blocking sleep in async context (#5)
- **File:** `src/backend/mod.rs`
  - Replace `use std::thread::sleep` → `use tokio::time::sleep`
  - Line ~57: change `sleep(Duration::from_secs(1))` → `sleep(Duration::from_secs(1)).await`

### 1b. Fix concentratord retry logic (#6) + remove blocking sleep
- **File:** `src/backend/concentratord.rs`
  - `send_command`: after successful reconnect, **re-send the command** instead of
    returning the original error. Add bounded retry (max 3 attempts).
  - Remove `use std::thread::sleep`. The retry is immediate after reconnect (no sleep
    between retries — the reconnect itself is the recovery step).

### 1c. Fix concentratord config command ignoring errors (#11)
- **File:** `src/backend/concentratord.rs`
  - `send_configuration_command`: replace `let _ = self.send_command(cmd)` with proper
    `match` — log error on failure, log success only on `Ok`.

### 1d. Guard TX_ACK index access (#7)
- **File:** `src/backend/semtech_udp/mod.rs`
  - Before line ~310 (`ack_items[downlink_cache.index]`), add bounds check:
    ```rust
    if downlink_cache.index >= ack_items.len() {
        return Err(anyhow!("TX_ACK: index {} out of bounds (len: {})",
            downlink_cache.index, ack_items.len()));
    }
    ```

### 1e. Add CUPS HTTP timeouts (#8)
- **File:** `src/cups/client.rs`
  - In `build_client`, add to the builder:
    `.timeout(Duration::from_secs(30))` and `.connect_timeout(Duration::from_secs(10))`

### 1f. Document timestamp wrapping (#10)
- **File:** `src/backend/semtech_udp/structs.rs`
  - Line ~612: change `timestamp += delay.as_micros() as u32` to
    `timestamp = timestamp.wrapping_add(delay.as_micros() as u32)` with a comment
    explaining that Semtech UDP timestamps are 32-bit µs counters that wrap naturally.

### Verification (Phase 1)
```sh
cargo build
cargo test
cargo clippy --no-deps
```

---

## Phase 2: Security Hardening (no breaking changes, no config changes)

### 2a. Fix TOML injection in OpenWrt shell script (#1)
- **File:** `openwrt/rak-basicstation/files/rak-basicstation.sh`
  - Add helper functions near the top:
    ```sh
    _toml_escape() {
        printf '%s' "$1" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g' | tr -d '\n\r'
    }
    _valid_key() {
        case "$1" in
            *[!a-zA-Z0-9_]*|"") return 1 ;;
            *) return 0 ;;
        esac
    }
    ```
  - Wrap all UCI-derived TOML string values with `$(_toml_escape "$var")`
  - Validate metadata keys with `_valid_key` before emitting

### 2b. Harden credential file permissions (#2)
- **File:** `src/cups/credentials.rs`
  - Replace `fs::write()` with `OpenOptions::new().create(true).write(true).truncate(true).mode(0o600).open()` + `write_all()`
  - Add `use std::os::unix::fs::OpenOptionsExt;`

### 2c. Add hex validation to gateway_id_to_id6 (#12)
- **File:** `src/lns/discovery.rs`
  - After length check, add: `if !gateway_id.chars().all(|c| c.is_ascii_hexdigit())`
  - Add test: `assert!(gateway_id_to_id6("ZZZZZZZZZZZZZZZZ").is_err())`

### 2d. Set explicit WebSocket message size limits (#13)
- **File:** `src/lns/websocket.rs`
  - Pass `WebSocketConfig { max_message_size: Some(1_048_576), max_frame_size: Some(262_144), ..Default::default() }` instead of `None`

### Verification (Phase 2)
```sh
cargo build
cargo test
cargo clippy --no-deps
```
Shell script: manually test with values containing `"`, `\`, and newlines.

---

## Phase 3: Config & Type Safety (breaking: env var syntax change)

### 3a. Type-safe backend selection (#14)
- **File:** `src/config.rs`
  - Add enum:
    ```rust
    #[derive(Clone, Serialize, Deserialize, Default)]
    #[serde(rename_all = "snake_case")]
    pub enum BackendType {
        #[default]
        Concentratord,
        SemtechUdp,
    }
    ```
  - Change `Backend.enabled: String` → `Backend.enabled: BackendType`
  - Add `impl fmt::Display for BackendType`
- **File:** `src/backend/mod.rs`
  - Change string match to enum match
- **File:** `src/cmd/configfile.rs`
  - Verify template renders correctly with the enum (serde serializes to same strings)

### 3b. Switch env var substitution to `${VAR}` only (#9)
- **File:** `src/config.rs`
  - Replace the `for (k, v) in env::vars()` loop with a pattern-based replacer:
    ```rust
    while let Some(start) = content.find("${") {
        let end = content[start..].find('}')
            .map(|i| start + i)
            .ok_or_else(|| anyhow!("Unclosed ${{}} in config"))?;
        let var_name = &content[start + 2..end];
        let value = env::var(var_name).unwrap_or_default();
        content = format!("{}{}{}", &content[..start], value, &content[end + 1..]);
    }
    ```
  - No new dependencies (hand-rolled, no regex crate needed)
- **File:** `src/cmd/configfile.rs`
  - Update template comments/examples to show `${VAR}` syntax
- **File:** `openwrt/rak-basicstation/files/rak-basicstation.sh`
  - Update any `$VAR` references in generated TOML to `${VAR}` if applicable
- **File:** `openwrt/rak-basicstation/files/rak-basicstation.config`
  - Update any default values using `$VAR` syntax

### Verification (Phase 3)
```sh
cargo build
cargo test
cargo clippy --no-deps
```
Test with a config file using `${VAR}` syntax and verify substitution works.
Test that bare `$VAR` is NOT substituted (breaking change, intentional).

---

## Phase 4: Documentation Updates

### 4a. Update README.md for env var syntax change
- **File:** `README.md`
  - Line 201: change `$VAR_NAME` → `${VAR_NAME}` syntax description
  - Docker section (lines 261-276): update the intro text to reference `${VAR}` syntax
  - Config examples (lines 141-168, 170-199): update any `$VAR` references if present

### 4b. Add security/operational guidance section to README.md
- **File:** `README.md`
  - Add a new **Security Considerations** section (before "License") covering:
    - **Credential file permissions**: CUPS-persisted credentials are written with
      mode 0600. Ensure the config directory is only readable by the service user.
    - **Semtech UDP trust model**: The UDP backend accepts packets from any local
      sender. It is designed for co-located processes on the same host. Do not
      expose the UDP bind port to untrusted networks.
    - **OpenWrt UCI input**: UCI values are escaped before TOML interpolation, but
      operators should avoid setting configuration via untrusted input sources.
    - **TLS authentication**: Production deployments should use mutual TLS or
      token-based auth. Plain WebSocket (`ws://`) should only be used for
      development.

### 4c. Document LNS discovery behavior accurately
- **File:** `README.md`
  - Clarify that `lns.server` is used as the initial endpoint for router discovery
    (`/router-info`). The actual WebSocket connection target (MUXS URI) is returned
    by the discovery response. If `lns.discovery_endpoint` is set, it overrides the
    discovery URL.

### Verification (Phase 4)
Review README.md for accuracy against the actual code behavior.

---

## Deferred Items

| Issue | Reason |
|-------|--------|
| #3 Semtech UDP source pinning | Limited attack surface (localhost). Document trust assumption instead. |
| #4 CUPS signature verification | Needs crypto deps, spec research, real CUPS server testing. Separate PR. |
| #9 (review) Global state refactor | High effort, low urgency. Works correctly as-is. |

---

## Files Modified (complete list)

| File | Phases |
|------|--------|
| `src/backend/mod.rs` | 1a, 3a |
| `src/backend/concentratord.rs` | 1b, 1c |
| `src/backend/semtech_udp/mod.rs` | 1d |
| `src/backend/semtech_udp/structs.rs` | 1f |
| `src/cups/client.rs` | 1e |
| `src/cups/credentials.rs` | 2b |
| `src/lns/discovery.rs` | 2c |
| `src/lns/websocket.rs` | 2d |
| `src/config.rs` | 3a, 3b |
| `src/cmd/configfile.rs` | 3a, 3b |
| `openwrt/rak-basicstation/files/rak-basicstation.sh` | 2a, 3b |
| `openwrt/rak-basicstation/files/rak-basicstation.config` | 3b |
| `README.md` | 4a, 4b, 4c |
