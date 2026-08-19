//! Build script: compiles the echo test proto for the `grpc_test_server`
//! binary (tonic-build; protoc is a dev-tool dependency only — the released
//! `mpe_plugin_grpc` binary never links the generated code).

fn main() {
    // Only compile the test server proto when building the test server binary.
    // The main plugin binary (mpe_plugin_grpc) never references the generated
    // code, so skipping proto compilation avoids requiring `protoc` in CI
    // for normal builds.
    let bin_name = std::env::var("CARGO_BIN_NAME").unwrap_or_default();
    if bin_name != "grpc_test_server" {
        return;
    }

    // Skip gracefully when protoc is not available (e.g. CI without protoc).
    // The integration tests that need the generated code are skipped in CI,
    // so the test server binary is not required there.
    let has_protoc = std::process::Command::new("protoc")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !has_protoc {
        println!("cargo:warning=protoc not found, skipping echo.proto compilation");
        return;
    }

    println!("cargo:rerun-if-changed=src/bin/echo.proto");
    tonic_build::configure()
        .build_server(true)
        .build_client(false)
        .compile_protos(&["src/bin/echo.proto"], &["src/bin/"])
        .expect("failed to compile echo.proto");
}
