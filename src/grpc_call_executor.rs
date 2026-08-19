//! `grpc:call` 节点执行器
//!
//! 从宿主 `flow-engine-grpc::grpc_call_executor` 迁移：签名改为
//! `execute(ctx, pool)`，config 已是宿主变量解析后的最终值。模式检测、
//! 重试（unary/server_streaming）、call_id 生成、输出形状与旧实现
//! 字节级兼容。流式消息由 pool 内收集循环 `ctx.emit("grpc.stream", …)`
//! 实时推送。

use std::time::{Duration, Instant};

use mpe_plugin_sdk::context::ExecuteContext;
use mpe_plugin_sdk::types::ExecuteResult;
use rand::Rng;
use serde_json::json;

use crate::i18n;
use crate::pool::{CallConfig, GrpcPool};
use crate::types::{GrpcPattern, GrpcTimingInfo};

/// 最大退避时间上限（30 秒）
const MAX_BACKOFF_CAP_MS: u64 = 30_000;

/// 检查 gRPC Error是否可重试（UNAVAILABLE / DEADLINE_EXCEEDED）。
fn is_retryable_error(error: &str) -> bool {
    let last_paren = match error.rfind('(') {
        Some(pos) => pos,
        None => return false,
    };
    let code = &error[last_paren + 1..];
    let code = match code.strip_suffix(')') {
        Some(c) => c.trim(),
        None => return false,
    };
    code == "UNAVAILABLE" || code == "DEADLINE_EXCEEDED"
}

/// 计算指数退避时间（带随机抖动，上限 30 秒 + 0-50% 抖动）。
fn calculate_backoff_ms(initial_backoff_ms: u64, attempt: u32) -> u64 {
    let base_ms = initial_backoff_ms.saturating_mul(2u64.saturating_pow(attempt));
    let capped_ms = base_ms.min(MAX_BACKOFF_CAP_MS);
    let jitter_ms = if capped_ms > 0 {
        let mut rng = rand::thread_rng();
        rng.gen_range(0..=capped_ms / 2)
    } else {
        0
    };
    capped_ms + jitter_ms
}

/// 成功结果：显式路由 `true` 端口。
fn ok_result(output: serde_json::Value) -> ExecuteResult {
    ExecuteResult {
        next_ports: vec!["true".to_string()],
        ..ExecuteResult::ok(output)
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

/// Unary 调用输出辅助元组。
type CallOutput = (
    &'static str,
    serde_json::Value,
    Vec<serde_json::Value>,
    usize,
    usize,
    std::collections::HashMap<String, String>,
    Option<GrpcTimingInfo>,
    Vec<(String, String)>,
    bool, // cancelled
);

pub async fn execute(ctx: &mut ExecuteContext, pool: &GrpcPool) -> ExecuteResult {
    let start_time = Instant::now();

    let config = match CallConfig::from_value(ctx.config()) {
        Ok(c) => c,
        Err(e) => {
            return fail_result(
                e.clone(),
                json!({ "success": false, "pattern": "unary", "error": e }),
            )
        }
    };

    let execution_id = match ctx.execution_id() {
        Some(id) => id.to_string(),
        None => {
            return fail_result(
                i18n::t("缺少 execution_id", "missing execution_id").to_string(),
                json!({ "success": false, "pattern": "unary", "error": "missing execution_id" }),
            )
        }
    };
    let node_uuid = ctx.node_instance_id().unwrap_or_default().to_string();
    let connection_id = config.connection_id.clone();

    // 1. 检测流式通信模式（默认 Unary）
    let pattern = {
        let services = pool.list_services(&execution_id, &connection_id);
        let method_info = services.as_ref().and_then(|svcs| {
            svcs.iter()
                .find(|s| s.service_name == config.service_name)
                .and_then(|s| {
                    s.methods
                        .iter()
                        .find(|m| m.method_name == config.method_name)
                })
        });
        match method_info {
            Some(info) => {
                GrpcPattern::from_method_info(info.is_client_streaming, info.is_server_streaming)
            }
            None => GrpcPattern::Unary,
        }
    };
    let pattern_str = format!("{:?}", pattern).to_lowercase();

    // 2. 生成调用 ID（前端据此关联实时流消息）
    let call_id = format!("{}-{}", node_uuid, start_time.elapsed().as_millis());

    log::info!(
        "[gRPC] call start: connection_id={}, service={}/{}, pattern={}, call_id={}",
        connection_id,
        config.service_name,
        config.method_name,
        pattern_str,
        call_id
    );

    // 3. 解析重试配置（call 级覆盖连接级默认值）
    let (default_max_retries, default_backoff) =
        pool.get_retry_defaults(&execution_id, &connection_id);
    let max_retries = config.max_retries.unwrap_or(default_max_retries);
    let initial_backoff_ms = config.initial_backoff_ms.unwrap_or(default_backoff);

    // 4. 根据模式路由
    let call_start = Instant::now();

    let call_result: Result<CallOutput, String> = match pattern {
        GrpcPattern::Unary => {
            let mut attempt: u32 = 0;
            loop {
                let result = pool
                    .call_unary(
                        &execution_id,
                        &connection_id,
                        &config.service_name,
                        &config.method_name,
                        &config.request_json,
                        config.metadata.clone(),
                        config.timeout_ms,
                        config.compression_encoding.clone(),
                        &call_id,
                    )
                    .await
                    .map(|(v, meta)| {
                        (
                            "unary",
                            v,
                            vec![] as Vec<serde_json::Value>,
                            0usize,
                            0usize,
                            std::collections::HashMap::new(),
                            None as Option<GrpcTimingInfo>,
                            meta,
                            false,
                        )
                    });
                match result {
                    Ok(output) => break Ok(output),
                    Err(e) => {
                        if attempt < max_retries && is_retryable_error(&e) {
                            let backoff_ms = calculate_backoff_ms(initial_backoff_ms, attempt);
                            log::warn!(
                                "[gRPC] Unary call failed (retryable), retry {}, waiting {}ms: {}",
                                attempt + 1,
                                backoff_ms,
                                e
                            );
                            tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                            attempt += 1;
                        } else {
                            break Err(e);
                        }
                    }
                }
            }
        }
        GrpcPattern::ServerStreaming => {
            let mut attempt: u32 = 0;
            loop {
                let result = pool
                    .call_server_streaming(
                        &execution_id,
                        &connection_id,
                        &config.service_name,
                        &config.method_name,
                        &config.request_json,
                        config.metadata.clone(),
                        config.timeout_ms,
                        config.compression_encoding.clone(),
                        &call_id,
                        ctx,
                    )
                    .await
                    .map(|(responses, meta)| {
                        let received = responses.len();
                        (
                            "server_streaming",
                            serde_json::Value::Null,
                            responses,
                            1usize,
                            received,
                            std::collections::HashMap::new(),
                            None as Option<GrpcTimingInfo>,
                            meta,
                            false,
                        )
                    });
                match result {
                    Ok(output) => break Ok(output),
                    Err(e) => {
                        if attempt < max_retries && is_retryable_error(&e) {
                            let backoff_ms = calculate_backoff_ms(initial_backoff_ms, attempt);
                            log::warn!(
                                "[gRPC] Server streaming call failed (retryable), retry {}, waiting {}ms: {}",
                                attempt + 1,
                                backoff_ms,
                                e
                            );
                            tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                            attempt += 1;
                        } else {
                            break Err(e);
                        }
                    }
                }
            }
        }
        GrpcPattern::ClientStreaming => {
            if max_retries > 0 {
                log::warn!(
                    "[gRPC] Client streaming mode does not support retry, ignoring retry config (max_retries={})",
                    max_retries
                );
            }
            let messages: Vec<String> = config
                .request_messages
                .iter()
                .filter(|(_, enabled)| *enabled)
                .map(|(content, _)| content.clone())
                .collect();
            let sent = messages.len();
            pool.call_client_streaming(
                &execution_id,
                &connection_id,
                &config.service_name,
                &config.method_name,
                messages,
                config.metadata.clone(),
                config.timeout_ms,
                config.compression_encoding.clone(),
                &call_id,
            )
            .await
            .map(|(response, meta)| {
                (
                    "client_streaming",
                    response,
                    vec![] as Vec<serde_json::Value>,
                    sent,
                    1usize,
                    std::collections::HashMap::new(),
                    None as Option<GrpcTimingInfo>,
                    meta,
                    false,
                )
            })
        }
        GrpcPattern::BidiStreaming => {
            if max_retries > 0 {
                log::warn!(
                    "[gRPC] Bidi streaming mode does not support retry, ignoring retry config (max_retries={})",
                    max_retries
                );
            }
            let messages: Vec<String> = config
                .request_messages
                .iter()
                .filter(|(_, enabled)| *enabled)
                .map(|(content, _)| content.clone())
                .collect();
            let sent = messages.len();
            pool.call_bidi_streaming(
                &execution_id,
                &connection_id,
                &config.service_name,
                &config.method_name,
                messages,
                config.metadata.clone(),
                config.timeout_ms,
                config.compression_encoding.clone(),
                &call_id,
                ctx,
            )
            .await
            .map(|result| {
                let received = result.received_count;
                let timing = Some(result.timing);
                let meta = result.response_metadata;
                let cancelled = result.cancelled;
                (
                    "bidi_streaming",
                    serde_json::Value::Null,
                    result.responses,
                    sent,
                    received,
                    result.trailers,
                    timing,
                    meta,
                    cancelled,
                )
            })
        }
    };
    let call_duration_ms = call_start.elapsed().as_millis() as u64;

    match call_result {
        Ok((
            pattern_str,
            response_data,
            responses,
            sent_count,
            received_count,
            trailers,
            streaming_timing,
            response_metadata,
            cancelled,
        )) => {
            let duration = start_time.elapsed();
            let mut output_data = json!({
                "success": true,
                "pattern": pattern_str,
                "connection_id": connection_id,
                "service_name": config.service_name,
                "method_name": config.method_name,
                "status": "ok",
                "status_message": "",
                "request": {
                    "connection_id": connection_id,
                    "service_name": config.service_name,
                    "method_name": config.method_name,
                    "request_json": config.request_json,
                    "timeout_ms": config.timeout_ms,
                    "metadata_count": config.metadata.len(),
                },
                "timing": {
                    "call_ms": call_duration_ms,
                    "total_ms": duration.as_millis() as u64,
                },
            });

            if sent_count > 0 || received_count > 0 {
                output_data["sent_count"] = json!(sent_count);
                output_data["received_count"] = json!(received_count);
            }
            if !responses.is_empty() {
                output_data["responses"] = json!(responses);
            }
            if !trailers.is_empty() {
                output_data["trailers"] = json!(trailers);
            }
            let meta_map: std::collections::HashMap<String, String> =
                response_metadata.into_iter().collect();
            if !meta_map.is_empty() {
                output_data["response_metadata"] = json!(meta_map);
            }
            if let Some(t) = streaming_timing {
                output_data["streaming_timing"] = json!(t);
            }
            if cancelled {
                output_data["cancelled"] = json!(true);
                output_data["status"] = json!("cancelled");
                output_data["status_message"] = json!("User cancelled");
            }
            if response_data != serde_json::Value::Null {
                output_data["data"] = json!(response_data);
            }

            log::info!(
                "[gRPC] Call succeeded: connection_id={}, service={}/{}, pattern={}, duration={}ms",
                connection_id,
                config.service_name,
                config.method_name,
                pattern_str,
                call_duration_ms
            );
            ok_result(output_data)
        }
        Err(e) => {
            let (error_message, error_details) = crate::error_detail::parse_error_details(&e);
            let duration = start_time.elapsed();
            let mut error_output = json!({
                "success": false,
                "pattern": pattern_str,
                "connection_id": connection_id,
                "service_name": config.service_name,
                "method_name": config.method_name,
                "request": {
                    "connection_id": connection_id,
                    "service_name": config.service_name,
                    "method_name": config.method_name,
                    "request_json": config.request_json,
                    "timeout_ms": config.timeout_ms,
                    "metadata_count": config.metadata.len(),
                },
                "timing": {
                    "call_ms": call_duration_ms,
                    "total_ms": duration.as_millis() as u64,
                },
                "data": serde_json::Value::Null,
                "status": "error",
                "status_message": error_message,
                "error": error_message,
            });
            if !error_details.is_empty() {
                error_output["error_details"] = json!(error_details);
            }
            log::error!(
                "[gRPC] Call failed: connection_id={}, service={}/{}, pattern={}, error={}",
                connection_id,
                config.service_name,
                config.method_name,
                pattern_str,
                error_message
            );
            fail_result(error_message.to_string(), error_output)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_retryable_error() {
        assert!(is_retryable_error("connect failed (UNAVAILABLE)"));
        assert!(is_retryable_error("deadline exceeded (DEADLINE_EXCEEDED)"));
        assert!(!is_retryable_error("not found (NOT_FOUND)"));
        assert!(!is_retryable_error("no parens here"));
        assert!(!is_retryable_error(""));
    }

    #[test]
    fn test_calculate_backoff_caps_at_30s() {
        let backoff = calculate_backoff_ms(1000, 20);
        assert!(backoff >= 30_000 && backoff <= 45_000);
    }

    #[test]
    fn test_calculate_backoff_grows_exponentially() {
        let b0 = calculate_backoff_ms(100, 0);
        assert!(b0 >= 100 && b0 <= 150);
        let b1 = calculate_backoff_ms(100, 1);
        assert!(b1 >= 200 && b1 <= 300);
    }
}
