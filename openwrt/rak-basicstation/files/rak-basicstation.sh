. /lib/functions.sh

CONF_DIR=/var/etc/rak-basicstation
CONF_FILE=$CONF_DIR/rak-basicstation.toml

configure() {
    local config_name="$1"
    mkdir -p "$CONF_DIR"
    config_load "$config_name"
    _write_cert_files
    _conf_logging
    _conf_backend
    _conf_lns
    _conf_cups
    _conf_metadata
}

# ── cert files ───────────────────────────────────────────────────────────────
# For each cert field: if UCI content is non-empty, write file (mode 0600 for
# keys, 0644 for certs); otherwise remove the file so the TOML path is omitted.

_write_cert_files() {
    local content
    # LNS
    config_get content lns ca_cert_content ""
    _write_or_remove "$content" "$CONF_DIR/lns_ca.crt"     0644
    config_get content lns tls_cert_content ""
    _write_or_remove "$content" "$CONF_DIR/lns_client.crt"  0644
    config_get content lns tls_key_content ""
    _write_or_remove "$content" "$CONF_DIR/lns_client.key"  0600
    # CUPS
    config_get content cups ca_cert_content ""
    _write_or_remove "$content" "$CONF_DIR/cups_ca.crt"     0644
    config_get content cups tls_cert_content ""
    _write_or_remove "$content" "$CONF_DIR/cups_client.crt" 0644
    config_get content cups tls_key_content ""
    _write_or_remove "$content" "$CONF_DIR/cups_client.key" 0600
}

_write_or_remove() {
    local content="$1" path="$2" mode="$3"
    if [ -n "$content" ]; then
        printf '%s' "$content" > "$path"
        chmod "$mode" "$path"
    else
        rm -f "$path"
    fi
}

# ── [logging] ────────────────────────────────────────────────────────────────
_conf_logging() {
    local level log_to_syslog
    config_get      level         logging level         "info"
    config_get_bool log_to_syslog logging log_to_syslog 0
    [ "$log_to_syslog" = "1" ] && log_to_syslog=true || log_to_syslog=false
    cat > "$CONF_FILE" <<-EOF
	[logging]
	  level="$level"
	  log_to_syslog=$log_to_syslog
	EOF
}

# ── [backend] ────────────────────────────────────────────────────────────────
# Reads the flat 'backend' UCI section; maps prefixed fields to TOML sub-tables.
# Booleans converted 0/1 → false/true. gateway_id omitted when empty.
_conf_backend() {
    local enabled gateway_id
    local forward_crc_ok forward_crc_invalid forward_crc_missing
    local event_url command_url context_caching
    local bind time_fallback

    config_get      enabled    backend enabled    "concentratord"
    config_get      gateway_id backend gateway_id ""

    config_get_bool forward_crc_ok      backend forward_crc_ok      1
    config_get_bool forward_crc_invalid backend forward_crc_invalid 0
    config_get_bool forward_crc_missing backend forward_crc_missing 0

    config_get      event_url       backend concentratord_event_url      "ipc:///tmp/concentratord_event"
    config_get      command_url     backend concentratord_command_url    "ipc:///tmp/concentratord_command"
    config_get_bool context_caching backend concentratord_context_caching 0

    config_get      bind          backend semtech_udp_bind          "0.0.0.0:1700"
    config_get_bool time_fallback backend semtech_udp_time_fallback 0

    # 0/1 → false/true for all booleans
    for _v in forward_crc_ok forward_crc_invalid forward_crc_missing context_caching time_fallback; do
        eval "[ \"\$$_v\" = 1 ] && $_v=true || $_v=false"
    done

    cat >> "$CONF_FILE" <<-EOF

	[backend]
	  enabled="$enabled"
	EOF
    [ -n "$gateway_id" ] && echo "  gateway_id=\"$gateway_id\"" >> "$CONF_FILE"

    cat >> "$CONF_FILE" <<-EOF

	  [backend.filters]
	    forward_crc_ok=$forward_crc_ok
	    forward_crc_invalid=$forward_crc_invalid
	    forward_crc_missing=$forward_crc_missing

	  [backend.concentratord]
	    event_url="$event_url"
	    command_url="$command_url"
	    context_caching=$context_caching

	  [backend.semtech_udp]
	    bind="$bind"
	    time_fallback_enabled=$time_fallback
	EOF
}

# ── [lns] ────────────────────────────────────────────────────────────────────
# Cert paths only emitted if the file was actually written.
_conf_lns() {
    local server discovery_endpoint reconnect_interval
    config_get server             lns server             "wss://localhost:8887"
    config_get discovery_endpoint lns discovery_endpoint ""
    config_get reconnect_interval lns reconnect_interval "5s"

    cat >> "$CONF_FILE" <<-EOF

	[lns]
	  server="$server"
	  reconnect_interval="$reconnect_interval"
	EOF
    [ -n "$discovery_endpoint" ] && \
        echo "  discovery_endpoint=\"$discovery_endpoint\"" >> "$CONF_FILE"
    [ -f "$CONF_DIR/lns_ca.crt"     ] && echo "  ca_cert=\"$CONF_DIR/lns_ca.crt\""      >> "$CONF_FILE"
    [ -f "$CONF_DIR/lns_client.crt" ] && echo "  tls_cert=\"$CONF_DIR/lns_client.crt\""  >> "$CONF_FILE"
    [ -f "$CONF_DIR/lns_client.key" ] && echo "  tls_key=\"$CONF_DIR/lns_client.key\""   >> "$CONF_FILE"
}

# ── [cups] ───────────────────────────────────────────────────────────────────
_conf_cups() {
    local enabled server oksync_interval resync_interval credentials_dir
    config_get_bool enabled         cups enabled         0
    config_get      server          cups server          ""
    config_get      oksync_interval cups oksync_interval "24h"
    config_get      resync_interval cups resync_interval "60s"
    config_get      credentials_dir cups credentials_dir "/etc/rak-basicstation/credentials"
    [ "$enabled" = "1" ] && enabled=true || enabled=false

    cat >> "$CONF_FILE" <<-EOF

	[cups]
	  enabled=$enabled
	  server="$server"
	  oksync_interval="$oksync_interval"
	  resync_interval="$resync_interval"
	  credentials_dir="$credentials_dir"
	  sig_keys=[
	EOF
    config_list_foreach cups sig_keys _conf_cups_sig_key
    echo "  ]" >> "$CONF_FILE"

    [ -f "$CONF_DIR/cups_ca.crt"     ] && echo "  ca_cert=\"$CONF_DIR/cups_ca.crt\""      >> "$CONF_FILE"
    [ -f "$CONF_DIR/cups_client.crt" ] && echo "  tls_cert=\"$CONF_DIR/cups_client.crt\""  >> "$CONF_FILE"
    [ -f "$CONF_DIR/cups_client.key" ] && echo "  tls_key=\"$CONF_DIR/cups_client.key\""   >> "$CONF_FILE"
}

_conf_cups_sig_key() { echo "    \"$1\"," >> "$CONF_FILE"; }

# ── [metadata] ───────────────────────────────────────────────────────────────
_conf_metadata() {
    local split_delimiter
    config_get split_delimiter metadata split_delimiter "="

    cat >> "$CONF_FILE" <<-EOF

	[metadata]
	  split_delimiter="$split_delimiter"

	  [metadata.static]
	EOF
    config_foreach _conf_metadata_static  metadata_static

    echo "  [metadata.commands]" >> "$CONF_FILE"
    config_foreach _conf_metadata_command metadata_commands
}

_conf_metadata_static() {
    local cfg="$1" key value
    config_get key   "$cfg" key   ""
    config_get value "$cfg" value ""
    [ -n "$key" ] && echo "    $key=\"$value\"" >> "$CONF_FILE"
}

_conf_metadata_command() {
    local cfg="$1" name
    config_get name "$cfg" name ""
    [ -z "$name" ] && return 0
    printf '    %s=[' "$name" >> "$CONF_FILE"
    config_list_foreach "$cfg" args _conf_metadata_arg
    echo "]" >> "$CONF_FILE"
}

_conf_metadata_arg() { printf '"%s",' "$1" >> "$CONF_FILE"; }
