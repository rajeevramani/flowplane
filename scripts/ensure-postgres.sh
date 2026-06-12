#!/usr/bin/env bash
# Idempotent: start PostgreSQL if down and ensure the dev database exists.
# Used by the Claude session hook and by developers in disposable environments.
set -e
if ! pg_isready -q 2>/dev/null; then
  service postgresql start >/dev/null 2>&1 || pg_ctlcluster 16 main start >/dev/null 2>&1 || true
  for i in $(seq 1 15); do pg_isready -q 2>/dev/null && break; sleep 1; done
fi
pg_isready -q || { echo "postgres failed to start" >&2; exit 1; }
su postgres -s /bin/bash -c "psql -tc \"SELECT 1 FROM pg_database WHERE datname='flowplane_dev'\" | grep -q 1 || createdb flowplane_dev" 2>/dev/null || true
su postgres -s /bin/bash -c "psql -c \"ALTER USER postgres PASSWORD 'postgres'\"" >/dev/null 2>&1 || true
echo "postgres ready"
