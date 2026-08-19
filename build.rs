//! Build script: compiles the echo test proto for the `grpc_test_server`
//! binary (tonic-build; protoc is a dev-tool dependency only — the released
//! `mpe_plugin_grpc` binary never links the generated code).

fn main() {
    // Skip proto compilation when protoc is not available (e.g. CI without
    // protoc). The integration tests that need the generated code are skipped
    // in CI, so the test server binary is not required there. We write a stub
    // so the binary can compile without the real generated code.
    let has_protoc = std::process::Command::new("protoc")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !has_protoc {
        println!("cargo:warning=protoc not found, writing stub echo.rs");
        let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR set by cargo");
        let stub = out_dir + "/echo.rs";
        std::fs::write(&stub, "// stub: protoc not available in CI\n")
            .expect("failed to write stub echo.rs");
        println!("cargo:rerun-if-changed=src/bin/echo.proto");
        return;
    }

    println!("cargo:rerun-if-changed=src/bin/echo.proto");
    tonic_build::configure()
        .build_server(true)
        .build_client(false)
        .compile_protos(&["src/bin/echo.proto"], &["src/bin/"])
        .expect("failed to compile echo.proto");
}
