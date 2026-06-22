# E2E phase P6 — sourced by scripts/e2e/run.sh (shares the runner shell: $TOKEN, auth[], $ENVOY_MODE, helpers).
REQUIRES=""

# ---- Phase 6: SDS-backed downstream TLS rotation. The listener references a
# tls_certificate secret by name; rotating the secret must update the certificate presented by
# the already-running Envoy without restart.
SDS_PORT=$((GW_PORT+4))
command -v openssl >/dev/null || fail "openssl is required for SDS rotation phase"
mkdir -p /tmp/fp-e2e-sds
openssl req -x509 -newkey rsa:2048 -nodes -days 1 -subj "/CN=fp-sds-one" \
  -keyout /tmp/fp-e2e-sds/one.key -out /tmp/fp-e2e-sds/one.crt >/dev/null 2>&1
openssl req -x509 -newkey rsa:2048 -nodes -days 1 -subj "/CN=fp-sds-two" \
  -keyout /tmp/fp-e2e-sds/two.key -out /tmp/fp-e2e-sds/two.crt >/dev/null 2>&1
python3 - /tmp/fp-e2e-sds/one.crt /tmp/fp-e2e-sds/one.key > /tmp/fp-e2e-sds/secret-one.json <<'PY'
import json, sys
cert, key = sys.argv[1], sys.argv[2]
print(json.dumps({
    "name": "edge-sds",
    "spec": {
        "type": "tls_certificate",
        "certificate_chain": open(cert).read(),
        "private_key": open(key).read(),
    },
}))
PY
python3 - /tmp/fp-e2e-sds/two.crt /tmp/fp-e2e-sds/two.key > /tmp/fp-e2e-sds/secret-two.json <<'PY'
import json, sys
cert, key = sys.argv[1], sys.argv[2]
print(json.dumps({
    "spec": {
        "type": "tls_certificate",
        "certificate_chain": open(cert).read(),
        "private_key": open(key).read(),
    },
}))
PY
curl -fsS "${auth[@]}" -X POST http://$API/api/v1/teams/default/secrets \
  --data-binary @/tmp/fp-e2e-sds/secret-one.json >/dev/null
curl -fsS "${auth[@]}" -X POST http://$API/api/v1/teams/default/listeners -d "{
  \"name\":\"e2e-sds\",
  \"spec\":{\"address\":\"0.0.0.0\",\"port\":$SDS_PORT,\"route_config\":\"e2e-auth-routes\",
    \"tls_context\":{\"tls_certificate_sds_secret_name\":\"edge-sds\"}}}" >/dev/null

for i in $(seq 1 40); do
  curl -fksS --max-time 2 https://127.0.0.1:$SDS_PORT/ >/dev/null 2>&1 && break
  sleep 1
done
SUBJECT=$(echo | openssl s_client -connect 127.0.0.1:$SDS_PORT -servername localhost 2>/dev/null \
  | openssl x509 -noout -subject 2>/dev/null || true)
echo "$SUBJECT" | grep -q "fp-sds-one" || fail "SDS listener did not present initial cert (subject: $SUBJECT)"
SECRET_REV=$(psql "$PG_DB_URL" -Atc "SELECT version FROM secrets WHERE name = 'edge-sds'")
curl -fsS "${auth[@]}" -X POST -H "If-Match: $SECRET_REV" http://$API/api/v1/teams/default/secrets/edge-sds/rotate \
  --data-binary @/tmp/fp-e2e-sds/secret-two.json >/dev/null
for i in $(seq 1 40); do
  SUBJECT=$(echo | openssl s_client -connect 127.0.0.1:$SDS_PORT -servername localhost 2>/dev/null \
    | openssl x509 -noout -subject 2>/dev/null || true)
  echo "$SUBJECT" | grep -q "fp-sds-two" && break
  sleep 1
done
echo "$SUBJECT" | grep -q "fp-sds-two" || fail "SDS rotation did not update Envoy cert (subject: $SUBJECT)"
curl -fksS --max-time 2 https://127.0.0.1:$SDS_PORT/ >/dev/null 2>&1 \
  || fail "HTTPS traffic failed after SDS rotation"
echo "PHASE 6 OK: SDS TLS secret rotated live; Envoy presented the new certificate"
