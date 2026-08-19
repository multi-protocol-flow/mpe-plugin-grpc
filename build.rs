//! Build script: compiles the echo test proto for the `grpc_test_server`
//! binary (tonic-build; protoc is a dev-tool dependency only — the released
//! `mpe_plugin_grpc` binary never links the generated code).

fn main() {
    println!("cargo:rerun-if-changed=src/bin/echo.proto");
    tonic_build::configure()
        .build_server(true)
        .build_client(false)
        .compile_protos(&["src/bin/echo.proto"], &["src/bin/"])
        .expect("failed to compile echo.proto");
}
