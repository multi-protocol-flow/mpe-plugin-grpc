//! 全生命周期集成测试：spawn `grpc_test_server`（echo Unary / server
//! streaming / bidi + reflection v1 + health）+ spawn `mpe_plugin_grpc`，
//! 走连接 → 发现 → 调用（unary/streaming）→ 实时流消息 → 取消 →
//! channelz/health → close → flow_ended 全链路。
//!
//! 流式消息断言：`grpc.stream` 通知帧（无 id）在 execute 响应前逐条
//! 到达 stdout，call_id 与调用一一对应。

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use serde_json::{json, Value};

/// Path to the compiled binaries (set by cargo for `[[bin]]` targets).
const PLUGIN_BIN: &str = env!("CARGO_BIN_EXE_mpe_plugin_grpc");
const SERVER_BIN: &str = env!("CARGO_BIN_EXE_grpc_test_server");

/// echo.proto 内容（与 src/bin/echo.proto 保持一致；测试内联以便插件侧
/// proto 解析路径使用）。
const ECHO_PROTO: &str = r#"syntax = "proto3";

package echo;

message EchoMessage {
  string text = 1;
  int32 n = 2;
  map<string, string> attrs = 3;
  repeated string tags = 4;
}

service EchoService {
  rpc Unary(EchoMessage) returns (EchoMessage);
  rpc ServerStreaming(EchoMessage) returns (stream EchoMessage);
  rpc ClientStreaming(stream EchoMessage) returns (EchoMessage);
  rpc BidiStreaming(stream EchoMessage) returns (stream EchoMessage);
}
"#;

/// 启动 `grpc_test_server` 并解析 `LISTENING <addr>` 行。
/// 测试结束时显式 kill（Child::drop 不杀进程，避免孤儿进程挂住测试进程）。
struct TestServer {
    child: Child,
    addr: String,
}

impl TestServer {
    fn spawn() -> TestServer {
        let mut child = Command::new(SERVER_BIN)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("failed to spawn grpc_test_server");
        let stdout = child.stdout.take().expect("server stdout unavailable");
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            let mut line = String::new();
            while let Ok(n) = reader.read_line(&mut line) {
                if n == 0 {
                    break;
                }
                let trimmed = line.trim();
                if let Some(addr) = trimmed.strip_prefix("LISTENING ") {
                    let _ = tx.send(addr.to_string());
                    break;
                }
                line.clear();
            }
        });
        let addr = rx
            .recv_timeout(Duration::from_secs(15))
            .expect("grpc_test_server did not report LISTENING within 15s");
        TestServer { child, addr }
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

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

    /// 发送请求并等待对应 id 的响应（跳过通知帧）。
    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.send(method, params);
        self.read_response(id)
    }

    /// 发送请求，返回分配的请求 id（不等待响应）。
    fn send(&mut self, method: &str, params: Value) -> u64 {
        self.next_id += 1;
        let id = self.next_id;
        let frame = serde_json::to_string(&json!({
            "jsonrpc": "2.0", "id": id, "method": method, "params": params,
        }))
        .expect("request serializable");
        let stdin = self.stdin.as_mut().expect("stdin alive");
        writeln!(stdin, "{frame}").expect("request write");
        stdin.flush().expect("request flush");
        id
    }

    /// 读取一帧（阻塞）。
    fn read_frame(&mut self) -> Value {
        let mut line = String::new();
        let n = self.stdout.read_line(&mut line).expect("read frame");
        assert!(n > 0, "plugin stdout closed early");
        serde_json::from_str(&line).expect("frame is valid JSON")
    }

    /// 读到指定 id 的响应为止（期间的通知帧返回给调用方）。
    fn read_response_collecting(&mut self, id: u64) -> (Value, Vec<Value>) {
        let mut notifications = Vec::new();
        loop {
            let frame = self.read_frame();
            if frame.get("id").is_some() {
                assert_eq!(
                    frame["id"].as_u64(),
                    Some(id),
                    "unexpected response id: {}",
                    frame
                );
                return (frame, notifications);
            }
            notifications.push(frame);
        }
    }

    fn read_response(&mut self, id: u64) -> Value {
        let (resp, _) = self.read_response_collecting(id);
        resp
    }

    fn shutdown(mut self) {
        drop(self.stdin.take());
        let status = self.child.wait().expect("plugin exit");
        assert!(status.success(), "plugin should exit cleanly");
    }
}

/// 构造 grpc:connect 执行参数（proto 文件模式）。
fn connect_params(addr: &str, connection_id: &str) -> Value {
    json!({
        "execution_id": "exec-1",
        "node_instance_id": connection_id,
        "config": {
            "type": "grpc:connect",
            "url": addr,
            "use_tls": false,
            "enable_reflection": false,
            "connect_timeout_ms": 10000,
            "proto_files": [ { "path": "echo.proto", "content": ECHO_PROTO } ],
            "default_metadata": [],
        }
    })
}

/// 构造 grpc:call 执行参数；`config_extra` 的键直接合并进 `config` 对象。
fn call_params(call_node: &str, method: &str, request_json: &str, config_extra: Value) -> Value {
    let mut config = json!({
        "type": "grpc:call",
        "connection_id": "node-connect-1",
        "service_name": "echo.EchoService",
        "method_name": method,
        "request_json": request_json,
        "timeout_ms": 30000,
        "metadata": [],
        "request_messages": [],
        "max_retries": 0,
    });
    if let (Some(target), Some(extra_obj)) = (config.as_object_mut(), config_extra.as_object()) {
        for (k, v) in extra_obj {
            target.insert(k.clone(), v.clone());
        }
    }
    json!({
        "execution_id": "exec-1",
        "node_instance_id": call_node,
        "config": config,
    })
}

/// 全生命周期：connect(proto) → unary → server_streaming（实时流消息）→
/// bidi → channelz → health → close → flow_ended。
#[test]
fn full_lifecycle_with_proto_discovery() {
    let server = TestServer::spawn();
    let addr = &server.addr;
    let mut plugin = PluginProcess::spawn();

    // 1. grpc:connect（proto 文件发现）
    let resp = plugin.request("execute", connect_params(addr, "node-connect-1"));
    assert_eq!(resp["result"]["success"], true, "connect must succeed: {resp}");
    let services = &resp["result"]["output_data"]["services"];
    assert!(
        services
            .as_array()
            .expect("services array")
            .iter()
            .any(|s| s["service_name"] == "echo.EchoService"),
        "discovered services must include echo.EchoService: {services}"
    );

    // 2. unary 调用
    let resp = plugin.request(
        "execute",
        call_params("node-call-1", "Unary", r#"{"text":"hello","n":7}"#, json!({})),
    );
    assert_eq!(resp["result"]["success"], true, "unary must succeed: {resp}");
    assert_eq!(resp["result"]["output_data"]["pattern"], "unary");
    assert_eq!(resp["result"]["output_data"]["data"]["text"], "hello");
    assert_eq!(resp["result"]["output_data"]["data"]["n"], 7);

    // 3. server_streaming（n=3 → 3 条实时 grpc.stream 消息 + 3 条响应）
    let id = plugin.send(
        "execute",
        call_params("node-call-2", "ServerStreaming", r#"{"text":"chunk","n":3}"#, json!({})),
    );
    let (resp, notifications) = plugin.read_response_collecting(id);
    assert_eq!(resp["result"]["success"], true, "server streaming must succeed: {resp}");
    let stream_frames: Vec<&Value> = notifications
        .iter()
        .filter(|f| f["method"] == "grpc.stream")
        .collect();
    assert_eq!(stream_frames.len(), 3, "3 stream messages expected, got {stream_frames:?}");
    for frame in &stream_frames {
        assert_eq!(frame["params"]["kind"], "message");
        assert_eq!(frame["params"]["execution_id"], "exec-1");
        assert_eq!(frame["params"]["node_instance_id"], "node-call-2");
        let call_id = frame["params"]["call_id"]
            .as_str()
            .unwrap_or_default();
        assert!(
            call_id.starts_with("node-call-2-"),
            "call_id must carry the node uuid: {}",
            frame["params"]["call_id"]
        );
    }
    let responses = &resp["result"]["output_data"]["responses"];
    assert_eq!(responses.as_array().map(|a| a.len()), Some(3));
    assert_eq!(resp["result"]["output_data"]["received_count"], 3);

    // 4. bidi streaming（2 条请求 → 2 条响应 + 2 条流消息）
    let bidi_config = json!({
        "request_messages": [
            { "content": r#"{"text":"a","n":1}"#, "enabled": true },
            { "content": r#"{"text":"b","n":2}"#, "enabled": true },
        ]
    });
    let id = plugin.send(
        "execute",
        call_params("node-call-3", "BidiStreaming", r#"{}"#, bidi_config),
    );
    let (resp, notifications) = plugin.read_response_collecting(id);
    assert_eq!(resp["result"]["success"], true, "bidi must succeed: {resp}");
    let stream_frames: Vec<&Value> = notifications
        .iter()
        .filter(|f| f["method"] == "grpc.stream")
        .collect();
    assert_eq!(stream_frames.len(), 2, "2 bidi echo messages expected");
    assert_eq!(resp["result"]["output_data"]["sent_count"], 2);
    assert_eq!(resp["result"]["output_data"]["received_count"], 2);

    // 5. channelz 内省（READY、0 活跃调用）
    let resp = plugin.request(
        "uiCall",
        json!({
            "method": "grpc.channelz",
            "params": { "execution_id": "exec-1", "connection_id": "node-connect-1" }
        }),
    );
    assert_eq!(resp["result"]["success"], true, "channelz must succeed: {resp}");
    assert_eq!(resp["result"]["info"]["state"], "READY");
    assert_eq!(resp["result"]["info"]["active_calls"], 0);

    // 6. 健康检查（echo.EchoService → SERVING）
    let resp = plugin.request(
        "uiCall",
        json!({
            "method": "grpc.health",
            "params": { "execution_id": "exec-1", "connection_id": "node-connect-1", "service": "echo.EchoService" }
        }),
    );
    assert_eq!(resp["result"]["status"], "SERVING", "health: {resp}");

    // 7. grpc:close 显式释放
    let resp = plugin.request(
        "execute",
        json!({
            "execution_id": "exec-1",
            "config": { "type": "grpc:close", "connection_id": "node-connect-1" }
        }),
    );
    assert_eq!(resp["result"]["success"], true, "close must succeed: {resp}");

    // 8. close 后 channelz 报连接不存在
    let resp = plugin.request(
        "uiCall",
        json!({
            "method": "grpc.channelz",
            "params": { "execution_id": "exec-1", "connection_id": "node-connect-1" }
        }),
    );
    assert_eq!(resp["result"]["success"], false);
    assert!(
        resp["result"]["error"]
            .as_str()
            .is_some_and(|e| e.contains("does not exist")),
        "channelz after close must report missing connection: {resp}"
    );

    // 9. flowEnded 清池（no-op，不 panic；连接已释放）
    let _ = plugin.send("flowEnded", json!({ "execution_id": "exec-1" }));

    plugin.shutdown();
}

/// 反射模式发现：enable_reflection=true 且无 proto 文件 → 通过 Server
/// Reflection v1 发现 echo.EchoService。
#[test]
fn reflection_discovery() {
    let server = TestServer::spawn();
    let addr = &server.addr;
    let mut plugin = PluginProcess::spawn();

    let resp = plugin.request(
        "execute",
        json!({
            "execution_id": "exec-2",
            "node_instance_id": "node-connect-r1",
            "config": {
                "type": "grpc:connect",
                "url": addr,
                "use_tls": false,
                "enable_reflection": true,
                "connect_timeout_ms": 10000,
                "proto_files": [],
                "default_metadata": [],
            }
        }),
    );
    assert_eq!(resp["result"]["success"], true, "reflection connect: {resp}");
    let services = &resp["result"]["output_data"]["services"];
    assert!(
        services
            .as_array()
            .expect("services array")
            .iter()
            .any(|s| s["service_name"] == "echo.EchoService"),
        "reflection discovery must include echo.EchoService: {services}"
    );

    // 反射连接也可直接调用
    let resp = plugin.request(
        "execute",
        json!({
            "execution_id": "exec-2",
            "node_instance_id": "node-call-r1",
            "config": {
                "type": "grpc:call",
                "connection_id": "node-connect-r1",
                "service_name": "echo.EchoService",
                "method_name": "Unary",
                "request_json": r#"{"text":"via-reflection"}"#,
                "timeout_ms": 30000,
                "metadata": [],
                "request_messages": [],
                "max_retries": 0,
            }
        }),
    );
    assert_eq!(resp["result"]["success"], true, "reflection unary: {resp}");
    assert_eq!(resp["result"]["output_data"]["data"]["text"], "via-reflection");

    plugin.shutdown();
}

/// 流式取消：长流执行中 uiCall grpc.cancelStream → 收集提前停止，且
/// execute 正常返回（非错误）。
#[test]
fn cancel_stream_stops_collection() {
    let server = TestServer::spawn();
    let addr = &server.addr;
    let mut plugin = PluginProcess::spawn();

    let resp = plugin.request("execute", connect_params(addr, "node-connect-1"));
    assert_eq!(resp["result"]["success"], true, "connect: {resp}");

    // 长流（n=100000，每条 20ms → 正常需 33 分钟；30s 超时会先触发）
    let id = plugin.send(
        "execute",
        call_params(
            "node-call-c1",
            "ServerStreaming",
            r#"{"text":"long","n":100000}"#,
            json!({ "timeout_ms": 60000 }),
        ),
    );

    // 等第一条 grpc.stream 通知，拿 call_id
    let mut call_id = None;
    let mut first_notifications = Vec::new();
    for _ in 0..200 {
        let frame = plugin.read_frame();
        if frame["method"] == "grpc.stream" {
            call_id = frame["params"]["call_id"].as_str().map(str::to_string);
            first_notifications.push(frame);
            if call_id.is_some() {
                break;
            }
        }
    }
    let call_id = call_id.expect("expected at least one grpc.stream notification");

    // 定向取消
    let resp = plugin.request(
        "uiCall",
        json!({
            "method": "grpc.cancelStream",
            "params": {
                "execution_id": "exec-1",
                "connection_id": "node-connect-1",
                "call_id": call_id,
            }
        }),
    );
    assert_eq!(resp["result"], json!({}), "cancelStream must return {{}}: {resp}");

    // execute 响应应较快返回（取消触发收集循环 break）
    let (resp, remaining) = plugin.read_response_collecting(id);
    assert_eq!(resp["result"]["success"], true, "cancelled execute must still succeed: {resp}");
    let total_stream_frames = first_notifications
        .iter()
        .filter(|f| f["method"] == "grpc.stream")
        .count()
        + remaining
            .iter()
            .filter(|f| f["method"] == "grpc.stream")
            .count();
    assert!(
        total_stream_frames < 1000,
        "cancel must stop collection early, got {} frames",
        total_stream_frames
    );

    // channelz 活跃调用归零
    let resp = plugin.request(
        "uiCall",
        json!({
            "method": "grpc.channelz",
            "params": { "execution_id": "exec-1", "connection_id": "node-connect-1" }
        }),
    );
    assert_eq!(resp["result"]["info"]["active_calls"], 0, "active calls must drain: {resp}");

    plugin.shutdown();
}

/// 骨架生成（proto 路径）：grpc.skeleton 返回可解析的 JSON。
#[test]
fn skeleton_from_proto() {
    let mut plugin = PluginProcess::spawn();

    let resp = plugin.request(
        "uiCall",
        json!({
            "method": "grpc.skeleton",
            "params": {
                "proto_files": [ { "path": "echo.proto", "content": ECHO_PROTO } ],
                "service_name": "echo.EchoService",
                "method_name": "Unary",
            }
        }),
    );
    assert_eq!(resp["result"]["success"], true, "skeleton: {resp}");
    let skeleton: Value = serde_json::from_str(
        resp["result"]["skeleton"]
            .as_str()
            .expect("skeleton must be a string"),
    )
    .expect("skeleton must be valid JSON");
    assert_eq!(skeleton["text"], "");
    assert_eq!(skeleton["n"], 0);

    plugin.shutdown();
}
