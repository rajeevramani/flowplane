//! Build script for `flowplane-agent`.
//!
//! Compiles the shared diagnostics proto from the repo-root `proto/` tree.
//! The agent deliberately runs its own codegen (rather than depending on the
//! main `flowplane` crate) so the binary footprint stays tight.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    const PROTOS: &[&str] = &["../../proto/flowplane/diagnostics/v1/diagnostics.proto"];
    const INCLUDES: &[&str] = &["../../proto"];

    for proto in PROTOS {
        println!("cargo:rerun-if-changed={}", proto);
    }

    tonic_prost_build::configure()
        .build_server(true)
        .build_client(true)
        .compile_protos(PROTOS, INCLUDES)?;

    Ok(())
}
