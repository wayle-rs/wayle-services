//! Compiles the Mullvad management-interface protobuf schemas.
//!
//! Two independent schemas are compiled into separate output directories
//! because both declare the same protobuf package
//! (`mullvad_daemon.management_interface`) and would otherwise collide:
//!
//! * `proto/bootstrap.proto` — the version-independent bootstrap client used to
//!   query the daemon version before a backend is selected.
//! * `src/backends/2025_14/management_interface.proto` — the minimal backend
//!   schema for the 2025.14-and-newer daemon line.
//! * `src/backends/2025_9/management_interface.proto` — the minimal backend
//!   schema for the 2025.9–2025.13 (OpenVPN-era) daemon line.
//!
//! A vendored `protoc` binary is used so the build requires no system protobuf
//! compiler, and none of the schemas import the well-known types, so no
//! protobuf include directory is needed either.

use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let protoc = protoc_bin_vendored::protoc_bin_path()?;
    // SAFETY: this runs single-threaded at the start of the build script,
    // before anything reads the environment concurrently.
    #[allow(unsafe_code)]
    unsafe {
        std::env::set_var("PROTOC", protoc);
    }

    let out_dir = PathBuf::from(std::env::var("OUT_DIR")?);

    let bootstrap_out = out_dir.join("bootstrap");
    std::fs::create_dir_all(&bootstrap_out)?;
    tonic_build::configure()
        .build_server(false)
        .out_dir(&bootstrap_out)
        .compile(&["proto/bootstrap.proto"], &["proto"])?;

    let v2025_14_out = out_dir.join("v2025_14");
    std::fs::create_dir_all(&v2025_14_out)?;
    tonic_build::configure()
        .build_server(false)
        .out_dir(&v2025_14_out)
        .compile(
            &["src/backends/2025_14/management_interface.proto"],
            &["src/backends/2025_14"],
        )?;

    let v2025_9_out = out_dir.join("v2025_9");
    std::fs::create_dir_all(&v2025_9_out)?;
    tonic_build::configure()
        .build_server(false)
        .out_dir(&v2025_9_out)
        .compile(
            &["src/backends/2025_9/management_interface.proto"],
            &["src/backends/2025_9"],
        )?;

    println!("cargo:rerun-if-changed=proto/bootstrap.proto");
    println!("cargo:rerun-if-changed=src/backends/2025_14/management_interface.proto");
    println!("cargo:rerun-if-changed=src/backends/2025_9/management_interface.proto");
    Ok(())
}
