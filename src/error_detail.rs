//! gRPC Error详情解码模块
//!
//! 从 tonic Status 解码 `google.rpc.Status` 中的 `google.protobuf.Any` Error详情，
//! 使用 prost-reflect 的 DynamicMessage 进行动态解码。
//! （从宿主 flow-engine-grpc 原样迁移，GrpcErrorDetail 改用本地 types）

use crate::types::GrpcErrorDetail;
use prost::Message;
use std::sync::OnceLock;

/// `google.rpc.Status` proto wrapper（用于解码 tonic Status details）
///
/// 直接使用 prost 结构体解码，避免编译 proto 文件的运行时开销。
/// 字段布局与 `google.rpc.Status` protobuf 定义一致。
#[derive(Clone, Message)]
struct RpcStatus {
    #[prost(int32, tag = "1")]
    code: i32,
    #[prost(string, tag = "2")]
    message: String,
    #[prost(message, repeated, tag = "3")]
    details: Vec<prost_types::Any>,
}

/// 全局Error详情类型 DescriptorPool（包含 `google.rpc.*` Error类型）
///
/// 首次使用时通过 protox 编译内联 proto 定义，后续复用缓存。
static ERROR_DETAIL_POOL: OnceLock<prost_reflect::DescriptorPool> = OnceLock::new();

/// `google.rpc.*` Error详情类型的 proto 定义
///
/// 包含 gRPC 标准Error详情类型:
/// - `RetryInfo` - 重试信息
/// - `DebugInfo` - 调试信息
/// - `QuotaFailure` - 配额Failed
/// - `BadRequest` - 请求Error
/// - `ResourceInfo` - 资源信息
/// - `Help` - 帮助链接
/// - `LocalizedMessage` - 本地化消息
/// - `PreconditionFailure` - 前置条件Failed
/// - `ErrorInfo` - Error信息
const ERROR_DETAILS_PROTO: &str = r#"
syntax = "proto3";
package google.rpc;

import "google/protobuf/duration.proto";

message RetryInfo {
  google.protobuf.Duration retry_delay = 1;
}

message DebugInfo {
  repeated string stack_entries = 1;
  string detail = 2;
}

message QuotaFailure {
  message Violation {
    string subject = 1;
    string description = 2;
  }
  repeated Violation violations = 1;
}

message BadRequest {
  message FieldViolation {
    string field = 1;
    string description = 2;
  }
  repeated FieldViolation field_violations = 1;
}

message ResourceInfo {
  string resource_type = 1;
  string resource_name = 2;
  string owner = 3;
  string description = 4;
}

message Help {
  message Link {
    string description = 1;
    string url = 2;
  }
  repeated Link links = 1;
}

message LocalizedMessage {
  string locale = 1;
  string message = 2;
}

message PreconditionFailure {
  message Violation {
    string type = 1;
    string subject = 2;
    string description = 3;
  }
  repeated Violation violations = 1;
}

message ErrorInfo {
  string reason = 1;
  string domain = 2;
  map<string, string> metadata = 3;
}
"#;

/// 获取Error详情类型 `DescriptorPool`
///
/// 编译 `google.rpc.*` proto 定义并缓存结果。
/// Failed时返回空 DescriptorPool（Error详情将以原始十六进制展示）。
fn get_error_detail_pool() -> &'static prost_reflect::DescriptorPool {
    ERROR_DETAIL_POOL.get_or_init(|| {
        let temp_dir = match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(e) => {
                log::warn!("[gRPC ErrorDetail] failed to create temp directory: {}", e);
                return prost_reflect::DescriptorPool::new();
            }
        };

        // 创建 google/rpc/ 目录结构
        let rpc_dir = temp_dir.path().join("google").join("rpc");
        if let Err(e) = std::fs::create_dir_all(&rpc_dir) {
            log::warn!("[gRPC ErrorDetail] failed to create directory: {}", e);
            return prost_reflect::DescriptorPool::new();
        }

        // 写入 proto 文件
        let proto_path = rpc_dir.join("error_details.proto");
        if let Err(e) = std::fs::write(&proto_path, ERROR_DETAILS_PROTO) {
            log::warn!("[gRPC ErrorDetail] failed to write proto file: {}", e);
            return prost_reflect::DescriptorPool::new();
        }

        // 使用 protox 编译（well-known types 如 google.protobuf.Duration 自动解析）
        match protox::compile(["google/rpc/error_details.proto"], [temp_dir.path()]) {
            Ok(fds) => match prost_reflect::DescriptorPool::from_file_descriptor_set(fds) {
                Ok(pool) => {
                    log::debug!(
                        "[gRPC ErrorDetail] error details DescriptorPool built successfully, contains {} message types",
                        pool.all_messages().count()
                    );
                    pool
                }
                Err(e) => {
                    log::warn!("[gRPC ErrorDetail] failed to build DescriptorPool: {}", e);
                    prost_reflect::DescriptorPool::new()
                }
            },
            Err(e) => {
                log::warn!("[gRPC ErrorDetail] failed to compile proto files: {}", e);
                prost_reflect::DescriptorPool::new()
            }
        }
    })
}

/// 从 type URL 提取类型全名
///
/// `type.googleapis.com/google.rpc.RetryInfo` → `google.rpc.RetryInfo`
fn extract_type_name(type_url: &str) -> &str {
    type_url.rsplit('/').next().unwrap_or(type_url)
}

/// 将字节数组编码为十六进制字符串
fn bytes_to_hex(bytes: &[u8]) -> String {
    let mut hex = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        hex.push_str(&format!("{:02x}", b));
    }
    hex
}

/// Error详情分隔标记（用于在Error字符串中嵌入 JSON）
pub(crate) const ERROR_DETAILS_MARKER: &str = "\n---GRPC_ERROR_DETAILS---\n";

/// 从 tonic Status 解码Error详情
///
/// 解码流程:
/// 1. 将 `Status.details()` 解码为 `google.rpc.Status`
/// 2. 遍历 `details` 字段中的每个 `google.protobuf.Any`
/// 3. 从 `type_url` 提取类型名，尝试在 `DescriptorPool` 中查找
/// 4. 使用 `DynamicMessage` 解码为 JSON
/// 5. 查找Failed时返回原始十六进制
///
/// # 参数
/// - `status` - tonic Status Error
/// - `connection_pool` - 连接的 DescriptorPool（包含用户 proto 定义的类型）
pub(crate) fn decode_error_details(
    status: &tonic::Status,
    connection_pool: &prost_reflect::DescriptorPool,
) -> Vec<GrpcErrorDetail> {
    let details_bytes = status.details();
    if details_bytes.is_empty() {
        return vec![];
    }

    // 解码 google.rpc.Status
    let rpc_status = match RpcStatus::decode(details_bytes) {
        Ok(s) => s,
        Err(_) => return vec![],
    };

    if rpc_status.details.is_empty() {
        return vec![];
    }

    let detail_pool = get_error_detail_pool();

    let mut result = Vec::with_capacity(rpc_status.details.len());
    for any in &rpc_status.details {
        let type_name = extract_type_name(&any.type_url).to_string();

        // 优先从连接的 DescriptorPool 解码（用户定义的类型）
        // 回退到Error详情 Pool（google.rpc.* 标准Error类型）
        let decoded = try_decode_any(&any.value, &type_name, connection_pool)
            .or_else(|| try_decode_any(&any.value, &type_name, detail_pool));

        let (decoded_json, raw_hex) = match decoded {
            Some(json) => (Some(json), None),
            None if any.value.is_empty() => (None, None),
            None => (None, Some(bytes_to_hex(&any.value))),
        };

        result.push(GrpcErrorDetail {
            type_url: any.type_url.clone(),
            decoded: decoded_json,
            raw_hex,
        });
    }

    result
}

/// 尝试用指定的 `DescriptorPool` 解码 Any 的 value 字段
fn try_decode_any(
    value_bytes: &[u8],
    type_name: &str,
    pool: &prost_reflect::DescriptorPool,
) -> Option<serde_json::Value> {
    let msg_desc = pool.get_message_by_name(type_name)?;
    let dynamic_msg = prost_reflect::DynamicMessage::decode(msg_desc, value_bytes).ok()?;
    crate::helpers::dynamic_message_to_json(&dynamic_msg).ok()
}

/// 格式化 gRPC Error消息（包含解码后的Error详情）
///
/// 当 Status 包含非空 details 时，在Error消息后追加 JSON 编码的Error详情。
/// 使用 [`ERROR_DETAILS_MARKER`] 分隔人类可读消息和 JSON 详情。
pub(crate) fn format_grpc_error(
    prefix: &str,
    status: &tonic::Status,
    pool: &prost_reflect::DescriptorPool,
) -> String {
    let details = decode_error_details(status, pool);
    let base_msg = format!("{}: {} ({})", prefix, status.message(), status.code());
    if details.is_empty() {
        base_msg
    } else {
        let details_json = serde_json::to_string(&details).unwrap_or_default();
        format!("{}{}{}", base_msg, ERROR_DETAILS_MARKER, details_json)
    }
}

/// `从Error字符串中解析Error消息和Error详情`
///
/// 返回 (人类可读的Error消息, 解码后的Error详情列表)。
/// 如果Error字符串不包含详情标记，返回完整字符串和空列表。
pub(crate) fn parse_error_details(error: &str) -> (&str, Vec<GrpcErrorDetail>) {
    if let Some(pos) = error.find(ERROR_DETAILS_MARKER) {
        let message = &error[..pos];
        let details_json = &error[pos + ERROR_DETAILS_MARKER.len()..];
        let details: Vec<GrpcErrorDetail> =
            serde_json::from_str(details_json.trim()).unwrap_or_default();
        (message.trim(), details)
    } else {
        (error, vec![])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_type_name() {
        assert_eq!(
            extract_type_name("type.googleapis.com/google.rpc.RetryInfo"),
            "google.rpc.RetryInfo"
        );
        assert_eq!(
            extract_type_name("google.rpc.RetryInfo"),
            "google.rpc.RetryInfo"
        );
        assert_eq!(extract_type_name(""), "");
    }

    #[test]
    fn test_bytes_to_hex() {
        assert_eq!(bytes_to_hex(&[]), "");
        assert_eq!(bytes_to_hex(&[0x00]), "00");
        assert_eq!(bytes_to_hex(&[0xff]), "ff");
        assert_eq!(bytes_to_hex(&[0x0a, 0x1b, 0x2c]), "0a1b2c");
    }

    #[test]
    fn test_parse_error_details_no_marker() {
        let error = "gRPC Unary 调用Failed: not found (NOT_FOUND)";
        let (msg, details) = parse_error_details(error);
        assert_eq!(msg, error);
        assert!(details.is_empty());
    }

    #[test]
    fn test_parse_error_details_with_marker() {
        let json = r#"[{"type_url":"type.googleapis.com/google.rpc.RetryInfo","decoded":{"retry_delay":"1.0s"},"raw_hex":null}]"#;
        let error = format!(
            "gRPC 调用Failed: rate limited (RESOURCE_EXHAUSTED){}{}",
            ERROR_DETAILS_MARKER, json
        );
        let (msg, details) = parse_error_details(&error);
        assert_eq!(msg, "gRPC 调用Failed: rate limited (RESOURCE_EXHAUSTED)");
        assert_eq!(details.len(), 1);
        assert_eq!(
            details[0].type_url,
            "type.googleapis.com/google.rpc.RetryInfo"
        );
        assert!(details[0].decoded.is_some());
        assert!(details[0].raw_hex.is_none());
    }

    #[test]
    fn test_parse_error_details_invalid_json() {
        let error = format!("some error{}invalid json", ERROR_DETAILS_MARKER);
        let (msg, details) = parse_error_details(&error);
        assert_eq!(msg, "some error");
        assert!(details.is_empty());
    }

    #[test]
    fn test_format_grpc_error_empty_details() {
        let status = tonic::Status::new(tonic::Code::NotFound, "not found");
        let pool = prost_reflect::DescriptorPool::new();
        let result = format_grpc_error("gRPC 调用Failed", &status, &pool);
        assert!(result.starts_with("gRPC 调用Failed: not found"));
        assert!(!result.contains(ERROR_DETAILS_MARKER));
    }

    #[test]
    fn test_format_grpc_error_with_details() {
        let rpc_status = RpcStatus {
            code: 8,
            message: "resource exhausted".to_string(),
            details: vec![prost_types::Any {
                type_url: "type.googleapis.com/google.rpc.RetryInfo".to_string(),
                value: vec![],
            }],
        };
        let mut details_buf = Vec::new();
        rpc_status
            .encode(&mut details_buf)
            .unwrap_or_else(|e| panic!("编码应Success: {e}"));

        let status = tonic::Status::with_details(
            tonic::Code::ResourceExhausted,
            "resource exhausted",
            details_buf.into(),
        );

        let pool = prost_reflect::DescriptorPool::new();
        let result = format_grpc_error("gRPC 调用Failed", &status, &pool);

        assert!(result.contains(ERROR_DETAILS_MARKER));
        let (msg, details) = parse_error_details(&result);
        assert!(msg.contains("resource exhausted"));
        assert_eq!(details.len(), 1);
        assert_eq!(
            details[0].type_url,
            "type.googleapis.com/google.rpc.RetryInfo"
        );
    }

    #[test]
    fn test_decode_error_details_empty_status() {
        let status = tonic::Status::new(tonic::Code::Ok, "");
        let pool = prost_reflect::DescriptorPool::new();
        let details = decode_error_details(&status, &pool);
        assert!(details.is_empty());
    }

    #[test]
    fn test_decode_error_details_with_multiple_anys() {
        let rpc_status = RpcStatus {
            code: 3,
            message: "bad request".to_string(),
            details: vec![
                prost_types::Any {
                    type_url: "type.googleapis.com/google.rpc.BadRequest".to_string(),
                    value: vec![],
                },
                prost_types::Any {
                    type_url: "type.googleapis.com/google.rpc.Help".to_string(),
                    value: vec![],
                },
            ],
        };
        let mut details_buf = Vec::new();
        rpc_status
            .encode(&mut details_buf)
            .unwrap_or_else(|e| panic!("编码应Success: {e}"));

        let status = tonic::Status::with_details(
            tonic::Code::InvalidArgument,
            "bad request",
            details_buf.into(),
        );

        let pool = get_error_detail_pool();
        let details = decode_error_details(&status, pool);
        assert_eq!(details.len(), 2);
    }

    #[test]
    fn test_get_error_detail_pool() {
        let pool = get_error_detail_pool();
        for name in [
            "google.rpc.RetryInfo",
            "google.rpc.DebugInfo",
            "google.rpc.QuotaFailure",
            "google.rpc.BadRequest",
            "google.rpc.ResourceInfo",
            "google.rpc.Help",
            "google.rpc.LocalizedMessage",
            "google.rpc.PreconditionFailure",
            "google.rpc.ErrorInfo",
        ] {
            assert!(pool.get_message_by_name(name).is_some(), "应包含 {name}");
        }
    }

    #[test]
    fn test_rpc_status_decode_roundtrip() {
        let original = RpcStatus {
            code: 5,
            message: "test error".to_string(),
            details: vec![prost_types::Any {
                type_url: "type.googleapis.com/test.Message".to_string(),
                value: vec![0x0a, 0x05, b'h', b'e', b'l', b'l', b'o'],
            }],
        };
        let mut buf = Vec::new();
        original
            .encode(&mut buf)
            .unwrap_or_else(|e| panic!("编码应Success: {e}"));
        let decoded =
            RpcStatus::decode(buf.as_slice()).unwrap_or_else(|e| panic!("解码应Success: {e}"));
        assert_eq!(decoded.code, 5);
        assert_eq!(decoded.message, "test error");
        assert_eq!(decoded.details.len(), 1);
        assert_eq!(
            decoded.details[0].type_url,
            "type.googleapis.com/test.Message"
        );
        assert_eq!(
            decoded.details[0].value,
            vec![0x0a, 0x05, b'h', b'e', b'l', b'l', b'o']
        );
    }
}
