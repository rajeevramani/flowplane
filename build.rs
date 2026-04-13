//! Build script for Flowplane.
//!
//! Compiles Flowplane-owned gRPC protos via tonic-build. Envoy / xDS protos
//! come pre-generated from the `envoy-types` crate — only protos we OWN live
//! under `proto/` and go through this build script.
//!
//! Adding a new proto:
//!   1. Drop the `.proto` under `proto/flowplane/<package>/<version>/`
//!   2. Add it to the `PROTOS` slice below
//!   3. `cargo check` to regenerate

fn main() -> Result<(), Box<dyn std::error::Error>> {
    const PROTOS: &[&str] = &["proto/flowplane/diagnostics/v1/diagnostics.proto"];
    const INCLUDES: &[&str] = &["proto"];

    for proto in PROTOS {
        println!("cargo:rerun-if-changed={}", proto);
    }

    tonic_prost_build::configure()
        .build_server(true)
        .build_client(true)
        .compile_protos(PROTOS, INCLUDES)?;

    Ok(())
}
