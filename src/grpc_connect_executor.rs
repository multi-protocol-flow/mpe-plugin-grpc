//! `grpc:connect` 节点执行器
//!
//! 从宿主 `flow-engine-grpc::grpc_connect_executor` 迁移：签名改为
//! `execute(ctx, pool)`，config 已是宿主变量解析后的最终值（无需
//! `replace_variables`）。连接池键 = (execution_id, node_instance_id)。
//! 输出形状与旧实现字节级兼容。

use std::time::Instant;

use mpe_plugin_sdk::context::ExecuteContext;
use mpe_plugin_sdk::types::ExecuteResult;
use serde_json::json;

use crate::i18n;
use crate::pool::{ConnectConfig, GrpcPool};
use crate::types::GrpcServiceInfo;

/// 成功结果：显式路由 `true` 端口（宿主串联执行依赖 next_ports）。
fn ok_result(output: serde_json::Value) -> ExecuteResult {
    ExecuteResult {
        next_ports: vec!["true".to_string()],
        ..ExecuteResult::ok(output)
    }
}

/// 失败结果：宿主按节点 `on_error` 策略路由（默认 RouteToFalse → false
/// 端口）；错误输出经 `report_data` back-channel 进入报告 `plugin_data`，
/// 报告 viewer 从 `output_data ?? plugin_data` 读取。
fn fail_result(message: String, error_output: serde_json::Value) -> ExecuteResult {
    ExecuteResult {
        errors: vec![message],
        report_data: Some(error_output),
        ..ExecuteResult::fail("")
    }
}

pub async fn execute(ctx: &mut ExecuteContext, pool: &GrpcPool) -> ExecuteResult {
    let start_time = Instant::now();

    let config = match ConnectConfig::from_value(ctx.config()) {
        Ok(c) => c,
        Err(e) => {
            return fail_result(
                e.clone(),
                json!({ "success": false, "error": e }),
            )
        }
    };

    // 连接节点以自身 instance id 注册连接池
    let execution_id = match ctx.execution_id() {
        Some(id) => id.to_string(),
        None => {
            return fail_result(
                i18n::t("缺少 execution_id", "missing execution_id").to_string(),
                json!({ "success": false, "error": "missing execution_id" }),
            )
        }
    };
    let connection_id = match ctx.node_instance_id() {
        Some(id) => id.to_string(),
        None => {
            return fail_result(
                i18n::t("缺少 node_instance_id", "missing node_instance_id").to_string(),
                json!({ "success": false, "error": "missing node_instance_id" }),
            )
        }
    };

    log::info!(
        "[gRPC] connect start: execution_id={}, connection_id={}, url={}",
        execution_id,
        connection_id,
        config.url
    );

    let url = config.url.clone();
    let connect_start = Instant::now();
    let connect_result = pool
        .connect(&execution_id, &connection_id, &config)
        .await;
    let connect_duration_ms = connect_start.elapsed().as_millis() as u64;

    match connect_result {
        Ok(services) => {
            let duration = start_time.elapsed();
            let output_data = build_success_output(
                &connection_id,
                &url,
                connect_duration_ms,
                duration.as_millis() as u64,
                &services,
                &config,
            );
            log::info!(
                "[gRPC] connect succeeded: connection_id={}, url={}, service_count={}",
                connection_id,
                url,
                services.len()
            );
            ok_result(output_data)
        }
        Err(e) => {
            let duration = start_time.elapsed();
            let error_output = json!({
                "success": false,
                "connection_id": serde_json::Value::Null,
                "url": url,
                "request": {
                    "url": url,
                    "use_tls": config.use_tls,
                    "tls_skip_verify": config.tls_skip_verify,
                    "enable_reflection": config.enable_reflection,
                    "proto_files_count": config.proto_files.len(),
                    "default_metadata_count": config.default_metadata.len(),
                    "connect_timeout_ms": config.connect_timeout_ms,
                },
                "timing": {
                    "connect_ms": connect_duration_ms,
                    "total_ms": duration.as_millis() as u64,
                },
                "error": e,
            });
            log::error!("[gRPC] connect failed: connection_id={}, url={}", connection_id, url);
            fail_result(e, error_output)
        }
    }
}

/// 构建成功输出数据（与旧 `GrpcConnectExecutor::build_success_output`
/// 形状一致）。
fn build_success_output(
    connection_id: &str,
    url: &str,
    connect_duration_ms: u64,
    total_ms: u64,
    services: &[GrpcServiceInfo],
    config: &ConnectConfig,
) -> serde_json::Value {
    let services_json: Vec<serde_json::Value> = services
        .iter()
        .map(|s| {
            let methods_json: Vec<serde_json::Value> = s
                .methods
                .iter()
                .map(|m| {
                    json!({
                        "method_name": m.method_name,
                        "input_type": m.input_type,
                        "output_type": m.output_type,
                        "is_server_streaming": m.is_server_streaming,
                    })
                })
                .collect();
            json!({
                "service_name": s.service_name,
                "methods": methods_json,
            })
        })
        .collect();

    json!({
        "success": true,
        "connection_id": connection_id,
        "url": url,
        "request": {
            "url": url,
            "use_tls": config.use_tls,
            "tls_skip_verify": config.tls_skip_verify,
            "enable_reflection": config.enable_reflection,
            "proto_files_count": config.proto_files.len(),
            "default_metadata_count": config.default_metadata.len(),
            "connect_timeout_ms": config.connect_timeout_ms,
        },
        "timing": {
            "connect_ms": connect_duration_ms,
            "total_ms": total_ms,
        },
        "services": services_json,
        "service_count": services.len(),
    })
}
