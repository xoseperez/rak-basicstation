#!/usr/bin/env bash

set -e

PACKAGE_NAME=$(cargo metadata --no-deps --format-version 1 | jq -r '.packages[0].name')
PACKAGE_VERSION=$(cargo metadata --no-deps --format-version 1 | jq -r '.packages[0].version')
PACKAGE_DESCRIPTION=$(cargo metadata --no-deps --format-version 1 | jq -r '.packages[0].description')

PACKAGE_DIR=$(mktemp -d)

# Create directory structure
mkdir -p "${PACKAGE_DIR}/CONTROL"
mkdir -p "${PACKAGE_DIR}/usr/bin"
mkdir -p "${PACKAGE_DIR}/etc/${PACKAGE_NAME}"
mkdir -p "${PACKAGE_DIR}/etc/init.d"

# Binary
cp ../../../../target/mipsel-unknown-linux-musl/release/rak-basicstation "${PACKAGE_DIR}/usr/bin/"

# Config
cp files/rak-basicstation.toml "${PACKAGE_DIR}/etc/${PACKAGE_NAME}/"

# Init script
cp files/rak-basicstation.init "${PACKAGE_DIR}/etc/init.d/rak-basicstation"
chmod +x "${PACKAGE_DIR}/etc/init.d/rak-basicstation"

# Control file
cat > "${PACKAGE_DIR}/CONTROL/control" << EOF
Package: ${PACKAGE_NAME}
Version: ${PACKAGE_VERSION}
Architecture: mipsel_24kc
Maintainer: RAK Wireless
Description: ${PACKAGE_DESCRIPTION}
EOF

# Build the .ipk
opkg-build "${PACKAGE_DIR}" .

# Cleanup
rm -rf "${PACKAGE_DIR}"
