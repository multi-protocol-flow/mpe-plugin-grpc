//! 设计时查询 uiCall 子方法实现。
//!
//! 全部由宿主 `plugin_ui_call` 中继转发（宿主纯转发，无业务逻辑），
//! 方法名与参数形状见下。失败统一 JSON-RPC error（`Err(String)`）。

use std::sync::Arc;

use serde_json::json;

use crate::pool::GrpcPool;
use crate::proto_parser;
use crate::reflection::{
    discover_services_via_proto, discover_services_via_reflection,
    generate_skeleton_via_reflection,
};
use crate::types::{GrpcMethodInfo, GrpcServiceInfo};

/// 把 `DescriptorPool` 的服务列表转为 `GrpcServiceInfo`（带 message_definitions）。
fn services_from_pool(
    pool: &prost_reflect::DescriptorPool,
) -> Vec<GrpcServiceInfo> {
    let services = proto_parser::list_services(pool);
    let message_definitions = Arc::new(proto_parser::get_message_definitions(pool));
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
        .collect()
}

/// `grpc.discover` — 测试连接 & 发现服务（临时连接，不入池）。
async fn discover(params: &serde_json::Value) -> Result<serde_json::Value, String> {
    let url = params.get("url").and_then(serde_json::Value::as_str).unwrap_or("");
    if url.trim().is_empty() {
        return Err("url is required".to_string());
    }
    let use_tls = params.get("use_tls").and_then(serde_json::Value::as_bool).unwrap_or(false);
    let tls_skip_verify = params
        .get("tls_skip_verify")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let enable_reflection = params
        .get("enable_reflection")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let connect_timeout_ms = params
        .get("connect_timeout_ms")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(30_000);

    let proto_files: Vec<(String, String)> = params
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

    let reflection_metadata: Vec<(String, String)> = params
        .get("reflection_metadata")
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
        .unwrap_or_default();

    log::info!(
        "[gRPC uiCall] discover: url={}, reflection={}, proto_files={}",
        url,
        enable_reflection,
        proto_files.len()
    );

    let result = if enable_reflection {
        discover_services_via_reflection(
            url,
            use_tls,
            tls_skip_verify,
            connect_timeout_ms,
            params.get("tls_ca_cert").and_then(serde_json::Value::as_str),
            params.get("tls_client_cert").and_then(serde_json::Value::as_str),
            params.get("tls_client_key").and_then(serde_json::Value::as_str),
            params
                .get("tls_server_name_override")
                .and_then(serde_json::Value::as_str),
            reflection_metadata,
        )
        .await
    } else if !proto_files.is_empty() {
        discover_services_via_proto(&proto_files)
    } else {
        Err("No proto files provided and Server Reflection not enabled".to_string())
    };

    match result {
        Ok(services) => {
            log::info!("[gRPC uiCall] discover succeeded: {} services", services.len());
            Ok(json!({ "success": true, "services": services, "error": null }))
        }
        Err(e) => Ok(json!({ "success": false, "services": [], "error": e })),
    }
}

/// `grpc.scanProtoDirectory` — 递归扫描目录发现 .proto 文件。
fn scan_proto_directory(params: &serde_json::Value) -> Result<serde_json::Value, String> {
    let dir_path = params
        .get("dir_path")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    if dir_path.trim().is_empty() {
        return Err("dir_path is required".to_string());
    }
    match proto_parser::discover_proto_files_in_dir(dir_path) {
        Ok((files, import_path)) => {
            let files_json: Vec<serde_json::Value> = files
                .into_iter()
                .map(|(path, content)| json!({ "path": path, "content": content }))
                .collect();
            Ok(json!({
                "success": true,
                "files": files_json,
                "import_path": import_path,
                "error": null,
            }))
        }
        Err(e) => Ok(json!({
            "success": false,
            "files": [],
            "import_path": null,
            "error": format!("{}", e),
        })),
    }
}

/// `grpc.readProtoFiles` — 读取多个 proto 文件内容（path 取基名）。
fn read_proto_files(params: &serde_json::Value) -> Result<serde_json::Value, String> {
    let file_paths: Vec<String> = params
        .get("file_paths")
        .and_then(serde_json::Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();

    let mut files = Vec::new();
    for file_path in &file_paths {
        match std::fs::read_to_string(file_path) {
            Ok(content) => {
                let file_name = std::path::Path::new(file_path)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| file_path.clone());
                files.push(json!({ "path": file_name, "content": content }));
            }
            Err(e) => {
                return Ok(json!({
                    "success": false,
                    "files": [],
                    "error": format!("Failed to read file {}: {}", file_path, e),
                }));
            }
        }
    }
    Ok(json!({ "success": true, "files": files, "error": null }))
}

/// `grpc.parseDescriptorSet` — 解析 FileDescriptorSet 二进制文件。
fn parse_descriptor_set(params: &serde_json::Value) -> Result<serde_json::Value, String> {
    let file_path = params
        .get("file_path")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    if file_path.trim().is_empty() {
        return Err("file_path is required".to_string());
    }
    let data = std::fs::read(file_path)
        .map_err(|e| format!("Failed to read file: {}", e))?;
    let pool = proto_parser::parse_descriptor_set(&data)
        .map_err(|e| format!("{}", e))?;
    let services = services_from_pool(&pool);
    Ok(json!({ "success": true, "services": services, "error": null }))
}

/// `grpc.skeleton` — 从 proto 文件生成请求 JSON 骨架。
fn skeleton(params: &serde_json::Value) -> Result<serde_json::Value, String> {
    let service_name = params
        .get("service_name")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let method_name = params
        .get("method_name")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");

    let proto_files: Vec<(String, String)> = params
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

    let files: Vec<proto_parser::ProtoFile> = proto_files
        .iter()
        .map(|(path, content)| proto_parser::ProtoFile {
            path: path.clone(),
            content: content.clone(),
        })
        .collect();
    let pool = proto_parser::parse_proto_files(&files)
        .map_err(|e| format!("failed to parse proto files: {}", e))?;
    let skeleton = proto_parser::generate_skeleton_from_pool(&pool, service_name, method_name)?;
    Ok(json!({ "success": true, "skeleton": skeleton, "error": null }))
}

/// `grpc.skeletonReflection` — 通过反射生成请求 JSON 骨架。
async fn skeleton_reflection(params: &serde_json::Value) -> Result<serde_json::Value, String> {
    let url = params.get("url").and_then(serde_json::Value::as_str).unwrap_or("");
    let service_name = params
        .get("service_name")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let method_name = params
        .get("method_name")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");

    let reflection_metadata: Vec<(String, String)> = params
        .get("reflection_metadata")
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
        .unwrap_or_default();

    let skeleton = generate_skeleton_via_reflection(
        url,
        params.get("use_tls").and_then(serde_json::Value::as_bool).unwrap_or(false),
        params
            .get("tls_skip_verify")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        params
            .get("connect_timeout_ms")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(30_000),
        params.get("tls_ca_cert").and_then(serde_json::Value::as_str),
        params.get("tls_client_cert").and_then(serde_json::Value::as_str),
        params.get("tls_client_key").and_then(serde_json::Value::as_str),
        params
            .get("tls_server_name_override")
            .and_then(serde_json::Value::as_str),
        reflection_metadata,
        service_name,
        method_name,
    )
    .await?;
    Ok(json!({ "success": true, "skeleton": skeleton, "error": null }))
}

/// `grpc.validate` — 建议性 JSON 校验（不阻止发送）。
fn validate(params: &serde_json::Value) -> Result<serde_json::Value, String> {
    let json_str = params
        .get("json")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let message_name = params
        .get("message_name")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");

    let proto_files: Vec<(String, String)> = params
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

    if proto_files.is_empty() {
        return Ok(json!({
            "success": false,
            "result": null,
            "error": "No proto files provided, cannot validate",
        }));
    }

    let files: Vec<proto_parser::ProtoFile> = proto_files
        .iter()
        .map(|(path, content)| proto_parser::ProtoFile {
            path: path.clone(),
            content: content.clone(),
        })
        .collect();
    let pool = match proto_parser::parse_proto_files(&files) {
        Ok(pool) => pool,
        Err(e) => {
            return Ok(json!({
                "success": false,
                "result": null,
                "error": format!("Failed to parse proto file: {}", e),
            }));
        }
    };

    match proto_parser::validate_request_json(&pool, message_name, json_str) {
        Ok(result) => Ok(json!({ "success": true, "result": result, "error": null })),
        Err(e) => Ok(json!({ "success": false, "result": null, "error": e })),
    }
}

/// `grpc.channelz` — 运行中连接的 Channelz 内省。
async fn channelz(
    params: &serde_json::Value,
    pool: &GrpcPool,
) -> Result<serde_json::Value, String> {
    let execution_id = params
        .get("execution_id")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let connection_id = params
        .get("connection_id")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    match pool.get_channelz_info(execution_id, connection_id).await {
        Ok(info) => Ok(json!({ "success": true, "info": info, "error": null })),
        Err(e) => Ok(json!({ "success": false, "info": null, "error": e })),
    }
}

/// `grpc.health` — gRPC Health Checking Protocol unary Check。
async fn health(
    params: &serde_json::Value,
    pool: &GrpcPool,
) -> Result<serde_json::Value, String> {
    let execution_id = params
        .get("execution_id")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let connection_id = params
        .get("connection_id")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let service = params
        .get("service")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    match pool.health_check(execution_id, connection_id, service).await {
        Ok(resp) => Ok(json!({ "status": resp.status })),
        Err(e) => Err(e),
    }
}

/// `grpc.cancelStream` — 定向取消进行中的流式调用。
fn cancel_stream(params: &serde_json::Value, pool: &GrpcPool) -> Result<serde_json::Value, String> {
    let execution_id = params
        .get("execution_id")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let connection_id = params
        .get("connection_id")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let call_id = params
        .get("call_id")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    pool.cancel_stream(execution_id, connection_id, call_id)?;
    Ok(json!({}))
}

/// uiCall 分发入口（lib.rs 调用）。
pub async fn dispatch(
    method: &str,
    params: serde_json::Value,
    pool: &GrpcPool,
) -> Result<serde_json::Value, String> {
    match method {
        "grpc.discover" => discover(&params).await,
        "grpc.scanProtoDirectory" => scan_proto_directory(&params),
        "grpc.readProtoFiles" => read_proto_files(&params),
        "grpc.parseDescriptorSet" => parse_descriptor_set(&params),
        "grpc.skeleton" => skeleton(&params),
        "grpc.skeletonReflection" => skeleton_reflection(&params).await,
        "grpc.validate" => validate(&params),
        "grpc.channelz" => channelz(&params, pool).await,
        "grpc.health" => health(&params, pool).await,
        "grpc.cancelStream" => cancel_stream(&params, pool),
        _ => Err(format!("unknown uiCall sub-method `{method}`")),
    }
}

/// 运行中连接键（channelz/health/cancelStream 面板查询用）。
///
/// ui_call 不携带 execution_id（无 execute 上下文），因此这三个方法
/// 需要面板显式传 execution_id + connection_id。

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scan_proto_directory_missing_dir() {
        let result = scan_proto_directory(&json!({}));
        assert!(result.is_err());
    }

    #[test]
    fn test_read_proto_files_missing() {
        let result = read_proto_files(&json!({ "file_paths": ["/nonexistent/x.proto"] }));
        let value = result.unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(value["success"], false);
    }

    #[test]
    fn test_skeleton_requires_proto_files() {
        let result = skeleton(&json!({
            "proto_files": [],
            "service_name": "s",
            "method_name": "m",
        }));
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_no_proto_files() {
        let result = validate(&json!({
            "json": "{}",
            "message_name": "x",
            "proto_files": [],
        }))
        .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(result["success"], false);
        assert!(
            result["error"]
                .as_str()
                .is_some_and(|e| e.contains("No proto files"))
        );
    }

    #[test]
    fn test_cancel_stream_missing_connection() {
        let pool = GrpcPool::new();
        let result = cancel_stream(
            &json!({ "execution_id": "e", "connection_id": "c", "call_id": "x" }),
            &pool,
        );
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_dispatch_unknown_method() {
        let pool = GrpcPool::new();
        let result = dispatch("grpc.unknown", json!({}), &pool).await;
        let message = result.err().expect("unknown method must error");
        assert!(message.contains("unknown uiCall sub-method"));
    }
}
