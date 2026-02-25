use handlebars::Handlebars;

use crate::config::Configuration;

pub fn run(config: &Configuration) {
    let template = r#"
# Logging settings.
[logging]

  # Log level.
  #
  # Valid options are:
  #   * TRACE
  #   * DEBUG
  #   * INFO
  #   * WARN
  #   * ERROR
  #   * OFF
  level="{{ logging.level }}"

  # Log to syslog.
  #
  # If set to true, log messages are being written to syslog instead of stdout.
  log_to_syslog={{ logging.log_to_syslog }}


# Backend configuration.
[backend]

  # Backend to use.
  #
  # Valid options are:
  #   * concentratord  - ChirpStack Concentratord (ZMQ IPC)
  #   * semtech_udp    - Semtech UDP Packet Forwarder
  enabled="{{ backend.enabled }}"

  # Gateway ID (optional).
  #
  # For concentratord: if empty, retrieved via GetGatewayId ZMQ command.
  # For semtech_udp: if empty, auto-discovered from first PULL_DATA packet.
  gateway_id="{{ backend.gateway_id }}"

  # Uplink CRC filters.
  [backend.filters]

    # Forward CRC ok.
    forward_crc_ok={{ backend.filters.forward_crc_ok }}

    # Forward CRC invalid.
    forward_crc_invalid={{ backend.filters.forward_crc_invalid }}

    # Forward CRC missing.
    forward_crc_missing={{ backend.filters.forward_crc_missing }}

  # ChirpStack Concentratord backend configuration.
  [backend.concentratord]

    # Event API URL.
    event_url="{{ backend.concentratord.event_url }}"

    # Command API URL.
    command_url="{{ backend.concentratord.command_url }}"

    # Cache full rx_info.context on uplink and restore it verbatim on the
    # matching downlink. 
    # Enable this when using chirpstack-gateway-mesh as backend.
    context_caching={{ backend.concentratord.context_caching }}

  # Semtech UDP Packet Forwarder backend configuration.
  [backend.semtech_udp]

    # UDP bind address.
    #
    # The address on which the backend will listen for packets from the
    # Semtech UDP Packet Forwarder.
    bind="{{ backend.semtech_udp.bind }}"

    # Time fallback.
    #
    # Use system time as fallback when no time field is present in rxpk.
    time_fallback_enabled={{ backend.semtech_udp.time_fallback_enabled }}


# LNS (LoRa Network Server) protocol configuration.
[lns]

  # LNS server URI.
  #
  # This is the WebSocket URI of the LNS MUXS endpoint.
  # Example: wss://lns.example.com:8887
  server="{{ lns.server }}"

  # Discovery endpoint (optional).
  #
  # If set, the gateway will first query this endpoint to discover the
  # MUXS WebSocket URI. If discovery fails, falls back to 'server'.
  # Example: https://lns.example.com:8887
  discovery_endpoint="{{ lns.discovery_endpoint }}"

  # Reconnection interval after disconnect.
  reconnect_interval="{{ lns.reconnect_interval }}"

  # CA certificate file (optional).
  #
  # Use this when the server uses a self-signed certificate or a CA
  # not trusted by the system.
  ca_cert="{{ lns.ca_cert }}"

  # TLS client certificate file (optional).
  #
  # For mutual TLS authentication.
  tls_cert="{{ lns.tls_cert }}"

  # TLS client key file (optional).
  #
  # For mutual TLS authentication. Can also contain an authorization
  # token for token-based authentication.
  tls_key="{{ lns.tls_key }}"


# CUPS (Configuration and Update Server) protocol configuration.
[cups]

  # Enable CUPS.
  enabled={{ cups.enabled }}

  # CUPS server URI.
  #
  # Example: https://cups.example.com:443
  server="{{ cups.server }}"

  # Update check interval when last check succeeded.
  oksync_interval="{{ cups.oksync_interval }}"

  # Update check interval when last check failed.
  resync_interval="{{ cups.resync_interval }}"

  # CA certificate file (optional).
  ca_cert="{{ cups.ca_cert }}"

  # TLS client certificate file (optional).
  tls_cert="{{ cups.tls_cert }}"

  # TLS client key file (optional).
  tls_key="{{ cups.tls_key }}"

  # Directory for persisted credentials.
  credentials_dir="{{ cups.credentials_dir }}"

  # Signature verification key file paths.
  sig_keys=[
    {{#each cups.sig_keys}}
    "{{this}}",
    {{/each}}
  ]


# Gateway metadata configuration.
[metadata]

  # Static key / value metadata.
  [metadata.static]

    # Example:
    # serial_number="1234"
    {{#each metadata.static}}
    {{ @key }}="{{ this }}"
    {{/each}}

  # Split delimiter for command output key=value parsing.
  split_delimiter="{{metadata.split_delimiter}}"

  # Commands returning metadata.
  [metadata.commands]

    # Example:
    # datetime=["date", "-R"]
    {{#each metadata.commands}}
    {{ @key }}=[
      {{#each this}}
      "{{ this }}",
      {{/each}}
    ]
    {{/each}}
"#;

    let mut reg = Handlebars::new();
    reg.register_escape_fn(|s| s.to_string().replace('"', r#"\""#));
    println!(
        "{}",
        reg.render_template(template, config)
            .expect("Render configfile error")
    );
}
