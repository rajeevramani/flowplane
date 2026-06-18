#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

VERSION=${FLOWPLANE_RELEASE_VERSION:-$(cargo pkgid -p flowplane | sed 's/.*#//')}
TARGET=${FLOWPLANE_RELEASE_TARGET:-}
HOST=${FLOWPLANE_RELEASE_HOST:-$(uname -s | tr '[:upper:]' '[:lower:]')-$(uname -m)}
ROOT="target/release-artifacts/flowplane-v$VERSION"
ARTIFACT="flowplane-$VERSION-$HOST"
ARTIFACT_DIR="$ROOT/$ARTIFACT"
IMAGE_TAG=${FLOWPLANE_IMAGE_TAG:-"flowplane:$VERSION"}
PACKAGE_IMAGE=${FLOWPLANE_PACKAGE_IMAGE:-0}

rm -rf "$ARTIFACT_DIR"
mkdir -p "$ARTIFACT_DIR/bin" "$ARTIFACT_DIR/dataplane" "$ROOT"

BUILD_ARGS=(--release --locked)
TARGET_DIR=target/release
if [ -n "$TARGET" ]; then
  BUILD_ARGS+=(--target "$TARGET")
  TARGET_DIR="target/$TARGET/release"
  HOST=${FLOWPLANE_RELEASE_HOST:-$TARGET}
  ARTIFACT="flowplane-$VERSION-$HOST"
  ARTIFACT_DIR="$ROOT/$ARTIFACT"
  rm -rf "$ARTIFACT_DIR"
  mkdir -p "$ARTIFACT_DIR/bin" "$ARTIFACT_DIR/dataplane"
  if [ "$TARGET" = "x86_64-unknown-linux-musl" ] &&
    [ -z "${CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER:-}" ] &&
    command -v x86_64-linux-musl-gcc >/dev/null 2>&1; then
    export CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=x86_64-linux-musl-gcc
  fi
fi

cargo build "${BUILD_ARGS[@]}" -p flowplane --bin flowplane --no-default-features
cargo build "${BUILD_ARGS[@]}" -p fp-agent --bin fp-agent

cp "$TARGET_DIR/flowplane" "$ARTIFACT_DIR/bin/"
cp "$TARGET_DIR/fp-agent" "$ARTIFACT_DIR/bin/"
cargo metadata --format-version 1 > "$ROOT/flowplane-$VERSION.cargo-metadata.sbom.json"

cat > "$ARTIFACT_DIR/release-manifest.md" <<EOF
# Flowplane $VERSION Release Manifest

- CP binary: \`bin/flowplane\`
- DP sidecar binary: \`bin/fp-agent\`
- Binary target: \`$HOST\`
- Static-link decision: vendored OpenSSL is enabled for v1.0 release builds. Use
  \`FLOWPLANE_RELEASE_TARGET=x86_64-unknown-linux-musl\` and verify with \`ldd\` or \`file\`.
- Distribution caveat: public distribution waits on Q-006 license posture.
- OCI image tag: \`$IMAGE_TAG\`
- SBOM source artifact: \`flowplane-$VERSION.cargo-metadata.sbom.json\`
- Checksums: \`SHA256SUMS\`

Dataplane bootstrap:

\`\`\`bash
FLOWPLANE_SERVER=... FLOWPLANE_TOKEN=... \\
  bin/flowplane --team <team> dataplane bootstrap <dataplane> --mode mtls \\
  --xds-host <cp-xds-host> --xds-port 18000 \\
  --cert-path /etc/flowplane/tls/tls.crt \\
  --key-path /etc/flowplane/tls/tls.key \\
  --ca-path /etc/flowplane/tls/ca.crt
\`\`\`
EOF

if [ "${FLOWPLANE_PACKAGE_DATAPLANE:-0}" = "1" ]; then
  : "${FLOWPLANE_PACKAGE_TEAM:?set FLOWPLANE_PACKAGE_TEAM}"
  : "${FLOWPLANE_PACKAGE_DATAPLANE_NAME:?set FLOWPLANE_PACKAGE_DATAPLANE_NAME}"
  MODE=${FLOWPLANE_PACKAGE_DATAPLANE_MODE:-mtls}
  BOOTSTRAP_ARGS=(
    --team "$FLOWPLANE_PACKAGE_TEAM"
    dataplane bootstrap "$FLOWPLANE_PACKAGE_DATAPLANE_NAME"
    --mode "$MODE"
    --xds-host "${FLOWPLANE_PACKAGE_XDS_HOST:-127.0.0.1}"
    --xds-port "${FLOWPLANE_PACKAGE_XDS_PORT:-18000}"
    --admin-port "${FLOWPLANE_PACKAGE_ADMIN_PORT:-9901}"
  )
  if [ "$MODE" = "mtls" ]; then
    BOOTSTRAP_ARGS+=(
      --cert-path "${FLOWPLANE_PACKAGE_CERT_PATH:?set FLOWPLANE_PACKAGE_CERT_PATH}"
      --key-path "${FLOWPLANE_PACKAGE_KEY_PATH:?set FLOWPLANE_PACKAGE_KEY_PATH}"
      --ca-path "${FLOWPLANE_PACKAGE_CA_PATH:?set FLOWPLANE_PACKAGE_CA_PATH}"
    )
  fi
  "$ARTIFACT_DIR/bin/flowplane" "${BOOTSTRAP_ARGS[@]}" > "$ARTIFACT_DIR/dataplane/envoy.yaml"
fi

tar -C "$ROOT" -czf "$ROOT/$ARTIFACT.tar.gz" "$ARTIFACT"

ENGINE=
if command -v docker >/dev/null 2>&1; then
  ENGINE=docker
elif command -v podman >/dev/null 2>&1; then
  ENGINE=podman
fi

if [ "$PACKAGE_IMAGE" = "1" ]; then
  [ -n "$ENGINE" ] || { echo "FLOWPLANE_PACKAGE_IMAGE=1 requires docker or podman" >&2; exit 1; }
  "$ENGINE" info >/dev/null 2>&1 || { echo "$ENGINE is installed but not usable" >&2; exit 1; }
  "$ENGINE" build -f Containerfile.release -t "$IMAGE_TAG" .
  "$ENGINE" save -o "$ROOT/flowplane-$VERSION.oci.tar" "$IMAGE_TAG"
fi

(
  cd "$ROOT"
  rm -f SHA256SUMS
  for file in *; do
    [ -f "$file" ] || continue
    [ "$file" = "SHA256SUMS" ] && continue
    if command -v sha256sum >/dev/null 2>&1; then
      sha256sum "$file" >> SHA256SUMS
    else
      shasum -a 256 "$file" >> SHA256SUMS
    fi
  done
)

echo "release artifacts written to $ROOT"
