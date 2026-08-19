//! gRPC 连接池（插件进程内）
//!
//! 从宿主 `flow-engine-grpc::grpc_connection_manager` 迁移，键改为
//! `(execution_id, connection_id)` 复合键（对齐 mcp 插件 pool.rs）：
//! - `grpc:connect` 节点以自身 instance id 注册；
//! - `grpc:call` / `grpc:close` 节点通过 config 里的 `connection_id`
//!   （宿主 `x-node-selector` 注入的 connect 节点 instance id）复用；
//! - `flow_ended` 清空该 execution 全部连接；`grpc:close` 显式释放单条。
//!
//! 流式消息通过 `ctx.emit("grpc.stream", …)` 实时推送（backpressure
//! 通知，见 SDK v0.2.0），宿主侧转 `DebugEvent::GrpcStreamMessage/Error`
//! 进入现有 debug store 数据路径。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

use mpe_plugin_sdk::context::ExecuteContext;
use prost::Message;
use serde_json::json;
use tokio_util::sync::CancellationToken;
use tonic::metadata::{MetadataKey, MetadataValue};
use tower::ServiceExt;

use crate::codec::DynamicCodec;
use crate::error_detail::format_grpc_error;
use crate::helpers::{
    build_method_path, dynamic_message_to_json, json_to_dynamic_message, merge_metadata,
};
use crate::proto_parser;
use crate::reflection::reflect_services;
use crate::tls::{create_balanced_channel, create_channel};
use crate::types::{GrpcMethodInfo, GrpcServiceInfo, GrpcStreamingResult, GrpcTimingInfo};

/// `grpc:connect` 节点配置（宿主变量解析后的最终值）。
#[derive(Debug, Clone, Default)]
pub struct ConnectConfig {
    pub url: String,
    pub use_tls: bool,
    pub tls_skip_verify: bool,
    pub enable_reflection: bool,
    pub proto_files: Vec<(String, String)>,
    pub default_metadata: Vec<(String, String)>,
    pub connect_timeout_ms: u64,
    pub tls_ca_cert: Option<String>,
    pub tls_client_cert: Option<String>,
    pub tls_client_key: Option<String>,
    pub tls_server_name_override: Option<String>,
    pub reflection_metadata: Vec<(String, String)>,
    pub compression_encoding: Option<String>,
    pub keepalive_time_ms: Option<u64>,
    pub keepalive_timeout_ms: Option<u64>,
    pub keepalive_permit_without_streams: Option<bool>,
    pub endpoints: Option<Vec<String>>,
    /// 连接级重试默认值（call 级可覆盖）
    pub max_retries: Option<u32>,
    pub initial_backoff_ms: Option<u64>,
}

/// 从 execute/connect 的 `config` JSON 解析 `ConnectConfig`。
impl ConnectConfig {
    pub fn from_value(config: &serde_json::Value) -> Result<Self, String> {
        let get_str = |key: &str| {
            config
                .get(key)
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_string()
        };
        let get_bool = |key: &str| {
            config
                .get(key)
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false)
        };
        let get_u64 = |key: &str, default: u64| {
            config
                .get(key)
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(default)
        };
        let get_opt_u64 = |key: &str| {
            config
                .get(key)
                .and_then(serde_json::Value::as_u64)
        };
        let get_opt_bool = |key: &str| {
            config
                .get(key)
                .and_then(serde_json::Value::as_bool)
        };
        let get_opt_str = |key: &str| {
            config
                .get(key)
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        };

        let proto_files: Vec<(String, String)> = config
            .get("proto_files")
            .and_then(serde_json::Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|pf| {
                        let path = pf.get("path")?.as_str()?.to_string();
                        let content = pf.get("content")?.as_str()?.to_string();
                        Some((path, content))
                    })
                    .collect()
            })
            .unwrap_or_default();

        let metadata = metadata_from_value(config.get("default_metadata"));
        let reflection_metadata = metadata_from_value(config.get("reflection_metadata"));

        let endpoints = config
            .get("endpoints")
            .and_then(serde_json::Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(serde_json::Value::as_str)
                    .map(str::to_string)
                    .collect()
            });

        Ok(Self {
            url: get_str("url"),
            use_tls: get_bool("use_tls"),
            tls_skip_verify: get_bool("tls_skip_verify"),
            enable_reflection: get_bool("enable_reflection"),
            proto_files,
            default_metadata: metadata,
            connect_timeout_ms: get_u64("connect_timeout_ms", 30_000),
            tls_ca_cert: get_opt_str("tls_ca_cert"),
            tls_client_cert: get_opt_str("tls_client_cert"),
            tls_client_key: get_opt_str("tls_client_key"),
            tls_server_name_override: get_opt_str("tls_server_name_override"),
            reflection_metadata,
            compression_encoding: get_opt_str("compression_encoding"),
            keepalive_time_ms: get_opt_u64("keepalive_time_ms"),
            keepalive_timeout_ms: get_opt_u64("keepalive_timeout_ms"),
            keepalive_permit_without_streams: get_opt_bool("keepalive_permit_without_streams"),
            endpoints,
            max_retries: config
                .get("max_retries")
                .and_then(serde_json::Value::as_u64)
                .map(|v| v as u32),
            initial_backoff_ms: get_opt_u64("initial_backoff_ms"),
        })
    }
}

/// `grpc:call` 节点配置。
#[derive(Debug, Clone, Default)]
pub struct CallConfig {
    pub connection_id: String,
    pub service_name: String,
    pub method_name: String,
    pub request_json: String,
    pub timeout_ms: u64,
    pub metadata: Vec<(String, String)>,
    pub request_messages: Vec<(String, bool)>, // (content, enabled)
    pub compression_encoding: Option<String>,
    pub max_retries: Option<u32>,
    pub initial_backoff_ms: Option<u64>,
}

impl CallConfig {
    pub fn from_value(config: &serde_json::Value) -> Result<Self, String> {
        let get_str = |key: &str| {
            config
                .get(key)
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_string()
        };
        let get_opt_u64 = |key: &str| {
            config
                .get(key)
                .and_then(serde_json::Value::as_u64)
        };

        let request_messages: Vec<(String, bool)> = config
            .get("request_messages")
            .and_then(serde_json::Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| {
                        let content = m.get("content")?.as_str()?.to_string();
                        let enabled = m.get("enabled").and_then(serde_json::Value::as_bool).unwrap_or(true);
                        Some((content, enabled))
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(Self {
            connection_id: get_str("connection_id"),
            service_name: get_str("service_name"),
            method_name: get_str("method_name"),
            request_json: get_str("request_json"),
            timeout_ms: get_opt_u64("timeout_ms").unwrap_or(30_000),
            metadata: metadata_from_value(config.get("metadata")),
            request_messages,
            compression_encoding: config
                .get("compression_encoding")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
            max_retries: config
                .get("max_retries")
                .and_then(serde_json::Value::as_u64)
                .map(|v| v as u32),
            initial_backoff_ms: get_opt_u64("initial_backoff_ms"),
        })
    }
}

/// 解析 metadata 数组 `[{key, value}]` 为 `(key, value)` 列表。
fn metadata_from_value(value: Option<&serde_json::Value>) -> Vec<(String, String)> {
    value
        .and_then(serde_json::Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|m| {
                    let key = m.get("key")?.as_str()?.to_string();
                    let value = m.get("value")?.as_str()?.to_string();
                    Some((key, value))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// 从 tonic `MetadataMap` 提取 ASCII 键值对（跳过 binary headers）。
fn extract_metadata(metadata: &tonic::metadata::MetadataMap) -> Vec<(String, String)> {
    let mut result = Vec::new();
    for kv in metadata.iter() {
        match kv {
            tonic::metadata::KeyAndValueRef::Ascii(key, value) => match value.to_str() {
                Ok(v) => result.push((key.to_string(), v.to_string())),
                Err(_) => result.push((
                    key.to_string(),
                    format!("<non-utf8: {} bytes>", value.len()),
                )),
            },
            tonic::metadata::KeyAndValueRef::Binary(_, _) => {}
        }
    }
    result
}

/// 解析压缩编码字符串为 tonic `CompressionEncoding`（仅支持 "gzip"）。
fn parse_compression(encoding: &str) -> Option<tonic::codec::CompressionEncoding> {
    match encoding.to_lowercase().as_str() {
        "gzip" => Some(tonic::codec::CompressionEncoding::Gzip),
        _ => None,
    }
}

/// 解析调用级压缩配置，覆盖连接级默认值。
fn resolve_compression(
    call_level: Option<&str>,
    connect_level: Option<&str>,
) -> Option<tonic::codec::CompressionEncoding> {
    match call_level {
        Some("none") => None,
        Some(enc) => parse_compression(enc),
        None => connect_level.and_then(parse_compression),
    }
}

/// Channelz 内省信息（轻量级，非完整 Channelz 协议）
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ChannelzInfo {
    pub state: String,
    pub connected_at: String,
    pub uptime_secs: u64,
    pub active_calls: usize,
    pub last_error: Option<String>,
}

/// 活跃的 gRPC 连接
struct GrpcConnection {
    channel: tonic::transport::Channel,
    descriptor_pool: Arc<prost_reflect::DescriptorPool>,
    #[allow(dead_code)]
    status: String,
    #[allow(dead_code)]
    url: String,
    created_at: chrono::DateTime<chrono::Utc>,
    default_metadata: Vec<(String, String)>,
    compression_encoding: Option<String>,
    active_calls: Arc<Mutex<HashMap<String, CancellationToken>>>,
    last_error: Option<String>,
    /// 连接级重试默认值（call 级未配置时使用）
    max_retries: u32,
    initial_backoff_ms: u64,
}

/// gRPC 连接池（进程级单例，由 `GrpcPlugin` 持有）
#[derive(Default, Clone)]
pub struct GrpcPool {
    connections: Arc<RwLock<HashMap<(String, String), GrpcConnection>>>,
    /// request_id（SDK `cancel` 钩子）→ 进行中 execute 的取消令牌。
    /// 流式调用同时注册到 active_calls（按 call_id，uiCall grpc.cancelStream）
    /// 与本表（按 request_id，SDK cancel 钩子），任一取消即触发。
    in_flight: Arc<Mutex<HashMap<u64, CancellationToken>>>,
}

impl GrpcPool {
    pub fn new() -> Self {
        Self {
            connections: Arc::new(RwLock::new(HashMap::new())),
            in_flight: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// SDK `cancel(request_id)` 钩子入口：取消对应 in-flight execute 的
    /// 流式收集循环（该 execute 可能已结束——缺失条目则 no-op）。
    pub fn cancel_request(&self, request_id: u64) {
        let token = self
            .in_flight
            .lock()
            .map(|calls| calls.get(&request_id).cloned())
            .ok()
            .flatten();
        if let Some(token) = token {
            log::info!("[gRPC] SDK cancel hook: cancelling request_id={}", request_id);
            token.cancel();
        }
    }

    /// 注册 request-level 取消令牌（request_id 已知时）。
    fn register_in_flight(&self, request_id: Option<u64>, token: &CancellationToken) {
        if let Some(id) = request_id {
            if let Ok(mut calls) = self.in_flight.lock() {
                calls.insert(id, token.clone());
            }
        }
    }

    /// 注销 request-level 取消令牌。
    fn unregister_in_flight(&self, request_id: Option<u64>) {
        if let Some(id) = request_id {
            if let Ok(mut calls) = self.in_flight.lock() {
                calls.remove(&id);
            }
        }
    }

    /// 建立 gRPC 连接并存入池（键 = (execution_id, connection_id)）。
    pub async fn connect(
        &self,
        execution_id: &str,
        connection_id: &str,
        cfg: &ConnectConfig,
    ) -> Result<Vec<GrpcServiceInfo>, String> {
        // 1. 构造 URL（确保包含 scheme）
        let url_with_scheme = if cfg.url.starts_with("http://") || cfg.url.starts_with("https://") {
            cfg.url.clone()
        } else if cfg.use_tls {
            format!("https://{}", cfg.url)
        } else {
            format!("http://{}", cfg.url)
        };

        // 2. 创建 channel（含 TLS + Keepalive 配置 + 负载均衡）
        let additional: Vec<String> = cfg
            .endpoints
            .as_ref()
            .map(|eps| eps.iter().filter(|ep| !ep.trim().is_empty()).cloned().collect())
            .unwrap_or_default();

        let channel = if additional.is_empty() {
            create_channel(
                &url_with_scheme,
                cfg.use_tls,
                cfg.tls_skip_verify,
                cfg.connect_timeout_ms,
                cfg.tls_ca_cert.as_deref(),
                cfg.tls_client_cert.as_deref(),
                cfg.tls_client_key.as_deref(),
                cfg.tls_server_name_override.as_deref(),
                cfg.keepalive_time_ms,
                cfg.keepalive_timeout_ms,
                cfg.keepalive_permit_without_streams,
            )?
        } else {
            create_balanced_channel(
                &url_with_scheme,
                &additional,
                cfg.use_tls,
                cfg.tls_skip_verify,
                cfg.connect_timeout_ms,
                cfg.tls_ca_cert.as_deref(),
                cfg.tls_client_cert.as_deref(),
                cfg.tls_client_key.as_deref(),
                cfg.tls_server_name_override.as_deref(),
                cfg.keepalive_time_ms,
                cfg.keepalive_timeout_ms,
                cfg.keepalive_permit_without_streams,
            )?
        };

        // 3. 获取 DescriptorPool（反射或解析 proto 文件）
        let pool = if cfg.enable_reflection && cfg.proto_files.is_empty() {
            reflect_services(channel.clone(), cfg.reflection_metadata.clone()).await?
        } else if cfg.proto_files.is_empty() {
            return Err(
                "no proto files provided and Server Reflection not enabled, cannot discover services"
                    .to_string(),
            );
        } else {
            let files: Vec<proto_parser::ProtoFile> = cfg
                .proto_files
                .iter()
                .map(|(path, content)| proto_parser::ProtoFile {
                    path: path.clone(),
                    content: content.clone(),
                })
                .collect();
            proto_parser::parse_proto_files(&files)
                .map_err(|e| format!("failed to parse proto files: {}", e))?
        };

        // 4. 提取服务信息
        let services = proto_parser::list_services(&pool);
        let message_definitions = Arc::new(proto_parser::get_message_definitions(&pool));
        let grpc_services: Vec<GrpcServiceInfo> = services
            .into_iter()
            .map(|s| GrpcServiceInfo {
                service_name: s.service_name,
                methods: s
                    .methods
                    .into_iter()
                    .map(|m| GrpcMethodInfo {
                        method_name: m.method_name,
                        input_type: m.input_type,
                        output_type: m.output_type,
                        is_server_streaming: m.is_server_streaming,
                        is_client_streaming: m.is_client_streaming,
                    })
                    .collect(),
                message_definitions: message_definitions.clone(),
            })
            .collect();

        // 5. 存入连接池（重复 connect 覆盖旧连接）
        let connection = GrpcConnection {
            channel,
            descriptor_pool: Arc::new(pool),
            url: cfg.url.clone(),
            status: "connected".to_string(),
            created_at: chrono::Utc::now(),
            default_metadata: cfg.default_metadata.clone(),
            compression_encoding: cfg.compression_encoding.clone(),
            active_calls: Arc::new(Mutex::new(HashMap::new())),
            last_error: None,
            max_retries: cfg.max_retries.unwrap_or(0),
            initial_backoff_ms: cfg.initial_backoff_ms.unwrap_or(1000),
        };

        {
            let mut conns = self
                .connections
                .write()
                .map_err(|e| format!("failed to acquire connection pool write lock: {}", e))?;
            conns.insert((execution_id.to_string(), connection_id.to_string()), connection);
        }

        Ok(grpc_services)
    }

    /// 读取连接（克隆 channel/descriptor_pool 后释放锁）。
    fn snapshot(
        &self,
        execution_id: &str,
        connection_id: &str,
    ) -> Result<
        (
            tonic::transport::Channel,
            Arc<prost_reflect::DescriptorPool>,
            Vec<(String, String)>,
            Option<String>,
        ),
        String,
    > {
        let conns = self
            .connections
            .read()
            .map_err(|e| format!("failed to acquire connection pool read lock: {}", e))?;
        let conn = conns
            .get(&(execution_id.to_string(), connection_id.to_string()))
            .ok_or_else(|| format!("connection '{}' does not exist", connection_id))?;
        Ok((
            conn.channel.clone(),
            Arc::clone(&conn.descriptor_pool),
            conn.default_metadata.clone(),
            conn.compression_encoding.clone(),
        ))
    }

    /// 获取指定连接的 `DescriptorPool` 克隆（设计时骨架生成用）。
    pub fn get_descriptor_pool(
        &self,
        execution_id: &str,
        connection_id: &str,
    ) -> Option<prost_reflect::DescriptorPool> {
        let conns = self.connections.read().ok()?;
        let conn = conns.get(&(execution_id.to_string(), connection_id.to_string()))?;
        Some((*conn.descriptor_pool).clone())
    }

    /// 获取连接的可用服务列表。
    pub fn list_services(
        &self,
        execution_id: &str,
        connection_id: &str,
    ) -> Option<Vec<GrpcServiceInfo>> {
        let conns = self.connections.read().ok()?;
        let conn = conns.get(&(execution_id.to_string(), connection_id.to_string()))?;
        let services = proto_parser::list_services(&conn.descriptor_pool);
        let message_definitions =
            Arc::new(proto_parser::get_message_definitions(&conn.descriptor_pool));
        Some(
            services
                .into_iter()
                .map(|s| GrpcServiceInfo {
                    service_name: s.service_name,
                    methods: s
                        .methods
                        .into_iter()
                        .map(|m| GrpcMethodInfo {
                            method_name: m.method_name,
                            input_type: m.input_type,
                            output_type: m.output_type,
                            is_server_streaming: m.is_server_streaming,
                            is_client_streaming: m.is_client_streaming,
                        })
                        .collect(),
                    message_definitions: message_definitions.clone(),
                })
                .collect(),
        )
    }

    /// 连接级重试默认值（call 级未配置时使用）。
    pub fn get_retry_defaults(
        &self,
        execution_id: &str,
        connection_id: &str,
    ) -> (u32, u64) {
        let conns = match self.connections.read() {
            Ok(c) => c,
            Err(_) => return (0, 1000),
        };
        match conns.get(&(execution_id.to_string(), connection_id.to_string())) {
            Some(conn) => (conn.max_retries, conn.initial_backoff_ms),
            None => (0, 1000),
        }
    }

    /// 关闭 gRPC 连接（移除池条目，tonic Channel 自动 drop 关闭）。
    pub async fn close(&self, execution_id: &str, connection_id: &str) -> Result<(), String> {
        let mut conns = self
            .connections
            .write()
            .map_err(|e| format!("failed to acquire connection pool write lock: {}", e))?;
        conns
            .remove(&(execution_id.to_string(), connection_id.to_string()))
            .ok_or_else(|| format!("connection '{}' does not exist", connection_id))?;
        Ok(())
    }

    /// 清空某个 execution 的全部连接（`flow_ended` 调用；不存在条目则 no-op）。
    pub fn close_all(&self, execution_id: &str) {
        let mut conns = match self.connections.write() {
            Ok(c) => c,
            Err(_) => return,
        };
        conns.retain(|(exec, _), _| exec != execution_id);
    }

    /// 取消进行中的流式调用。
    ///
    /// `execution_id` 为空时按 `connection_id` 全局匹配（设计时 viewer
    /// 取消按钮不知道运行期 execution_id；call_id 已全局唯一）。
    pub fn cancel_stream(
        &self,
        execution_id: &str,
        connection_id: &str,
        call_id: &str,
    ) -> Result<(), String> {
        let active_calls = {
            let conns = self
                .connections
                .read()
                .map_err(|e| format!("failed to acquire connection pool read lock: {}", e))?;
            let conn = if execution_id.is_empty() {
                conns
                    .iter()
                    .find(|((_, cid), _)| cid == connection_id)
                    .map(|(_, c)| c)
            } else {
                conns.get(&(execution_id.to_string(), connection_id.to_string()))
            };
            let conn = conn.ok_or_else(|| format!("connection '{}' does not exist", connection_id))?;
            Arc::clone(&conn.active_calls)
        };

        let mut calls = active_calls
            .lock()
            .map_err(|e| format!("failed to acquire active calls lock: {}", e))?;
        match calls.get(call_id) {
            Some(token) => {
                log::info!(
                    "[gRPC] cancelling streaming call: connection_id={}, call_id={}",
                    connection_id,
                    call_id
                );
                token.cancel();
                calls.remove(call_id);
                Ok(())
            }
            None => Err(format!("call '{}' does not exist or already finished", call_id)),
        }
    }

    /// gRPC Unary 调用。
    pub async fn call_unary(
        &self,
        execution_id: &str,
        connection_id: &str,
        service_name: &str,
        method_name: &str,
        request_json: &str,
        metadata: Vec<(String, String)>,
        timeout_ms: u64,
        compression_encoding: Option<String>,
        _call_id: &str,
    ) -> Result<(serde_json::Value, Vec<(String, String)>), String> {
        let (channel, pool, default_metadata, default_compression) =
            self.snapshot(execution_id, connection_id)?;

        let merged_metadata = merge_metadata(&default_metadata, metadata);

        let service = pool
            .get_service_by_name(service_name)
            .ok_or_else(|| format!("service '{}' does not exist", service_name))?;
        let method = service
            .methods()
            .find(|m| m.name() == method_name)
            .ok_or_else(|| {
                format!(
                    "method '{}' does not exist in service '{}'",
                    method_name, service_name
                )
            })?;

        let req_message = json_to_dynamic_message(request_json, method.input())?;

        let mut request = tonic::Request::new(req_message);
        for (key, value) in merged_metadata {
            let meta_key = MetadataKey::from_bytes(key.as_bytes())
                .map_err(|e| format!("invalid metadata key '{}': {:?}", key, e))?;
            let meta_val = MetadataValue::try_from(&value)
                .map_err(|e| format!("invalid metadata value '{}': {:?}", value, e))?;
            request.metadata_mut().insert(meta_key, meta_val);
        }
        if timeout_ms > 0 {
            request.set_timeout(std::time::Duration::from_millis(timeout_ms));
        }

        let path = build_method_path(service_name, method_name)?;
        let codec = DynamicCodec::new(method);
        let mut client = tonic::client::Grpc::new(channel)
            .accept_compressed(tonic::codec::CompressionEncoding::Gzip);
        let compression =
            resolve_compression(compression_encoding.as_deref(), default_compression.as_deref());
        if let Some(enc) = compression {
            client = client.send_compressed(enc);
        }
        client
            .ready()
            .await
            .map_err(|e| format!("gRPC client not ready: {}", e))?;

        let response = client
            .unary(request, path, codec)
            .await
            .map_err(|e| format_grpc_error("gRPC Unary call failed", &e, &pool))?;

        let response_metadata = extract_metadata(response.metadata());
        let response_msg = response.into_inner();
        let json_val = dynamic_message_to_json(&response_msg)?;

        Ok((json_val, response_metadata))
    }

    /// gRPC Server Streaming 调用（逐条实时 emit + 收集全部响应）。
    pub async fn call_server_streaming(
        &self,
        execution_id: &str,
        connection_id: &str,
        service_name: &str,
        method_name: &str,
        request_json: &str,
        metadata: Vec<(String, String)>,
        timeout_ms: u64,
        compression_encoding: Option<String>,
        call_id: &str,
        ctx: &ExecuteContext,
    ) -> Result<(Vec<serde_json::Value>, Vec<(String, String)>), String> {
        let (channel, pool, default_metadata, default_compression, active_calls) = {
            let conns = self
                .connections
                .read()
                .map_err(|e| format!("failed to acquire connection pool read lock: {}", e))?;
            let conn = conns
                .get(&(execution_id.to_string(), connection_id.to_string()))
                .ok_or_else(|| format!("connection '{}' does not exist", connection_id))?;
            (
                conn.channel.clone(),
                Arc::clone(&conn.descriptor_pool),
                conn.default_metadata.clone(),
                conn.compression_encoding.clone(),
                Arc::clone(&conn.active_calls),
            )
        };

        // 注册 CancellationToken（uiCall grpc.cancelStream / SDK cancel 触发）
        let cancel_token = CancellationToken::new();
        {
            let mut calls = active_calls
                .lock()
                .map_err(|e| format!("failed to acquire active calls lock: {}", e))?;
            calls.insert(call_id.to_string(), cancel_token.clone());
        }
        self.register_in_flight(ctx.request_id(), &cancel_token);

        let merged_metadata = merge_metadata(&default_metadata, metadata);

        let service = pool
            .get_service_by_name(service_name)
            .ok_or_else(|| format!("service '{}' does not exist", service_name))?;
        let method = service
            .methods()
            .find(|m| m.name() == method_name)
            .ok_or_else(|| {
                format!(
                    "method '{}' does not exist in service '{}'",
                    method_name, service_name
                )
            })?;

        let req_message = json_to_dynamic_message(request_json, method.input())?;

        let mut request = tonic::Request::new(req_message);
        for (key, value) in merged_metadata {
            let meta_key = MetadataKey::from_bytes(key.as_bytes())
                .map_err(|e| format!("invalid metadata key '{}': {:?}", key, e))?;
            let meta_val = MetadataValue::try_from(&value)
                .map_err(|e| format!("invalid metadata value '{}': {:?}", value, e))?;
            request.metadata_mut().insert(meta_key, meta_val);
        }
        // 不设 request deadline（流式响应可长于 timeout；收集阶段用 timeout 包裹）

        let path = build_method_path(service_name, method_name)?;
        let codec = DynamicCodec::new(method);
        let mut client = tonic::client::Grpc::new(channel)
            .accept_compressed(tonic::codec::CompressionEncoding::Gzip);
        let compression =
            resolve_compression(compression_encoding.as_deref(), default_compression.as_deref());
        if let Some(enc) = compression {
            client = client.send_compressed(enc);
        }
        client
            .ready()
            .await
            .map_err(|e| format!("gRPC client not ready: {}", e))?;

        let response = client
            .server_streaming(request, path, codec)
            .await
            .map_err(|e| format_grpc_error("gRPC Server Streaming call failed", &e, &pool))?;

        let response_metadata = extract_metadata(response.metadata());
        let mut stream = response.into_inner();
        let mut results: Vec<serde_json::Value> = Vec::new();
        let collect_start = std::time::Instant::now();

        let collect_result = tokio::time::timeout(
            std::time::Duration::from_millis(timeout_ms),
            async {
                loop {
                    tokio::select! {
                        msg = stream.message() => {
                            match msg {
                                Ok(Some(msg)) => {
                                    let size_bytes = msg.encoded_len();
                                    let json_val = dynamic_message_to_json(&msg)?;
                                    let msg_index = results.len() + 1;
                                    let received_at = collect_start.elapsed().as_millis() as u64;

                                    // 实时推送（backpressure 通知）
                                    ctx.emit(
                                        "grpc.stream",
                                        json!({
                                            "call_id": call_id,
                                            "kind": "message",
                                            "data": json_val,
                                        }),
                                    )
                                    .await;

                                    let stream_msg = crate::types::GrpcStreamMessage {
                                        index: msg_index,
                                        data: json_val,
                                        received_at_ms: received_at,
                                        size_bytes,
                                    };
                                    results.push(serde_json::to_value(&stream_msg).unwrap_or_default());
                                }
                                Ok(None) => break,
                                Err(e) => {
                                    ctx.emit(
                                        "grpc.stream",
                                        json!({
                                            "call_id": call_id,
                                            "kind": "error",
                                            "data": {
                                                "message": e.message(),
                                                "code": e.code().to_string(),
                                            },
                                        }),
                                    )
                                    .await;
                                    return Err(format!(
                                        "streaming response error: {} ({})",
                                        e.message(),
                                        e.code()
                                    ));
                                }
                            }
                        }
                        _ = cancel_token.cancelled() => {
                            log::info!(
                                "[gRPC Server Streaming] call cancelled (call_id={}), collected {} responses",
                                call_id,
                                results.len()
                            );
                            break;
                        }
                    }
                }
                Ok(())
            },
        )
        .await;

        // 清理 CancellationToken
        {
            let mut calls = active_calls
                .lock()
                .map_err(|e| format!("failed to acquire active calls lock: {}", e))?;
            calls.remove(call_id);
        }
        self.unregister_in_flight(ctx.request_id());

        match collect_result {
            Ok(Ok(_)) => {}
            Ok(Err(e)) => return Err(e),
            Err(_) => {
                log::warn!(
                    "[gRPC Server Streaming] collection timed out ({}ms), collected {} responses",
                    timeout_ms,
                    results.len()
                );
            }
        }

        Ok((results, response_metadata))
    }

    /// gRPC Client Streaming 调用。
    pub async fn call_client_streaming(
        &self,
        execution_id: &str,
        connection_id: &str,
        service_name: &str,
        method_name: &str,
        request_messages: Vec<String>,
        metadata: Vec<(String, String)>,
        timeout_ms: u64,
        compression_encoding: Option<String>,
        _call_id: &str,
    ) -> Result<(serde_json::Value, Vec<(String, String)>), String> {
        let (channel, pool, default_metadata, default_compression) =
            self.snapshot(execution_id, connection_id)?;

        let merged_metadata = merge_metadata(&default_metadata, metadata);

        let service = pool
            .get_service_by_name(service_name)
            .ok_or_else(|| format!("service '{}' does not exist", service_name))?;
        let method = service
            .methods()
            .find(|m| m.name() == method_name)
            .ok_or_else(|| {
                format!(
                    "method '{}' does not exist in service '{}'",
                    method_name, service_name
                )
            })?;

        let messages: Vec<prost_reflect::DynamicMessage> = request_messages
            .iter()
            .map(|json| json_to_dynamic_message(json, method.input()))
            .collect::<Result<Vec<_>, _>>()?;

        let request_stream = tokio_stream::iter(messages);
        let mut request = tonic::Request::new(request_stream);

        for (key, value) in merged_metadata {
            let meta_key = MetadataKey::from_bytes(key.as_bytes())
                .map_err(|e| format!("invalid metadata key '{}': {:?}", key, e))?;
            let meta_val = MetadataValue::try_from(&value)
                .map_err(|e| format!("invalid metadata value '{}': {:?}", value, e))?;
            request.metadata_mut().insert(meta_key, meta_val);
        }
        if timeout_ms > 0 {
            request.set_timeout(std::time::Duration::from_millis(timeout_ms));
        }

        let path = build_method_path(service_name, method_name)?;
        let codec = DynamicCodec::new(method);
        let mut client = tonic::client::Grpc::new(channel)
            .accept_compressed(tonic::codec::CompressionEncoding::Gzip);
        let compression =
            resolve_compression(compression_encoding.as_deref(), default_compression.as_deref());
        if let Some(enc) = compression {
            client = client.send_compressed(enc);
        }
        client
            .ready()
            .await
            .map_err(|e| format!("gRPC client not ready: {}", e))?;

        let response = client
            .client_streaming(request, path, codec)
            .await
            .map_err(|e| format_grpc_error("gRPC Client Streaming call failed", &e, &pool))?;

        let response_metadata = extract_metadata(response.metadata());
        let response_msg = response.into_inner();
        let json_val = dynamic_message_to_json(&response_msg)?;

        Ok((json_val, response_metadata))
    }

    /// gRPC Bidi Streaming 调用（逐条实时 emit + 收集全部响应）。
    pub async fn call_bidi_streaming(
        &self,
        execution_id: &str,
        connection_id: &str,
        service_name: &str,
        method_name: &str,
        request_messages: Vec<String>,
        metadata: Vec<(String, String)>,
        timeout_ms: u64,
        compression_encoding: Option<String>,
        call_id: &str,
        ctx: &ExecuteContext,
    ) -> Result<GrpcStreamingResult, String> {
        use std::time::Instant;

        let start = Instant::now();

        let (channel, pool, default_metadata, default_compression, active_calls) = {
            let conns = self
                .connections
                .read()
                .map_err(|e| format!("failed to acquire connection pool read lock: {}", e))?;
            let conn = conns
                .get(&(execution_id.to_string(), connection_id.to_string()))
                .ok_or_else(|| format!("connection '{}' does not exist", connection_id))?;
            (
                conn.channel.clone(),
                Arc::clone(&conn.descriptor_pool),
                conn.default_metadata.clone(),
                conn.compression_encoding.clone(),
                Arc::clone(&conn.active_calls),
            )
        };

        let cancel_token = CancellationToken::new();
        {
            let mut calls = active_calls
                .lock()
                .map_err(|e| format!("failed to acquire active calls lock: {}", e))?;
            calls.insert(call_id.to_string(), cancel_token.clone());
        }
        self.register_in_flight(ctx.request_id(), &cancel_token);

        let merged_metadata = merge_metadata(&default_metadata, metadata);

        let service = pool
            .get_service_by_name(service_name)
            .ok_or_else(|| format!("service '{}' does not exist", service_name))?;
        let method = service
            .methods()
            .find(|m| m.name() == method_name)
            .ok_or_else(|| {
                format!(
                    "method '{}' does not exist in service '{}'",
                    method_name, service_name
                )
            })?;

        let messages: Vec<prost_reflect::DynamicMessage> = request_messages
            .iter()
            .map(|json| json_to_dynamic_message(json, method.input()))
            .collect::<Result<Vec<_>, _>>()?;
        let sent_count = messages.len();

        let request_stream = tokio_stream::iter(messages);
        let mut request = tonic::Request::new(request_stream);

        for (key, value) in merged_metadata {
            let meta_key = MetadataKey::from_bytes(key.as_bytes())
                .map_err(|e| format!("invalid metadata key '{}': {:?}", key, e))?;
            let meta_val = MetadataValue::try_from(&value)
                .map_err(|e| format!("invalid metadata value '{}': {:?}", value, e))?;
            request.metadata_mut().insert(meta_key, meta_val);
        }

        let path = build_method_path(service_name, method_name)?;
        let codec = DynamicCodec::new(method);
        let mut client = tonic::client::Grpc::new(channel)
            .accept_compressed(tonic::codec::CompressionEncoding::Gzip);
        let compression =
            resolve_compression(compression_encoding.as_deref(), default_compression.as_deref());
        if let Some(enc) = compression {
            client = client.send_compressed(enc);
        }
        client
            .ready()
            .await
            .map_err(|e| format!("gRPC client not ready: {}", e))?;

        let response = client
            .streaming(request, path, codec)
            .await
            .map_err(|e| format_grpc_error("gRPC Bidi Streaming call failed", &e, &pool))?;

        let response_metadata = extract_metadata(response.metadata());
        let mut stream = response.into_inner();
        let mut responses: Vec<serde_json::Value> = Vec::new();
        let mut first_response_ms: Option<u64> = None;
        let mut was_cancelled = false;

        let collect_result = tokio::time::timeout(
            std::time::Duration::from_millis(timeout_ms),
            async {
                loop {
                    tokio::select! {
                        msg = stream.message() => {
                            match msg {
                                Ok(Some(msg)) => {
                                    if first_response_ms.is_none() {
                                        first_response_ms = Some(start.elapsed().as_millis() as u64);
                                    }
                                    let size_bytes = msg.encoded_len();
                                    let json_val = dynamic_message_to_json(&msg)?;
                                    let msg_index = responses.len() + 1;
                                    let received_at = start.elapsed().as_millis() as u64;

                                    ctx.emit(
                                        "grpc.stream",
                                        json!({
                                            "call_id": call_id,
                                            "kind": "message",
                                            "data": json_val,
                                        }),
                                    )
                                    .await;

                                    let stream_msg = crate::types::GrpcStreamMessage {
                                        index: msg_index,
                                        data: json_val,
                                        received_at_ms: received_at,
                                        size_bytes,
                                    };
                                    responses.push(serde_json::to_value(&stream_msg).unwrap_or_default());
                                }
                                Ok(None) => break,
                                Err(e) => {
                                    ctx.emit(
                                        "grpc.stream",
                                        json!({
                                            "call_id": call_id,
                                            "kind": "error",
                                            "data": {
                                                "message": e.message(),
                                                "code": e.code().to_string(),
                                            },
                                        }),
                                    )
                                    .await;
                                    return Err(format!(
                                        "bidi streaming response error: {} ({})",
                                        e.message(),
                                        e.code()
                                    ));
                                }
                            }
                        }
                        _ = cancel_token.cancelled() => {
                            log::info!(
                                "[gRPC Bidi] call cancelled (call_id={}), collected {} responses",
                                call_id,
                                responses.len()
                            );
                            was_cancelled = true;
                            break;
                        }
                    }
                }
                Ok(())
            },
        )
        .await;

        {
            let mut calls = active_calls
                .lock()
                .map_err(|e| format!("failed to acquire active calls lock: {}", e))?;
            calls.remove(call_id);
        }
        self.unregister_in_flight(ctx.request_id());

        match collect_result {
            Ok(Ok(_)) => {}
            Ok(Err(e)) => return Err(e),
            Err(_) => {
                log::warn!(
                    "[gRPC Bidi] collection timed out ({}ms), collected {} responses",
                    timeout_ms,
                    responses.len()
                );
            }
        }

        let received_count = responses.len();
        let total_ms = start.elapsed().as_millis() as u64;

        Ok(GrpcStreamingResult {
            status: 0,
            responses,
            sent_count,
            received_count,
            response_metadata,
            trailers: std::collections::HashMap::new(),
            timing: GrpcTimingInfo {
                total_ms,
                first_response_ms,
            },
            cancelled: was_cancelled,
        })
    }

    /// 获取连接的 Channelz 内省信息。
    ///
    /// `execution_id` 为空时按 `connection_id` 全局匹配（设计时面板
    /// 不知道运行期 execution_id）。
    pub async fn get_channelz_info(
        &self,
        execution_id: &str,
        connection_id: &str,
    ) -> Result<ChannelzInfo, String> {
        let (mut channel, created_at, active_calls, last_error) = {
            let conns = self
                .connections
                .read()
                .map_err(|e| format!("failed to acquire connection pool read lock: {}", e))?;
            let conn = if execution_id.is_empty() {
                conns
                    .iter()
                    .find(|((_, cid), _)| cid == connection_id)
                    .map(|(_, c)| c)
            } else {
                conns.get(&(execution_id.to_string(), connection_id.to_string()))
            };
            let conn = conn
                .ok_or_else(|| format!("connection '{}' does not exist", connection_id))?;
            (
                conn.channel.clone(),
                conn.created_at,
                Arc::clone(&conn.active_calls),
                conn.last_error.clone(),
            )
        };

        let state =
            match tokio::time::timeout(std::time::Duration::from_secs(3), channel.ready()).await {
                Ok(Ok(_)) => "READY".to_string(),
                Ok(Err(_)) => "TRANSIENT_FAILURE".to_string(),
                Err(_) => "CONNECTING".to_string(),
            };

        let uptime_secs = created_at
            .signed_duration_since(chrono::Utc::now())
            .num_seconds()
            .unsigned_abs();

        let active_call_count = active_calls.lock().map(|calls| calls.len()).unwrap_or(0);

        Ok(ChannelzInfo {
            state,
            connected_at: created_at.to_rfc3339(),
            uptime_secs,
            active_calls: active_call_count,
            last_error,
        })
    }

    /// 健康检查（gRPC Health Checking Protocol unary Check）。
    ///
    /// `execution_id` 为空时按 `connection_id` 全局匹配。
    pub async fn health_check(
        &self,
        execution_id: &str,
        connection_id: &str,
        service: &str,
    ) -> Result<crate::types::HealthCheckResponse, String> {
        use tonic_health::pb::health_client::HealthClient;
        use tonic_health::pb::HealthCheckRequest;

        let channel = {
            let conns = self
                .connections
                .read()
                .map_err(|e| format!("failed to acquire connection pool read lock: {}", e))?;
            let conn = if execution_id.is_empty() {
                conns
                    .iter()
                    .find(|((_, cid), _)| cid == connection_id)
                    .map(|(_, c)| c)
            } else {
                conns.get(&(execution_id.to_string(), connection_id.to_string()))
            };
            let conn = conn
                .ok_or_else(|| format!("connection '{}' does not exist", connection_id))?;
            conn.channel.clone()
        };

        let mut client = HealthClient::new(channel);
        let request = tonic::Request::new(HealthCheckRequest {
            service: service.to_string(),
        });

        match client.check(request).await {
            Ok(response) => {
                let status = response.into_inner().status();
                let status_str = match status {
                    tonic_health::pb::health_check_response::ServingStatus::Serving => "SERVING",
                    tonic_health::pb::health_check_response::ServingStatus::NotServing => {
                        "NOT_SERVING"
                    }
                    tonic_health::pb::health_check_response::ServingStatus::ServiceUnknown => {
                        "SERVICE_UNKNOWN"
                    }
                    _ => "UNKNOWN",
                };
                Ok(crate::types::HealthCheckResponse {
                    status: status_str.to_string(),
                })
            }
            Err(status) => {
                if status.code() == tonic::Code::Unimplemented {
                    Ok(crate::types::HealthCheckResponse {
                        status: "NOT_IMPLEMENTED".to_string(),
                    })
                } else {
                    Err(format!(
                        "health check failed: {} ({})",
                        status.message(),
                        status.code()
                    ))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_connect_config_from_value_defaults() {
        let cfg = ConnectConfig::from_value(&json!({ "url": "localhost:50051" }))
            .unwrap_or_else(|e| panic!("parse: {e}"));
        assert_eq!(cfg.url, "localhost:50051");
        assert_eq!(cfg.connect_timeout_ms, 30_000);
        assert!(!cfg.use_tls);
        assert!(cfg.proto_files.is_empty());
        assert!(cfg.default_metadata.is_empty());
        assert_eq!(cfg.max_retries, None);
    }

    #[test]
    fn test_connect_config_from_value_full() {
        let cfg = ConnectConfig::from_value(&json!({
            "url": "localhost:50051",
            "use_tls": true,
            "enable_reflection": true,
            "proto_files": [{"path": "a.proto", "content": "syntax = \"proto3\";"}],
            "default_metadata": [{"key": "k", "value": "v"}],
            "connect_timeout_ms": 5000,
            "endpoints": ["localhost:50052"],
            "max_retries": 3,
            "initial_backoff_ms": 200,
        }))
        .unwrap_or_else(|e| panic!("parse: {e}"));
        assert!(cfg.use_tls);
        assert!(cfg.enable_reflection);
        assert_eq!(cfg.proto_files.len(), 1);
        assert_eq!(cfg.default_metadata, vec![("k".to_string(), "v".to_string())]);
        assert_eq!(cfg.connect_timeout_ms, 5000);
        assert_eq!(cfg.endpoints, Some(vec!["localhost:50052".to_string()]));
        assert_eq!(cfg.max_retries, Some(3));
        assert_eq!(cfg.initial_backoff_ms, Some(200));
    }

    #[test]
    fn test_call_config_from_value() {
        let cfg = CallConfig::from_value(&json!({
            "connection_id": "conn-1",
            "service_name": "echo.Echo",
            "method_name": "Unary",
            "request_json": "{}",
            "timeout_ms": 1000,
            "metadata": [{"key": "a", "value": "b"}],
            "request_messages": [
                {"content": "{\"n\":1}", "enabled": true},
                {"content": "{\"n\":2}", "enabled": false},
            ],
            "max_retries": 2,
        }))
        .unwrap_or_else(|e| panic!("parse: {e}"));
        assert_eq!(cfg.connection_id, "conn-1");
        assert_eq!(cfg.timeout_ms, 1000);
        assert_eq!(cfg.metadata, vec![("a".to_string(), "b".to_string())]);
        assert_eq!(
            cfg.request_messages,
            vec![
                ("{\"n\":1}".to_string(), true),
                ("{\"n\":2}".to_string(), false)
            ]
        );
        assert_eq!(cfg.max_retries, Some(2));
    }

    #[test]
    fn test_call_config_default_timeout() {
        let cfg = CallConfig::from_value(&json!({})).unwrap_or_else(|e| panic!("parse: {e}"));
        assert_eq!(cfg.timeout_ms, 30_000);
    }

    #[test]
    fn test_pool_close_all_noop_on_missing() {
        let pool = GrpcPool::new();
        pool.close_all("exec-missing"); // 不应 panic
    }

    #[test]
    fn test_retry_defaults_missing_connection() {
        let pool = GrpcPool::new();
        assert_eq!(pool.get_retry_defaults("e", "c"), (0, 1000));
    }
}
