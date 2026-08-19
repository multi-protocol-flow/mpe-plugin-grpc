//! gRPC sidecar plugin for MPE.
//!
//! Third-party plugin crate: depends only on `mpe-plugin-sdk` (which in turn
//! bundles the shared wire contract). Never imports host types
//! (`flow-engine-core`, `flow-engine-plugin`, ...) — the plugin is a
//! separate process speaking JSON-RPC 2.0 over stdio.
//!
//! Node types: `grpc:connect` / `grpc:call` / `grpc:close`. `execute`
//! dispatches by the `config["type"]` discriminator (the host injects the
//! node's `type` field into the resolved config). Connection selection: the
//! pool keys connections by `(execution_id, connection_id)` — the connect
//! node registers under its own instance id (`ctx.node_instance_id()`),
//! call/close nodes pick their connection via the `connection_id` field the
//! host resolves from the schema's `x-node-selector`.
//!
//! 流式链路：call 执行中每条响应经 `ctx.emit("grpc.stream", …)`
//! （backpressure 通知，自动带 correlation）推给宿主 →
//! `DebugEvent::GrpcStreamMessage/Error` → 现有 debug store
//! （grpcStreamMessages 按 call_id 累积）→ 报告 viewer iframe 实时追加。
//!
//! Config panel + report viewer 均为插件自带前端（iframe + postMessage
//! 桥）：describe 的 `frontend`（config-panel 页）与 `viewer`（报告页）
//! 各自嵌入 inline HTML（`include_str!`，构建产物 frontend/dist/）。

use std::future::Future;

use mpe_plugin_sdk::prelude::*;

mod codec;
mod error_detail;
mod grpc_call_executor;
mod grpc_close_executor;
mod grpc_connect_executor;
mod helpers;
mod i18n;
mod pool;
mod proto_parser;
mod reflection;
mod tls;
mod types;
mod ui;
use pool::GrpcPool;

/// Process-level plugin singleton: node registry + per-execution pool.
#[derive(Default)]
pub struct GrpcPlugin {
    pub pool: GrpcPool,
}

/// 操作节点（connect/call/close）的双输出端口：`in`(输入) + `true`(成功)
/// + `false`(失败)。失败不在此路由：节点返回失败后宿主按 `on_error` 策略
/// （默认 RouteToFalse）自动走 `false` 端口。
fn operation_ports() -> Vec<PortDescription> {
    vec![
        PortDescription::new("in", i18n::t("输入", "Input"), PORT_KIND_IN),
        PortDescription::new("true", i18n::t("成功", "Success"), PORT_KIND_OUT),
        PortDescription::new("false", i18n::t("失败", "Failure"), PORT_KIND_OUT),
    ]
}

/// `grpc:call` / `grpc:close` 节点 config_schema 里的 `connection_id`
/// 字段：宿主 `x-node-selector` 渲染为对流程内 `grpc:connect` 节点的
/// 选择器，选中后把 connect 节点的 instance id 注入 config。
fn connection_id_property() -> serde_json::Value {
    serde_json::json!({
        "type": "string",
        "title": i18n::t("gRPC 连接", "gRPC Connection"),
        "x-node-selector": { "node_type": "grpc:connect" },
    })
}

/// 构建一个 gRPC 节点描述（config-panel 页 + 报告 viewer 页均为 inline
/// 前端，由宿主归一化时生成各自的 iframe URL）。
fn grpc_node(
    type_id: &str,
    display_name: &str,
    icon: &str,
    default_config: serde_json::Value,
    properties: serde_json::Value,
    required: &[&str],
    capabilities: Option<PluginCapabilities>,
) -> NodeDescription {
    let mut node = NodeDescription::new(type_id, display_name);
    node.category = Some("grpc".to_string());
    node.icon = Some(icon.to_string());
    node.color = Some("#8B5CF6".to_string());
    node.ports = operation_ports();
    node.default_config = default_config;
    node.capabilities = capabilities.unwrap_or_default();
    node.config_schema = Some(serde_json::json!({
        "type": "object",
        "properties": properties,
        "required": required,
    }));
    node.frontend = Some(FrontendDescription {
        kind: "inline".into(),
        content: Some(include_str!("../frontend/dist/panel.html").to_string()),
        url: None, // 宿主归一化时生成 iframe URL
    });
    node.viewer = Some(FrontendDescription {
        kind: "inline".into(),
        content: Some(include_str!("../frontend/dist/viewer.html").to_string()),
        url: None, // 宿主归一化时生成 /viewer URL
    });
    node
}

impl Plugin for GrpcPlugin {
    fn describe(&self) -> Vec<NodeDescription> {
        vec![
            // `grpc:connect` — 建立 gRPC 连接（池按 (execution_id, 节点
            // 实例) 复用）。声明 single_node capability：宿主单节点路径
            // （execute_single_node / mpe run node）据此放行连接性验证。
            grpc_node(
                "grpc:connect",
                "gRPC Connect",
                "plug",
                serde_json::json!({
                    "url": "",
                    "use_tls": false,
                    "tls_skip_verify": false,
                    "enable_reflection": false,
                    "proto_files": [],
                    "default_metadata": [],
                    "connect_timeout_ms": 30000,
                    "discovered_services": [],
                    "tls_ca_cert": null,
                    "tls_client_cert": null,
                    "tls_client_key": null,
                    "tls_server_name_override": null,
                    "compression_encoding": null,
                    "keepalive_time_ms": null,
                    "keepalive_timeout_ms": null,
                    "keepalive_permit_without_streams": null,
                    "reflection_metadata": null,
                    "health_check_service": null,
                    "endpoints": null,
                    "max_retries": null,
                    "initial_backoff_ms": null,
                    "max_backoff_ms": null,
                    "retryable_status_codes": null,
                }),
                serde_json::json!({
                    "url": { "type": "string", "description": i18n::t("服务器地址（host:port 或 URL）", "Server address (host:port or URL)") },
                    "use_tls": { "type": "boolean", "description": i18n::t("启用 TLS", "Enable TLS") },
                    "tls_skip_verify": { "type": "boolean", "description": i18n::t("跳过证书验证（仅开发）", "Skip certificate verification (dev only)") },
                    "enable_reflection": { "type": "boolean", "description": i18n::t("Server Reflection 服务发现", "Discover via Server Reflection") },
                    "proto_files": { "type": "array", "items": { "type": "object" }, "description": i18n::t("Proto 文件", "Proto files") },
                    "default_metadata": { "type": "array", "items": { "type": "object" }, "description": i18n::t("默认 Metadata", "Default metadata") },
                    "connect_timeout_ms": { "type": "number", "description": i18n::t("连接超时（毫秒）", "Connect timeout (ms)") },
                    "discovered_services": { "type": "array", "items": { "type": "object" }, "description": i18n::t("发现的服务（设计时缓存）", "Discovered services (design-time cache)") },
                    "tls_ca_cert": { "type": "string", "description": i18n::t("自定义 CA 证书", "Custom CA cert") },
                    "tls_client_cert": { "type": "string", "description": i18n::t("客户端证书（mTLS）", "Client cert (mTLS)") },
                    "tls_client_key": { "type": "string", "description": i18n::t("客户端私钥（mTLS）", "Client key (mTLS)") },
                    "tls_server_name_override": { "type": "string", "description": i18n::t("TLS SNI 覆盖", "TLS SNI override") },
                    "compression_encoding": { "type": "string", "description": i18n::t("压缩编码（gzip/none）", "Compression (gzip/none)") },
                    "keepalive_time_ms": { "type": "number", "description": i18n::t("Keepalive 间隔（毫秒）", "Keepalive interval (ms)") },
                    "keepalive_timeout_ms": { "type": "number", "description": i18n::t("Keepalive 超时（毫秒）", "Keepalive timeout (ms)") },
                    "keepalive_permit_without_streams": { "type": "boolean", "description": i18n::t("无流时允许 PING", "Keepalive while idle") },
                    "reflection_metadata": { "type": "array", "items": { "type": "object" }, "description": i18n::t("反射认证 Metadata", "Reflection auth metadata") },
                    "endpoints": { "type": "array", "items": { "type": "string" }, "description": i18n::t("额外端点（负载均衡）", "Extra endpoints (load balancing)") },
                    "max_retries": { "type": "number", "description": i18n::t("最大重试次数", "Max retries") },
                    "initial_backoff_ms": { "type": "number", "description": i18n::t("初始退避（毫秒）", "Initial backoff (ms)") },
                }),
                &["url"],
                Some(PluginCapabilities {
                    single_node: true,
                    ..Default::default()
                }),
            ),
            // `grpc:call` — 调用 gRPC 方法（Unary / Server / Client / Bidi）。
            grpc_node(
                "grpc:call",
                "gRPC Call",
                "zap",
                serde_json::json!({
                    "connection_id": "",
                    "service_name": "",
                    "method_name": "",
                    "request_json": "",
                    "timeout_ms": 30000,
                    "metadata": [],
                    "request_messages": [],
                    "compression_encoding": null,
                    "max_retries": null,
                    "initial_backoff_ms": null,
                }),
                serde_json::json!({
                    "connection_id": connection_id_property(),
                    "service_name": { "type": "string", "description": i18n::t("服务全名", "Service full name") },
                    "method_name": { "type": "string", "description": i18n::t("方法名", "Method name") },
                    "request_json": { "type": "string", "description": i18n::t("请求 JSON", "Request JSON") },
                    "timeout_ms": { "type": "number", "description": i18n::t("调用超时（毫秒）", "Call timeout (ms)") },
                    "metadata": { "type": "array", "items": { "type": "object" }, "description": i18n::t("调用级 Metadata", "Call metadata") },
                    "request_messages": { "type": "array", "items": { "type": "object" }, "description": i18n::t("流式请求消息", "Streaming request messages") },
                    "compression_encoding": { "type": "string", "description": i18n::t("压缩覆盖（gzip/none）", "Compression override (gzip/none)") },
                    "max_retries": { "type": "number", "description": i18n::t("重试次数（覆盖连接级）", "Retries (overrides connection)") },
                    "initial_backoff_ms": { "type": "number", "description": i18n::t("初始退避（毫秒）", "Initial backoff (ms)") },
                }),
                &["connection_id", "service_name", "method_name"],
                None,
            ),
            // `grpc:close` — 显式释放单条连接。
            grpc_node(
                "grpc:close",
                "gRPC Close",
                "x",
                serde_json::json!({
                    "connection_id": "",
                }),
                serde_json::json!({
                    "connection_id": connection_id_property(),
                }),
                &["connection_id"],
                None,
            ),
        ]
    }

    fn validate(&self, config: &serde_json::Value) -> Result<(), String> {
        let node_type = config
            .get("type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        match node_type {
            "grpc:connect" => {
                let url = config
                    .get("url")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("");
                if url.trim().is_empty() {
                    return Err(i18n::t("请填写服务器地址", "server URL is required").to_string());
                }
                Ok(())
            }
            "grpc:call" => {
                if config
                    .get("connection_id")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .trim()
                    .is_empty()
                {
                    return Err(i18n::t(
                        "请选择 grpc:connect 连接节点",
                        "Please select a grpc:connect connection node",
                    )
                    .to_string());
                }
                if config
                    .get("service_name")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .trim()
                    .is_empty()
                {
                    return Err(i18n::t(
                        "请填写服务全名",
                        "service name is required",
                    )
                    .to_string());
                }
                if config
                    .get("method_name")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .trim()
                    .is_empty()
                {
                    return Err(i18n::t("请填写方法名", "method name is required").to_string());
                }
                Ok(())
            }
            "grpc:close" => {
                if config
                    .get("connection_id")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .trim()
                    .is_empty()
                {
                    return Err(i18n::t(
                        "请选择 grpc:connect 连接节点",
                        "Please select a grpc:connect connection node",
                    )
                    .to_string());
                }
                Ok(())
            }
            _ => Err(format!(
                "{}: `{node_type}`",
                i18n::t("未知节点类型", "Unknown node type")
            )),
        }
    }

    // The runtime spawns execute futures, so the future must be Send; an
    // `async fn` cannot express that bound on stable (see `Plugin`).
    #[allow(clippy::manual_async_fn)]
    fn execute(&self, ctx: &mut ExecuteContext) -> impl Future<Output = ExecuteResult> + Send {
        let pool = self.pool.clone();
        async move {
            let node_type = ctx
                .config()
                .get("type")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            match node_type {
                "grpc:connect" => grpc_connect_executor::execute(ctx, &pool).await,
                "grpc:call" => grpc_call_executor::execute(ctx, &pool).await,
                "grpc:close" => grpc_close_executor::execute(ctx, &pool).await,
                other => ExecuteResult::fail(
                    i18n::t("未知节点类型", "Unknown node type").to_string()
                        + &format!(" `{other}`"),
                ),
            }
        }
    }

    // SDK `cancel(request_id)` 钩子（宿主在 execute 超时时触发）：透传到
    // 该 request 对应的 in-flight 流式收集循环的取消令牌。
    fn cancel(&self, request_id: u64) {
        self.pool.cancel_request(request_id);
    }

    // Flow-completion hook — release path 1：宿主在流程执行结束
    // （成功/失败/取消）时广播 flowEnded，此处清空该 execution 的全部
    // 连接。`grpc:close` 节点是释放路径 2；两者可都触发，close_all 对
    // 缺失条目 no-op，双释放无害。
    fn flow_ended(&self, execution_id: &str) {
        self.pool.close_all(execution_id);
    }

    // Config-panel 设计时查询 relayed by the host (`plugin_ui_call`)。
    #[allow(clippy::manual_async_fn)]
    fn ui_call(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> impl Future<Output = Result<serde_json::Value, String>> + Send {
        let owned_method = method.to_string();
        let pool = self.pool.clone();
        async move { ui::dispatch(&owned_method, params, &pool).await }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// 面板/查看器 inline `<script>` 结构平衡检查（mcp 插件同款）：括号
    /// 必须配对（字符串与注释外）。脚本解析失败 = iframe 永不发 `ready`。
    fn script_balanced(html: &str) -> bool {
        let Some(start) = html.find("<script>") else {
            return false;
        };
        let start = start + "<script>".len();
        let Some(rest) = html.get(start..) else {
            return false;
        };
        let Some(end) = rest.find("</script>") else {
            return false;
        };
        let js = &rest[..end];

        let mut stack: Vec<char> = Vec::new();
        let mut chars = js.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '/' if chars.peek() == Some(&'/') => {
                    for c in chars.by_ref() {
                        if c == '\n' {
                            break;
                        }
                    }
                }
                '/' if chars.peek() == Some(&'*') => {
                    chars.next();
                    let mut prev = '\0';
                    for c in chars.by_ref() {
                        if prev == '*' && c == '/' {
                            break;
                        }
                        prev = c;
                    }
                }
                '\'' | '"' => {
                    let quote = c;
                    loop {
                        match chars.next() {
                            None => break,
                            Some('\\') => {
                                let _ = chars.next();
                            }
                            Some(c) if c == quote => break,
                            Some(_) => {}
                        }
                    }
                }
                '(' | '[' | '{' => stack.push(c),
                ')' | ']' | '}' => {
                    let Some(open) = stack.pop() else {
                        return false;
                    };
                    let pairs = [('(', ')'), ('[', ']'), ('{', '}')];
                    if !pairs.iter().any(|&(o, cl)| o == open && cl == c) {
                        return false;
                    }
                }
                _ => {}
            }
        }
        stack.is_empty()
    }

    /// 每个节点必须 GUI 可用：category/icon/ports/config_schema/
    /// default_config（无 type 键）、inline frontend + viewer 非空。
    #[test]
    fn describe_metadata_shapes() {
        let nodes = GrpcPlugin::default().describe();
        assert_eq!(nodes.len(), 3, "all 3 grpc nodes must be described");

        let by_type: HashMap<&str, &NodeDescription> = nodes
            .iter()
            .map(|node| (node.type_id.as_str(), node))
            .collect();
        assert_eq!(by_type.len(), nodes.len(), "type_ids must be unique");

        for node in &nodes {
            assert_eq!(node.category.as_deref(), Some("grpc"));
            assert!(node.icon.is_some(), "{} must declare an icon", node.type_id);
            assert_eq!(node.color.as_deref(), Some("#8B5CF6"));
            assert!(
                node.default_config.get("type").is_none(),
                "{} default_config must not contain a `type` key",
                node.type_id
            );
            assert!(node.config_schema.is_some(), "{} must declare config_schema", node.type_id);
            assert_eq!(node.ports.len(), 3, "{} must declare in/true/false ports", node.type_id);
            let frontend = node
                .frontend
                .as_ref()
                .unwrap_or_else(|| panic!("{} must declare a frontend", node.type_id));
            assert_eq!(frontend.kind, "inline");
            assert!(
                frontend.content.as_ref().is_some_and(|c| c.len() >= 50),
                "{} inline panel content must be non-trivial",
                node.type_id
            );
            let viewer = node
                .viewer
                .as_ref()
                .unwrap_or_else(|| panic!("{} must declare a viewer", node.type_id));
            assert_eq!(viewer.kind, "inline");
            assert!(
                viewer.content.as_ref().is_some_and(|c| c.len() >= 50),
                "{} inline viewer content must be non-trivial",
                node.type_id
            );
            assert!(frontend.url.is_none(), "host generates the config url");
            assert!(viewer.url.is_none(), "host generates the viewer url");
        }
    }

    /// 面板与查看器脚本必须带宿主桥（监听 init、post ready）。
    #[test]
    fn panel_and_viewer_scripts_have_bridge_and_balanced_js() {
        let nodes = GrpcPlugin::default().describe();
        for node in &nodes {
            let frontend = node.frontend.as_ref().expect("frontend");
            assert!(
                script_balanced(frontend.content.as_deref().unwrap_or("")),
                "{} panel <script> must be balanced JS",
                node.type_id
            );
            assert!(
                frontend
                    .content
                    .as_deref()
                    .unwrap_or("")
                    .contains("post(\"ready\"")
                    || frontend
                        .content
                        .as_deref()
                        .unwrap_or("")
                        .contains("post('ready'"),
                "{} panel must post the `ready` bridge message",
                node.type_id
            );
            let viewer = node.viewer.as_ref().expect("viewer");
            assert!(
                script_balanced(viewer.content.as_deref().unwrap_or("")),
                "{} viewer <script> must be balanced JS",
                node.type_id
            );
        }
    }

    /// `grpc:connect` 声明 single_node capability；call/close 不声明。
    #[test]
    fn connect_declares_single_node_capability() {
        let nodes = GrpcPlugin::default().describe();
        let connect = nodes.iter().find(|n| n.type_id == "grpc:connect").expect("grpc:connect");
        assert_eq!(connect.capabilities.single_node, true);
        for other in ["grpc:call", "grpc:close"] {
            let node = nodes.iter().find(|n| n.type_id == other).expect("node");
            assert_ne!(node.capabilities.single_node, true, "{other} must NOT declare single_node");
        }
    }

    /// call/close 的 config_schema 暴露指向 grpc:connect 的 x-node-selector。
    #[test]
    fn operation_schema_has_connection_selector() {
        let nodes = GrpcPlugin::default().describe();
        for node in &nodes {
            if node.type_id == "grpc:connect" {
                continue;
            }
            let schema = node.config_schema.as_ref().expect("schema");
            assert_eq!(
                schema["properties"]["connection_id"]["x-node-selector"]["node_type"],
                "grpc:connect",
                "{} must expose an x-node-selector over grpc:connect",
                node.type_id
            );
        }
    }

    /// validate 规则。
    #[test]
    fn validate_rules() {
        let plugin = GrpcPlugin::default();
        assert!(plugin.validate(&serde_json::json!({ "type": "grpc:connect", "url": "" })).is_err());
        assert!(plugin.validate(&serde_json::json!({ "type": "grpc:connect", "url": "localhost:50051" })).is_ok());
        assert!(plugin.validate(&serde_json::json!({ "type": "grpc:call", "connection_id": "c1", "service_name": "s", "method_name": "m" })).is_ok());
        assert!(plugin.validate(&serde_json::json!({ "type": "grpc:call", "connection_id": "", "service_name": "s", "method_name": "m" })).is_err());
        assert!(plugin.validate(&serde_json::json!({ "type": "grpc:call", "connection_id": "c1", "service_name": "", "method_name": "m" })).is_err());
        assert!(plugin.validate(&serde_json::json!({ "type": "grpc:call", "connection_id": "c1", "service_name": "s", "method_name": "" })).is_err());
        assert!(plugin.validate(&serde_json::json!({ "type": "grpc:close", "connection_id": "c1" })).is_ok());
        assert!(plugin.validate(&serde_json::json!({ "type": "grpc:close", "connection_id": "" })).is_err());
        assert!(plugin.validate(&serde_json::json!({ "type": "grpc:nope" })).is_err());
    }
}
