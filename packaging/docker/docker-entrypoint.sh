#!/bin/sh

# Set defaults for environment variables used in the config template.
# These match the Rust struct defaults in src/config.rs.

: "${LOG_LEVEL:=info}"
: "${BACKEND_ENABLED:=concentratord}"
: "${BACKEND_GATEWAY_ID:=}"
: "${CONCENTRATORD_EVENT_URL:=ipc:///tmp/concentratord_event}"
: "${CONCENTRATORD_COMMAND_URL:=ipc:///tmp/concentratord_command}"
: "${CONCENTRATORD_CONTEXT_CACHING:=false}"
: "${SEMTECH_UDP_BIND:=0.0.0.0:1700}"
: "${LNS_SERVER:=wss://localhost:8887}"
: "${LNS_RECONNECT_INTERVAL:=5s}"
: "${CUPS_ENABLED:=false}"
: "${CUPS_SERVER:=}"

export LOG_LEVEL BACKEND_ENABLED BACKEND_GATEWAY_ID \
       CONCENTRATORD_EVENT_URL CONCENTRATORD_COMMAND_URL \
       CONCENTRATORD_CONTEXT_CACHING \
       SEMTECH_UDP_BIND LNS_SERVER LNS_RECONNECT_INTERVAL \
       CUPS_ENABLED CUPS_SERVER

exec "$@"
