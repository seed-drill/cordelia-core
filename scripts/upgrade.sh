#!/usr/bin/env bash
set -euo pipefail

# Cordelia Node Upgrade Script
# Downloads a release binary from GitHub, verifies checksum, and restarts the service.
#
# Usage:
#   ./upgrade.sh              # upgrade to latest release
#   ./upgrade.sh v0.1.1       # upgrade to specific version

REPO="seed-drill/cordelia-core"
BINARY_NAME="cordelia-node-x86_64-linux"
INSTALL_PATH="/usr/local/bin/cordelia-node"
SERVICE_NAME="cordelia-node"
API_URL="http://127.0.0.1:9473/api/v1/status"

# Resolve version
if [ $# -ge 1 ]; then
    VERSION="$1"
else
    echo "Fetching latest release..."
    VERSION=$(curl -sf "https://api.github.com/repos/${REPO}/releases/latest" | grep '"tag_name"' | sed -E 's/.*"tag_name": *"([^"]+)".*/\1/')
    if [ -z "$VERSION" ]; then
        echo "ERROR: Could not determine latest release" >&2
        exit 1
    fi
fi

echo "Upgrading cordelia-node to ${VERSION}"

# Download binary and checksum
DOWNLOAD_URL="https://github.com/${REPO}/releases/download/${VERSION}"
TMPDIR=$(mktemp -d)
trap 'rm -rf "$TMPDIR"' EXIT

echo "Downloading binary..."
curl -sfL "${DOWNLOAD_URL}/${BINARY_NAME}" -o "${TMPDIR}/${BINARY_NAME}"

echo "Downloading checksum..."
curl -sfL "${DOWNLOAD_URL}/sha256sums.txt" -o "${TMPDIR}/sha256sums.txt"

# Verify checksum
echo "Verifying SHA-256 checksum..."
cd "$TMPDIR"
if ! sha256sum -c sha256sums.txt; then
    echo "ERROR: Checksum verification failed!" >&2
    exit 1
fi
cd - > /dev/null

echo "Checksum verified."

# Stop service
echo "Stopping ${SERVICE_NAME}..."
sudo systemctl stop "$SERVICE_NAME"

# Replace binary
echo "Installing binary to ${INSTALL_PATH}..."
sudo cp "${TMPDIR}/${BINARY_NAME}" "$INSTALL_PATH"
sudo chmod 755 "$INSTALL_PATH"

# Start service
echo "Starting ${SERVICE_NAME}..."
sudo systemctl start "$SERVICE_NAME"

# Health check (wait up to 10 seconds)
echo "Checking health..."
TOKEN=$(cat ~/.cordelia/node-token 2>/dev/null || echo "")
for i in $(seq 1 10); do
    if [ -n "$TOKEN" ]; then
        RESPONSE=$(curl -sf -X POST "$API_URL" -H "Authorization: Bearer $TOKEN" 2>/dev/null || true)
    else
        RESPONSE=$(curl -sf -X POST "$API_URL" 2>/dev/null || true)
    fi
    if [ -n "$RESPONSE" ]; then
        echo "Node is up:"
        echo "$RESPONSE" | python3 -m json.tool 2>/dev/null || echo "$RESPONSE"
        echo ""
        echo "Upgrade to ${VERSION} complete."
        exit 0
    fi
    sleep 1
done

echo "WARNING: Node did not respond within 10 seconds. Check logs:"
echo "  sudo journalctl -u ${SERVICE_NAME} -n 50 --no-pager"
exit 1
