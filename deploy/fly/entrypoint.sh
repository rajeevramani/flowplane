#!/bin/sh
set -eu

if [ "$#" -gt 0 ]; then
    exec "$@"
fi

role=${FLOWPLANE_FLY_ROLE:-control-plane}

case "$role" in
    control-plane)
        flowplane db migrate
        tailscale_forward_port=18000
        tailscale_target_port=18000
        ;;
    rls)
        tailscale_forward_port=50051
        tailscale_target_port=50051
        ;;
    *)
        exit 64
        ;;
esac

if [ -n "${TAILSCALE_AUTHKEY_FILE:-}" ]; then
    /usr/local/bin/tailscaled \
        --tun=userspace-networking \
        --state=/var/lib/tailscale/tailscaled.state \
        --socket=/var/run/tailscale/tailscaled.sock &
    tailscaled_pid=$!
    trap 'kill "$tailscaled_pid" 2>/dev/null || true' EXIT INT TERM

    attempts=0
    until [ -S /var/run/tailscale/tailscaled.sock ]; do
        attempts=$((attempts + 1))
        if [ "$attempts" -ge 50 ]; then
            exit 1
        fi
        sleep 0.1
    done

    /usr/local/bin/tailscale --socket=/var/run/tailscale/tailscaled.sock up \
        --auth-key="file:${TAILSCALE_AUTHKEY_FILE}" \
        --hostname="${TAILSCALE_HOSTNAME:-fpq-flowplane-cp}" \
        --accept-dns=false \
        --accept-routes=false \
        --reset

    /usr/local/bin/tailscale --socket=/var/run/tailscale/tailscaled.sock serve \
        --bg \
        --tcp="$tailscale_forward_port" \
        "tcp://127.0.0.1:$tailscale_target_port"
fi

case "$role" in
    control-plane)
        exec flowplane serve
        ;;
    rls)
        exec flowplane-rls
        ;;
esac
