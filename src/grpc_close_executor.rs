//! `grpc:close` 节点执行器
//!
//! 从宿主 `flow-engine-grpc::grpc_close_executor` 迁移：显式释放
//! (execution_id, connection_id) 单条连接（`flow_ended` 是另一释放路径，
//! 双释放无害——close 对缺失条目报错，flow_ended 对缺失条目 no-op）。

use std::time::Instant;

use mpe_plugin_sdk::context::ExecuteContext;
use mpe_plugin_sdk::types::ExecuteResult;
use serde_json::json;

use crate::i18n;
use crate::pool::GrpcPool;

pub async fn execute(ctx: &mut ExecuteContext, pool: &GrpcPool) -> ExecuteResult {
    let start_time = Instant::now();

    let connection_id = ctx
        .config()
        .get("connection_id")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_string();
    let execution_id = match ctx.execution_id() {
        Some(id) => id.to_string(),
        None => {
            return fail_result(
                i18n::t("缺少 execution_id", "missing execution_id").to_string(),
                json!({ "success": false, "error": "missing execution_id" }),
            )
        }
    };

    if connection_id.trim().is_empty() {
        return fail_result(
            i18n::t("缺少 connection_id", "missing connection_id").to_string(),
            json!({ "success": false, "error": "missing connection_id" }),
        );
    }

    log::info!(
        "[gRPC] close start: execution_id={}, connection_id={}",
        execution_id,
        connection_id
    );

    let close_start = Instant::now();
    let close_result = pool.close(&execution_id, &connection_id).await;
    let close_ms = close_start.elapsed().as_millis() as u64;

    match close_result {
        Ok(()) => {
            let duration = start_time.elapsed();
            let output_data = json!({
                "success": true,
                "connection_id": connection_id,
                "request": {
                    "connection_id": connection_id,
                },
                "timing": {
                    "close_ms": close_ms,
                    "total_ms": duration.as_millis() as u64,
                },
            });
            log::info!("[gRPC] close succeeded: connection_id={}", connection_id);
            ExecuteResult {
                next_ports: vec!["true".to_string()],
                ..ExecuteResult::ok(output_data)
            }
        }
        Err(e) => {
            let duration = start_time.elapsed();
            let error_output = json!({
                "success": false,
                "connection_id": connection_id,
                "request": {
                    "connection_id": connection_id,
                },
                "timing": {
                    "close_ms": close_ms,
                    "total_ms": duration.as_millis() as u64,
                },
                "error": e,
            });
            log::error!("[gRPC] close failed: connection_id={}", connection_id);
            fail_result(e, error_output)
        }
    }
}

/// 失败结果：错误输出经 `report_data` 进入报告 `plugin_data`。
fn fail_result(message: String, error_output: serde_json::Value) -> ExecuteResult {
    ExecuteResult {
        errors: vec![message],
        report_data: Some(error_output),
        ..ExecuteResult::fail("")
    }
}
