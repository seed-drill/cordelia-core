#!/usr/bin/env bash
# Curl wrappers for cordelia-proxy and cordelia-portal API calls.
# Designed for e2e tests running from the Docker HOST (like resilience-test.sh).
#
# Proxy endpoints:  http://127.0.0.1:${PROXY_PORT}/api/...
# Portal endpoints: http://127.0.0.1:${PORTAL_PORT}/api/...
# Node endpoints:   via docker exec on orchestrator container
#
# Source this file: . lib/proxy-api.sh

PROXY_PORT="${PROXY_PORT:-3847}"
PORTAL_PORT="${PORTAL_PORT:-3001}"
PROXY_URL="http://127.0.0.1:${PROXY_PORT}"
PORTAL_URL="http://127.0.0.1:${PORTAL_PORT}"
SESSION_SECRET="${SESSION_SECRET:-e2e-test-session-secret}"
ORCH="${ORCH:-cordelia-e2e-orchestrator}"
BEARER_TOKEN="${BEARER_TOKEN:-test-token-fixed}"

# =============================================================================
# Proxy API helpers (HTTP GET/POST to proxy on host port)
# =============================================================================

# proxy_get PATH -- GET request to proxy
proxy_get() {
    local path="$1"
    curl -sf --max-time 10 "${PROXY_URL}${path}" 2>/dev/null
}

# proxy_post PATH BODY -- POST request to proxy
proxy_post() {
    local path="$1" body="${2:-{}}"
    curl -sf --max-time 10 -X POST \
        -H "Content-Type: application/json" \
        -d "$body" \
        "${PROXY_URL}${path}" 2>/dev/null
}

# proxy_put PATH BODY -- PUT request to proxy
proxy_put() {
    local path="$1" body="${2:-{}}"
    curl -sf --max-time 10 -X PUT \
        -H "Content-Type: application/json" \
        -d "$body" \
        "${PROXY_URL}${path}" 2>/dev/null
}

# proxy_delete PATH -- DELETE request to proxy
proxy_delete() {
    local path="$1"
    curl -sf --max-time 10 -X DELETE "${PROXY_URL}${path}" 2>/dev/null
}

# proxy_status -- GET /api/status
proxy_status() {
    proxy_get "/api/status"
}

# proxy_health -- GET /api/health
proxy_health() {
    proxy_get "/api/health"
}

# proxy_read_item ID -- GET /api/l2/item/:id (decrypted)
proxy_read_item() {
    local id="$1"
    proxy_get "/api/l2/item/${id}"
}

# proxy_search QUERY [TYPE] -- GET /api/l2/search
proxy_search() {
    local query="$1" type="${2:-}"
    local url="/api/l2/search?query=$(urlencode "$query")"
    [ -n "$type" ] && url="${url}&type=${type}"
    proxy_get "$url"
}

# proxy_create_group NAME [CULTURE] [SECURITY_POLICY] -- POST /api/groups
proxy_create_group() {
    local name="$1" culture="${2:-chatty}" policy="${3:-standard}"
    local id
    id=$(uuidgen 2>/dev/null || cat /proc/sys/kernel/random/uuid 2>/dev/null || echo "grp-$(date +%s)-${RANDOM}")
    id=$(echo "$id" | tr '[:upper:]' '[:lower:]')
    proxy_post "/api/groups" "{\"id\":\"${id}\",\"name\":\"${name}\",\"culture\":\"${culture}\",\"security_policy\":\"${policy}\"}"
}

# proxy_read_group ID -- GET /api/groups/:id
proxy_read_group() {
    local id="$1"
    proxy_get "/api/groups/${id}"
}

# proxy_add_member GROUP_ID ENTITY_ID [ROLE] -- POST /api/groups/:id/members
proxy_add_member() {
    local group_id="$1" entity_id="$2" role="${3:-member}"
    proxy_post "/api/groups/${group_id}/members" "{\"entity_id\":\"${entity_id}\",\"role\":\"${role}\"}"
}

# proxy_remove_member GROUP_ID ENTITY_ID -- DELETE /api/groups/:id/members/:entityId
proxy_remove_member() {
    local group_id="$1" entity_id="$2"
    proxy_delete "/api/groups/${group_id}/members/${entity_id}"
}

# proxy_export_profile USER_ID -- GET /api/profile/:userId/export
proxy_export_profile() {
    local user_id="$1"
    proxy_get "/api/profile/${user_id}/export"
}

# proxy_delete_profile USER_ID -- DELETE /api/profile/:userId?deleteL2=true
proxy_delete_profile() {
    local user_id="$1"
    curl -sf --max-time 10 -X DELETE "${PROXY_URL}/api/profile/${user_id}?deleteL2=true" 2>/dev/null
}

# =============================================================================
# Portal API helpers (require session cookie)
# =============================================================================

# portal_get PATH COOKIE -- GET request to portal with session cookie
portal_get() {
    local path="$1" cookie="${2:-}"
    local cookie_args=""
    [ -n "$cookie" ] && cookie_args="-b portal_session=${cookie}"
    # shellcheck disable=SC2086
    curl -sf --max-time 10 $cookie_args "${PORTAL_URL}${path}" 2>/dev/null
}

# portal_post PATH BODY COOKIE -- POST request to portal with session cookie
portal_post() {
    local path="$1" body="${2:-{}}" cookie="${3:-}"
    local cookie_args=""
    [ -n "$cookie" ] && cookie_args="-b portal_session=${cookie}"
    # shellcheck disable=SC2086
    curl -sf --max-time 10 -X POST \
        -H "Content-Type: application/json" \
        -d "$body" \
        $cookie_args \
        "${PORTAL_URL}${path}" 2>/dev/null
}

# portal_delete PATH COOKIE -- DELETE request to portal with session cookie
portal_delete() {
    local path="$1" cookie="${2:-}"
    local cookie_args=""
    [ -n "$cookie" ] && cookie_args="-b portal_session=${cookie}"
    # shellcheck disable=SC2086
    curl -sf --max-time 10 -X DELETE $cookie_args "${PORTAL_URL}${path}" 2>/dev/null
}

# portal_health -- GET /api/health (no auth)
portal_health() {
    curl -sf --max-time 10 "${PORTAL_URL}/api/health" 2>/dev/null
}

# portal_generate_device_code COOKIE -- POST /api/enroll/device-code
portal_generate_device_code() {
    local cookie="$1"
    portal_post "/api/enroll/device-code" '{"scope":"node"}' "$cookie"
}

# portal_authorize COOKIE USER_CODE PASSPHRASE -- POST /api/enroll/authorize
portal_authorize() {
    local cookie="$1" user_code="$2" passphrase="$3"
    portal_post "/api/enroll/authorize" \
        "{\"user_code\":\"${user_code}\",\"passphrase\":\"${passphrase}\"}" \
        "$cookie"
}

# portal_poll_device DEVICE_CODE -- GET /api/enroll/poll/:device_code (no auth)
portal_poll_device() {
    local device_code="$1"
    curl -sf --max-time 10 "${PORTAL_URL}/api/enroll/poll/${device_code}" 2>/dev/null
}

# portal_poll_user USER_CODE [X25519_PUB] -- GET /api/enroll/poll-user/:user_code (no auth)
portal_poll_user() {
    local user_code="$1" x25519_pub="${2:-}"
    local url="${PORTAL_URL}/api/enroll/poll-user/${user_code}"
    [ -n "$x25519_pub" ] && url="${url}?x25519_pub=${x25519_pub}"
    curl -sf --max-time 10 "$url" 2>/dev/null
}

# portal_rotate_group_key COOKIE GROUP_ID PASSPHRASE -- POST /api/vault/rotate-group-key/:groupId
portal_rotate_group_key() {
    local cookie="$1" group_id="$2" passphrase="$3"
    portal_post "/api/vault/rotate-group-key/${group_id}" \
        "{\"passphrase\":\"${passphrase}\"}" \
        "$cookie"
}

# portal_change_passphrase COOKIE OLD_PASS NEW_PASS -- POST /api/vault/change-passphrase
portal_change_passphrase() {
    local cookie="$1" old_pass="$2" new_pass="$3"
    portal_post "/api/vault/change-passphrase" \
        "{\"old_passphrase\":\"${old_pass}\",\"new_passphrase\":\"${new_pass}\"}" \
        "$cookie"
}

# portal_get_group_keys COOKIE ENTITY_ID -- GET /api/vault/group-keys/:entityId
portal_get_group_keys() {
    local cookie="$1" entity_id="$2"
    portal_get "/api/vault/group-keys/${entity_id}" "$cookie"
}

# portal_create_group COOKIE NAME [PASSPHRASE] -- POST /api/groups
portal_create_group() {
    local cookie="$1" name="$2" passphrase="${3:-}"
    local body="{\"name\":\"${name}\"}"
    [ -n "$passphrase" ] && body="{\"name\":\"${name}\",\"passphrase\":\"${passphrase}\"}"
    portal_post "/api/groups" "$body" "$cookie"
}

# portal_invite_member COOKIE GROUP_ID INVITEE_ENTITY_ID PASSPHRASE [ROLE]
portal_invite_member() {
    local cookie="$1" group_id="$2" invitee="$3" passphrase="$4" role="${5:-member}"
    portal_post "/api/groups/${group_id}/invite" \
        "{\"invitee_entity_id\":\"${invitee}\",\"passphrase\":\"${passphrase}\",\"role\":\"${role}\"}" \
        "$cookie"
}

# portal_remove_member COOKIE GROUP_ID ENTITY_ID
portal_remove_member() {
    local cookie="$1" group_id="$2" entity_id="$3"
    portal_delete "/api/groups/${group_id}/members/${entity_id}" "$cookie"
}

# =============================================================================
# Session management: create portal session via direct DB insert + cookie signing
# =============================================================================

# sign_cookie VALUE SECRET -- compute cookie-signature compatible HMAC
# Output: s:<value>.<base64-hmac>
sign_cookie() {
    local value="$1" secret="$2"
    local hmac
    hmac=$(printf '%s' "$value" | openssl dgst -sha256 -hmac "$secret" -binary | openssl base64 -A | sed 's/=*$//')
    echo "s:${value}.${hmac}"
}

# portal_create_session ENTITY_ID -- create user + session in portal DB, return signed cookie
# Uses docker exec to insert into portal's SQLite DB.
portal_create_session() {
    local entity_id="$1"
    local session_id
    session_id=$(python3 -c "import uuid; print(uuid.uuid4())" 2>/dev/null || \
                 uuidgen 2>/dev/null || \
                 echo "sess-$(date +%s)-${RANDOM}")

    # Create user and session in portal DB via docker exec + node
    docker exec cordelia-e2e-portal node -e "
        const Database = require('better-sqlite3');
        const db = new Database(process.env.PORTAL_DB || '/data/portal.db');
        db.exec('INSERT OR IGNORE INTO users (entity_id, display_name) VALUES (\"${entity_id}\", \"E2E Test User\")');
        db.exec('INSERT INTO sessions (id, entity_id, provider, expires_at) VALUES (\"${session_id}\", \"${entity_id}\", \"test\", datetime(\"now\", \"+7 days\"))');
        db.close();
        console.log('ok');
    " > /dev/null 2>&1

    # Return the signed cookie value
    sign_cookie "$session_id" "$SESSION_SECRET"
}

# =============================================================================
# Node API helpers (via docker exec on orchestrator -- inside Docker network)
# =============================================================================

# node_api HOST ENDPOINT [BODY] -- POST to Rust node API via orchestrator
node_api() {
    local host="$1" endpoint="$2" body="${3:-{}}"
    docker exec "$ORCH" curl -sf --max-time 5 \
        -X POST \
        -H "Authorization: Bearer ${BEARER_TOKEN}" \
        -H "Content-Type: application/json" \
        -d "$body" \
        "http://${host}:9473/api/v1/${endpoint}" 2>/dev/null
}

# node_write_item HOST ITEM_ID TYPE DATA GROUP_ID -- write item to Rust node (plaintext, no encryption)
node_write_item() {
    local host="$1" item_id="$2" type="$3" data="$4" group="$5"
    node_api "$host" "l2/write" "{
        \"item_id\": \"${item_id}\",
        \"type\": \"${type}\",
        \"data\": ${data},
        \"meta\": {
            \"visibility\": \"group\",
            \"group_id\": \"${group}\",
            \"owner_id\": \"e2e-test\",
            \"author_id\": \"e2e-test\",
            \"key_version\": 1
        }
    }"
}

# node_read_item HOST ITEM_ID -- read raw item from Rust node
node_read_item() {
    local host="$1" id="$2"
    node_api "$host" "l2/read" "{\"item_id\": \"${id}\"}"
}

# =============================================================================
# Encryption helpers: AES-256-GCM operations in bash via openssl
# =============================================================================

# generate_psk -- generate 32 random bytes, output as hex
generate_psk() {
    openssl rand -hex 32
}

# provision_proxy_psk GROUP_ID PSK_HEX [VERSION] -- write PSK to proxy container's key ring
provision_proxy_psk() {
    local group_id="$1" psk_hex="$2" version="${3:-1}"
    docker exec cordelia-e2e-proxy sh -c "
        mkdir -p /home/cordelia/.cordelia/group-keys 2>/dev/null || mkdir -p ~/.cordelia/group-keys
        KEYDIR=\$(ls -d /home/cordelia/.cordelia/group-keys 2>/dev/null || echo ~/.cordelia/group-keys)
        cat > \"\${KEYDIR}/${group_id}.json\" <<KEYEOF
{
  \"versions\": { \"${version}\": \"${psk_hex}\" },
  \"latest\": ${version}
}
KEYEOF
        chmod 600 \"\${KEYDIR}/${group_id}.json\"
    "
}

# clear_proxy_key_cache -- clear the proxy's in-memory PSK cache so it reloads from disk
clear_proxy_key_cache() {
    docker exec cordelia-e2e-proxy node -e '
        try { require("./dist/group-keys.js").clearGroupKeyCache(); } catch(e) {}
    ' > /dev/null 2>&1
}

# add_psk_version GROUP_ID PSK_HEX VERSION -- add a new version to existing key ring
add_psk_version() {
    local group_id="$1" psk_hex="$2" version="$3"
    docker exec cordelia-e2e-proxy node -e "
        const fs = require('fs');
        const path = require('path');
        const os = require('os');
        const keyDir = path.join(os.homedir(), '.cordelia', 'group-keys');
        const keyFile = path.join(keyDir, '${group_id}.json');
        let ring = { versions: {}, latest: 0 };
        try { ring = JSON.parse(fs.readFileSync(keyFile, 'utf-8')); } catch {}
        ring.versions['${version}'] = '${psk_hex}';
        if (${version} > ring.latest) ring.latest = ${version};
        fs.writeFileSync(keyFile, JSON.stringify(ring, null, 2), { mode: 0o600 });
        console.log('ok');
    " 2>/dev/null
}

# encrypt_aes256gcm PSK_HEX PLAINTEXT -- encrypt with AES-256-GCM, output JSON payload
# Returns: {"_encrypted":true,"version":1,"iv":"...","authTag":"...","ciphertext":"..."}
# Uses node (available on CI runner and dev machines) for proper GCM auth tag handling.
encrypt_aes256gcm() {
    local psk_hex="$1" plaintext="$2"
    # Use docker exec on the proxy container (has node) to avoid host node dependency
    docker exec cordelia-e2e-proxy node -e "
        const crypto = require('crypto');
        const key = Buffer.from('${psk_hex}', 'hex');
        const iv = crypto.randomBytes(12);
        const pt = Buffer.from(process.argv[1], 'utf-8');
        const cipher = crypto.createCipheriv('aes-256-gcm', key, iv, { authTagLength: 16 });
        const enc = Buffer.concat([cipher.update(pt), cipher.final()]);
        const tag = cipher.getAuthTag();
        console.log(JSON.stringify({
            _encrypted: true,
            version: 1,
            iv: iv.toString('base64'),
            authTag: tag.toString('base64'),
            ciphertext: enc.toString('base64')
        }));
    " -- "$plaintext"
}

# write_encrypted_to_node HOST ITEM_ID TYPE ENCRYPTED_JSON GROUP_ID -- write pre-encrypted blob to node
write_encrypted_to_node() {
    local host="$1" item_id="$2" type="$3" encrypted_json="$4" group="$5"
    # The encrypted_json IS the data blob -- the node stores it opaquely
    node_api "$host" "l2/write" "{
        \"item_id\": \"${item_id}\",
        \"type\": \"${type}\",
        \"data\": ${encrypted_json},
        \"meta\": {
            \"visibility\": \"group\",
            \"group_id\": \"${group}\",
            \"owner_id\": \"e2e-test\",
            \"author_id\": \"e2e-test\",
            \"key_version\": 1
        }
    }"
}

# =============================================================================
# Proxy DB helpers: insert items directly into proxy's SQLite
# Uses env vars to avoid shell quoting issues with JSON data.
# =============================================================================

# proxy_db_insert_item ITEM_ID TYPE ENCRYPTED_JSON GROUP_ID OWNER_ID KEY_VERSION
# Insert an item directly into the proxy's SQLite database.
proxy_db_insert_item() {
    local item_id="$1" type="$2" data="$3" group_id="$4"
    local owner_id="${5:-e2e-test}" key_version="${6:-1}"
    # Pass data via environment variable to avoid quoting issues
    docker exec -e "ITEM_ID=${item_id}" \
                -e "ITEM_TYPE=${type}" \
                -e "ITEM_DATA=${data}" \
                -e "GROUP_ID=${group_id}" \
                -e "OWNER_ID=${owner_id}" \
                -e "KEY_VERSION=${key_version}" \
        cordelia-e2e-proxy node -e '
        const Database = require("better-sqlite3");
        const db = new Database("/app/memory/cordelia.db");
        const stmt = db.prepare(
            "INSERT OR REPLACE INTO l2_items (id, type, data, group_id, owner_id, visibility, key_version, domain, access_count) VALUES (?, ?, ?, ?, ?, ?, ?, ?, 0)"
        );
        stmt.run(
            process.env.ITEM_ID,
            process.env.ITEM_TYPE,
            process.env.ITEM_DATA,
            process.env.GROUP_ID,
            process.env.OWNER_ID,
            "group",
            parseInt(process.env.KEY_VERSION),
            "procedural"
        );
        db.close();
    ' > /dev/null 2>&1
}

# =============================================================================
# Utility
# =============================================================================

# urlencode STRING -- percent-encode a string
urlencode() {
    python3 -c "import urllib.parse; print(urllib.parse.quote('$1'))" 2>/dev/null || echo "$1"
}

# wait_proxy_healthy TIMEOUT -- wait for proxy /api/health to respond
wait_proxy_healthy() {
    local timeout="${1:-60}"
    local deadline=$((SECONDS + timeout))
    while [ "$SECONDS" -lt "$deadline" ]; do
        if proxy_health > /dev/null 2>&1; then
            return 0
        fi
        sleep 2
    done
    return 1
}

# wait_portal_healthy TIMEOUT -- wait for portal /api/health to respond
wait_portal_healthy() {
    local timeout="${1:-60}"
    local deadline=$((SECONDS + timeout))
    while [ "$SECONDS" -lt "$deadline" ]; do
        if portal_health > /dev/null 2>&1; then
            return 0
        fi
        sleep 2
    done
    return 1
}
