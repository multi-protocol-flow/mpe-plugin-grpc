//! Offline stdio JSON-RPC roundtrip tests against the compiled
//! `mpe_plugin_grpc` binary — the exact transport shape the host uses:
//! JSON-RPC 2.0 requests on stdin, LF-framed responses on stdout.
//!
//! Fully offline: no gRPC server, no network. Every `execute` case
//! short-circuits before any socket I/O — call-without-connect fails at the
//! pool lookup, unknown/missing type fails at the dispatch match.
//!
//! Wire notes mirrored from `mpe-plugin-sdk/tests/roundtrip.rs`:
//! - The runtime spawns one task per `execute` and correlates responses by
//!   request `id`; responses may interleave with other traffic.
//! - Notifications (no `id`) never produce a response; readers skip them.
//! - A clean EOF on stdin (dropped write end) exits the loop with code 0.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::Value;

/// Path to the compiled plugin binary (set by cargo for `[[bin]]` targets).
const PLUGIN_BIN: &str = env!("CARGO_BIN_EXE_mpe_plugin_grpc");

/// The 3 node type_ids the plugin must describe (byte-sorted).
const EXPECTED_TYPE_IDS: [&str; 3] = ["grpc:call", "grpc:close", "grpc:connect"];

/// A spawned plugin process with piped stdio.
struct PluginProcess {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl PluginProcess {
    fn spawn() -> PluginProcess {
        let mut child = Command::new(PLUGIN_BIN)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to spawn mpe_plugin_grpc");
        let stdin = child.stdin.take().expect("plugin stdin unavailable");
        let stdout = child.stdout.take().expect("plugin stdout unavailable");
        PluginProcess {
            child,
            stdin: Some(stdin),
            stdout: BufReader::new(stdout),
            next_id: 0,
        }
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        let frame = serde_json::to_string(&serde_json::json!({
            "jsonrpc": "2.0", "id": id, "method": method, "params": params,
        }))
        .expect("request serializable");
        let stdin = self.stdin.as_mut().expect("stdin alive");
        writeln!(stdin, "{frame}").expect("request write");
        stdin.flush().expect("request flush");

        let mut line = String::new();
        loop {
            line.clear();
            let n = self.stdout.read_line(&mut line).expect("read response frame");
            assert!(n > 0, "plugin stdout closed early");
            let frame: Value = serde_json::from_str(&line).expect("frame is valid JSON");
            if frame.get("id").is_some() {
                return frame;
            }
            // 无 id → log/grpc.stream 通知帧，跳过。
        }
    }

    fn shutdown(mut self) {
        drop(self.stdin.take());
        let status = self.child.wait().expect("plugin exit");
        assert!(status.success(), "plugin should exit cleanly");
    }
}

/// `describe` returns exactly the 3 expected node types with inline
/// frontend + viewer pages and the single_node capability on `grpc:connect`.
#[test]
fn describe_lists_all_nodes() {
    let mut process = PluginProcess::spawn();
    let resp = process.request("describe", serde_json::json!({}));
    // SDK describe answers a bare array of NodeDescriptions (array shape).
    let nodes = resp["result"].as_array().expect("describe nodes array");

    let mut type_ids: Vec<&str> = nodes
        .iter()
        .filter_map(|n| n["type_id"].as_str())
        .collect();
    type_ids.sort_unstable();
    assert_eq!(type_ids, EXPECTED_TYPE_IDS, "all 3 grpc nodes described");

    for node in nodes {
        let frontend = &node["frontend"];
        assert_eq!(frontend["type"], "inline", "every node carries an inline panel");
        assert!(
            frontend["content"].as_str().is_some_and(|c| c.len() >= 50),
            "inline panel content must be non-trivial"
        );
        let viewer = &node["viewer"];
        assert_eq!(viewer["type"], "inline", "every node carries an inline viewer");
        assert!(
            viewer["content"].as_str().is_some_and(|c| c.len() >= 50),
            "inline viewer content must be non-trivial"
        );
        assert_eq!(node["category"], "grpc");
        assert_eq!(node["color"], "#8B5CF6");
        assert!(
            node["default_config"].get("type").is_none(),
            "default_config must not carry a `type` discriminator"
        );
    }

    let connect = nodes
        .iter()
        .find(|n| n["type_id"] == "grpc:connect")
        .expect("grpc:connect described");
    assert_eq!(connect["capabilities"]["single_node"], true);

    let call = nodes
        .iter()
        .find(|n| n["type_id"] == "grpc:call")
        .expect("grpc:call described");
    assert_eq!(
        call["config_schema"]["properties"]["connection_id"]["x-node-selector"]["node_type"],
        "grpc:connect"
    );

    process.shutdown();
}

/// `grpc:call` without a preceding connect fails at the pool lookup — no
/// server needed, proves the pool gate runs before any socket I/O.
#[test]
fn call_without_connect_fails_offline() {
    let mut process = PluginProcess::spawn();
    let resp = process.request(
        "execute",
        serde_json::json!({
            "execution_id": "exec-1",
            "node_instance_id": "node-call-1",
            "config": {
                "type": "grpc:call",
                "connection_id": "never-connected",
                "service_name": "echo.EchoService",
                "method_name": "Unary",
                "request_json": "{}",
            }
        }),
    );
    assert_eq!(resp["result"]["success"], false);
    let serialized = serde_json::to_string(&resp["result"]).unwrap_or_default();
    assert!(
        serialized.contains("does not exist"),
        "pool gate must produce a readable error: {serialized}"
    );
    process.shutdown();
}

/// Unknown node type fails at the dispatch match with a readable error.
#[test]
fn unknown_node_type_fails() {
    let mut process = PluginProcess::spawn();
    let resp = process.request(
        "execute",
        serde_json::json!({ "config": { "type": "grpc:totally_unknown" } }),
    );
    assert_eq!(resp["result"]["success"], false);
    let serialized = serde_json::to_string(&resp["result"]).unwrap_or_default();
    assert!(
        serialized.contains("Unknown node type"),
        "dispatch must name the unknown type: {serialized}"
    );
    process.shutdown();
}

/// `grpc:close` without a connect is a failure with a readable message
/// (explicit release of a missing connection is an error, unlike the
/// flow_ended no-op path).
#[test]
fn close_without_connect_fails() {
    let mut process = PluginProcess::spawn();
    let resp = process.request(
        "execute",
        serde_json::json!({
            "execution_id": "exec-1",
            "config": { "type": "grpc:close", "connection_id": "never-connected" }
        }),
    );
    assert_eq!(resp["result"]["success"], false);
    let serialized = serde_json::to_string(&resp["result"]).unwrap_or_default();
    assert!(
        serialized.contains("does not exist"),
        "close on a missing connection must report it: {serialized}"
    );
    process.shutdown();
}

/// validate: connect without a URL is rejected; a complete connect config
/// passes; call requires connection/service/method.
#[test]
fn validate_rules() {
    let mut process = PluginProcess::spawn();

    let bad = process.request(
        "validate",
        serde_json::json!({ "config": { "type": "grpc:connect", "url": "" } }),
    );
    assert_eq!(bad["result"]["valid"], false);

    let good = process.request(
        "validate",
        serde_json::json!({ "config": { "type": "grpc:connect", "url": "localhost:50051" } }),
    );
    assert_eq!(good["result"]["valid"], true);

    let no_conn = process.request(
        "validate",
        serde_json::json!({
            "config": { "type": "grpc:call", "connection_id": "", "service_name": "s", "method_name": "m" }
        }),
    );
    assert_eq!(no_conn["result"]["valid"], false);

    let full_call = process.request(
        "validate",
        serde_json::json!({
            "config": { "type": "grpc:call", "connection_id": "c1", "service_name": "s", "method_name": "m" }
        }),
    );
    assert_eq!(full_call["result"]["valid"], true);

    process.shutdown();
}

/// Clean shutdown: EOF on stdin exits with code 0.
#[test]
fn clean_shutdown_on_eof() {
    let process = PluginProcess::spawn();
    process.shutdown();
}
