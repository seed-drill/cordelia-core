#!/usr/bin/env bash
# Curl wrappers for cordelia-node API calls.
# Source this file: . lib/api.sh

BEARER_TOKEN="${BEARER_TOKEN:-test-token-fixed}"

# api_post HOST PATH BODY
# Makes a POST request and returns the JSON response.
api_post() {
    local host="$1" path="$2" body="${3:-{}}"
    curl -sf -X POST \
        -H "Authorization: Bearer ${BEARER_TOKEN}" \
        -H "Content-Type: application/json" \
        -d "$body" \
        "http://${host}${path}" 2>/dev/null
}

# api_status HOST -- returns status JSON
api_status() {
    api_post "$1" "/api/v1/status"
}

# api_peers HOST -- returns peers array
api_peers() {
    api_post "$1" "/api/v1/peers"
}

# api_diag HOST -- returns diagnostics JSON
api_diag() {
    api_post "$1" "/api/v1/diagnostics"
}

# api_write_item HOST ITEM_ID TYPE DATA_JSON GROUP_ID
api_write_item() {
    local host="$1" item_id="$2" type="$3" data="$4" group="$5"
    local body
    body=$(cat <<EOF
{
    "item_id": "${item_id}",
    "type": "${type}",
    "data": ${data},
    "meta": {
        "visibility": "group",
        "group_id": "${group}",
        "owner_id": "e2e-test",
        "author_id": "e2e-test",
        "key_version": 1
    }
}
EOF
)
    api_post "$host" "/api/v1/l2/write" "$body"
}

# api_read_item HOST ITEM_ID
api_read_item() {
    local host="$1" id="$2"
    api_post "$host" "/api/v1/l2/read" "{\"item_id\": \"${id}\"}"
}

# api_create_group HOST GROUP_ID NAME CULTURE
api_create_group() {
    local host="$1" group_id="$2" name="$3" culture="${4:-chatty}"
    local body
    body=$(cat <<EOF
{
    "group_id": "${group_id}",
    "name": "${name}",
    "culture": "${culture}",
    "security_policy": "standard"
}
EOF
)
    api_post "$host" "/api/v1/groups/create" "$body"
}

# api_add_group_member HOST GROUP_ID ENTITY_ID ROLE
api_add_group_member() {
    local host="$1" group_id="$2" entity_id="$3" role="${4:-member}"
    local body
    body=$(cat <<EOF
{
    "group_id": "${group_id}",
    "entity_id": "${entity_id}",
    "role": "${role}"
}
EOF
)
    api_post "$host" "/api/v1/groups/add_member" "$body"
}

# hot_peer_count HOST -- returns count of hot peers
hot_peer_count() {
    local host="$1"
    api_peers "$host" | jq '[.peers[] | select(.state == "hot")] | length' 2>/dev/null || echo 0
}
