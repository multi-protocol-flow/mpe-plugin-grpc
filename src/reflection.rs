//! Server Reflection 协议发现 gRPC 服务
//!
//! 通过 gRPC Server Reflection 协议或解析 .proto 文件发现服务和方法。
//! 支持 v1 和 v1alpha 两种反射协议，优先使用 v1，Failed时回退到 v1alpha。

use std::collections::HashSet;
use std::sync::Arc;

use crate::types::{GrpcMethodInfo, GrpcServiceInfo};
use tonic::metadata::{MetadataKey, MetadataValue};

use crate::proto_parser;
use crate::tls::create_channel;

/// 单次反射请求的超时时间（秒）
const REFLECTION_TIMEOUT_SECS: u64 = 15;

// ============================================================================
// v1alpha 反射（原始实现，保持不变）
// ============================================================================

/// 通过 Server Reflection v1alpha 协议发现 gRPC 服务
///
/// 连接到 gRPC 服务器后，使用 v1alpha 反射协议:
/// 1. 发送 `ListServices` 获取所有服务名
/// 2. 对每个服务发送 `FileContainingSymbol` 获取文件描述符
/// 3. 递归获取所有传递依赖（通过 `FileByFilename`）
/// 4. 构建 `DescriptorPool` 用于后续动态调用
///
/// 所有网络操作均带超时，避免无限挂起。
async fn reflect_services_v1alpha(
    channel: tonic::transport::Channel,
    metadata: &[(String, String)],
) -> Result<prost_reflect::DescriptorPool, String> {
    use tonic_reflection::pb::v1alpha::{
        server_reflection_client::ServerReflectionClient, server_reflection_request,
        ServerReflectionRequest,
    };

    log::info!("[gRPC Reflection v1alpha] creating reflection client...");

    // 创建反射客户端和双向流的请求通道
    let mut client = ServerReflectionClient::new(channel);
    let (tx, rx) = tokio::sync::mpsc::channel(16);
    let request_stream = tokio_stream::wrappers::ReceiverStream::new(rx);

    // 预发送 ListServices 请求
    log::info!("[gRPC Reflection v1alpha] pre-sending ListServices request...");
    tx.send(ServerReflectionRequest {
        host: String::new(),
        message_request: Some(server_reflection_request::MessageRequest::ListServices(
            String::new(),
        )),
    })
    .await
    .map_err(|e| format!("failed to send ListServices request: {}", e))?;

    // 发起双向流调用（带 metadata 和超时）
    log::info!("[gRPC Reflection v1alpha] starting bidi stream call...");
    let mut request = tonic::Request::new(request_stream);
    set_metadata_on_request(&mut request, metadata)?;

    let response = tokio::time::timeout(
        std::time::Duration::from_secs(REFLECTION_TIMEOUT_SECS),
        client.server_reflection_info(request),
    )
    .await
    .map_err(|_| {
        log::error!("[gRPC Reflection v1alpha] bidi stream connection timed out");
        "Server Reflection v1alpha connection timed out (server may not support Reflection)".to_string()
    })?
    .map_err(|e| {
        log::error!("[gRPC Reflection v1alpha] bidi stream connection failed: {}", e);
        format!(
            "Server Reflection v1alpha connection failed: {} (server may not support Reflection)",
            e
        )
    })?;
    let stream = response.into_inner();
    log::info!("[gRPC Reflection v1alpha] bidi stream established, waiting for ListServices response...");

    collect_reflection_data_v1alpha(stream, tx).await
}

/// v1alpha 反射: 收集文件描述符（从 `ListServices` 之后开始）
async fn collect_reflection_data_v1alpha(
    mut stream: tonic::codec::Streaming<tonic_reflection::pb::v1alpha::ServerReflectionResponse>,
    tx: tokio::sync::mpsc::Sender<tonic_reflection::pb::v1alpha::ServerReflectionRequest>,
) -> Result<prost_reflect::DescriptorPool, String> {
    use tonic_reflection::pb::v1alpha::server_reflection_response;

    // 等待 ListServices 响应
    let list_resp = tokio::time::timeout(
        std::time::Duration::from_secs(REFLECTION_TIMEOUT_SECS),
        stream.message(),
    )
    .await
    .map_err(|_| {
        log::error!("[gRPC Reflection v1alpha] ListServices response timed out");
        "ListServices response timed out (server may not support Reflection)".to_string()
    })?
    .map_err(|e| {
        log::error!(
            "[gRPC Reflection v1alpha] failed to read ListServices response: {}",
            e
        );
        format!("failed to read ListServices response: {}", e)
    })?
    .ok_or_else(|| {
        log::error!("[gRPC Reflection v1alpha] server closed the stream without returning a service list");
        "Server Reflection returned no service list response".to_string()
    })?;

    let services = match list_resp.message_response {
        Some(server_reflection_response::MessageResponse::ListServicesResponse(resp)) => {
            resp.service
        }
        Some(server_reflection_response::MessageResponse::ErrorResponse(err)) => {
            return Err(format!("Server Reflection Error: {}", err.error_message));
        }
        _ => return Err("Server Reflection returned an unexpected response type".to_string()),
    };

    let service_names = filter_services(services);

    if service_names.is_empty() {
        return Err("server does not expose any business gRPC services (only reflection service found)".to_string());
    }

    log::info!(
        "[gRPC Reflection v1alpha] discovered {} services: {:?}",
        service_names.len(),
        service_names
    );

    // 获取文件描述符 + 递归解析依赖
    let all_fds = fetch_file_descriptors_v1alpha(&service_names, &mut stream, &tx).await?;

    // 关闭请求流
    drop(tx);

    build_descriptor_pool(all_fds)
}

/// v1alpha: 获取服务文件描述符并递归解析依赖
async fn fetch_file_descriptors_v1alpha(
    service_names: &[String],
    stream: &mut tonic::codec::Streaming<tonic_reflection::pb::v1alpha::ServerReflectionResponse>,
    tx: &tokio::sync::mpsc::Sender<tonic_reflection::pb::v1alpha::ServerReflectionRequest>,
) -> Result<Vec<prost_types::FileDescriptorProto>, String> {
    use tonic_reflection::pb::v1alpha::{
        server_reflection_request, server_reflection_response, ServerReflectionRequest,
    };

    let mut all_fds: Vec<prost_types::FileDescriptorProto> = Vec::new();
    let mut seen_files: HashSet<String> = HashSet::new();

    // 获取每个服务对应的文件描述符
    for service_name in service_names {
        tx.send(ServerReflectionRequest {
            host: String::new(),
            message_request: Some(
                server_reflection_request::MessageRequest::FileContainingSymbol(
                    service_name.clone(),
                ),
            ),
        })
        .await
        .map_err(|e| format!("failed to send FileContainingSymbol request: {}", e))?;

        let resp = tokio::time::timeout(
            std::time::Duration::from_secs(REFLECTION_TIMEOUT_SECS),
            stream.message(),
        )
        .await
        .map_err(|_| format!("timed out querying file descriptors for service '{}'", service_name))?
        .map_err(|e| format!("failed to query file descriptors for service '{}': {}", service_name, e))?
        .ok_or_else(|| format!("Server Reflection closed the stream while querying service '{}'", service_name))?;

        match resp.message_response {
            Some(server_reflection_response::MessageResponse::FileDescriptorResponse(fdr)) => {
                for fd_bytes in &fdr.file_descriptor_proto {
                    let fd: prost_types::FileDescriptorProto =
                        prost::Message::decode(fd_bytes.as_slice())
                            .map_err(|e| format!("failed to decode FileDescriptorProto: {}", e))?;

                    if let Some(name) = &fd.name {
                        if seen_files.insert(name.clone()) {
                            log::debug!("fetched file descriptor from reflection: {}", name);
                            all_fds.push(fd);
                        }
                    }
                }
            }
            Some(server_reflection_response::MessageResponse::ErrorResponse(err)) => {
                log::warn!(
                    "received error while querying file descriptors for service '{}': {}",
                    service_name,
                    err.error_message
                );
                continue;
            }
            _ => {
                log::warn!("received unexpected response while querying service '{}'", service_name);
                continue;
            }
        }
    }

    // 递归解析传递依赖
    resolve_dependencies_v1alpha(&mut all_fds, &mut seen_files, stream, tx).await?;

    Ok(all_fds)
}

/// v1alpha: 递归解析传递依赖
#[allow(clippy::too_many_arguments)]
async fn resolve_dependencies_v1alpha(
    all_fds: &mut Vec<prost_types::FileDescriptorProto>,
    seen_files: &mut HashSet<String>,
    stream: &mut tonic::codec::Streaming<tonic_reflection::pb::v1alpha::ServerReflectionResponse>,
    tx: &tokio::sync::mpsc::Sender<tonic_reflection::pb::v1alpha::ServerReflectionRequest>,
) -> Result<(), String> {
    use tonic_reflection::pb::v1alpha::{
        server_reflection_request, server_reflection_response, ServerReflectionRequest,
    };

    let mut max_iterations = 50;
    loop {
        max_iterations -= 1;
        if max_iterations <= 0 {
            log::warn!("dependency resolution reached the max iteration limit, some dependencies may be unresolved");
            break;
        }

        let missing_deps: Vec<String> = all_fds
            .iter()
            .flat_map(|fd| fd.dependency.iter().cloned())
            .filter(|dep| !seen_files.contains(dep))
            .collect();

        if missing_deps.is_empty() {
            break;
        }

        log::debug!("found {} unresolved transitive dependencies", missing_deps.len());

        for dep_name in &missing_deps {
            tx.send(ServerReflectionRequest {
                host: String::new(),
                message_request: Some(server_reflection_request::MessageRequest::FileByFilename(
                    dep_name.clone(),
                )),
            })
            .await
            .map_err(|e| format!("failed to send FileByFilename request: {}", e))?;

            let resp = tokio::time::timeout(
                std::time::Duration::from_secs(REFLECTION_TIMEOUT_SECS),
                stream.message(),
            )
            .await
            .map_err(|_| format!("timed out querying dependency file '{}'", dep_name))?;

            let resp = match resp {
                Ok(Some(resp)) => resp,
                Ok(None) => {
                    log::warn!("stream closed while querying dependency file '{}'", dep_name);
                    seen_files.insert(dep_name.clone());
                    continue;
                }
                Err(e) => {
                    log::warn!("failed to query dependency file '{}': {}", dep_name, e);
                    seen_files.insert(dep_name.clone());
                    continue;
                }
            };

            match resp.message_response {
                Some(server_reflection_response::MessageResponse::FileDescriptorResponse(fdr)) => {
                    for fd_bytes in &fdr.file_descriptor_proto {
                        let fd: prost_types::FileDescriptorProto =
                            prost::Message::decode(fd_bytes.as_slice()).map_err(|e| {
                                format!("failed to decode dependency FileDescriptorProto: {}", e)
                            })?;
                        if let Some(name) = &fd.name {
                            if seen_files.insert(name.clone()) {
                                log::debug!("fetched dependency file from reflection: {}", name);
                                all_fds.push(fd);
                            }
                        }
                    }
                }
                Some(server_reflection_response::MessageResponse::ErrorResponse(err)) => {
                    log::warn!(
                        "received error while querying dependency file '{}': {}",
                        dep_name,
                        err.error_message
                    );
                    seen_files.insert(dep_name.clone());
                }
                _ => {
                    seen_files.insert(dep_name.clone());
                }
            }
        }
    }

    Ok(())
}

// ============================================================================
// v1 反射
// ============================================================================

/// 通过 Server Reflection v1 协议发现 gRPC 服务
///
/// 与 v1alpha 相同的流程，但使用 v1 协议包。
async fn reflect_services_v1(
    channel: tonic::transport::Channel,
    metadata: &[(String, String)],
) -> Result<prost_reflect::DescriptorPool, String> {
    use tonic_reflection::pb::v1::{
        server_reflection_client::ServerReflectionClient, server_reflection_request,
        server_reflection_response, ServerReflectionRequest,
    };

    log::info!("[gRPC Reflection v1] creating reflection client...");

    let mut client = ServerReflectionClient::new(channel);
    let (tx, rx) = tokio::sync::mpsc::channel(16);
    let request_stream = tokio_stream::wrappers::ReceiverStream::new(rx);

    // 预发送 ListServices 请求
    log::info!("[gRPC Reflection v1] pre-sending ListServices request...");
    tx.send(ServerReflectionRequest {
        host: String::new(),
        message_request: Some(server_reflection_request::MessageRequest::ListServices(
            String::new(),
        )),
    })
    .await
    .map_err(|e| format!("failed to send ListServices request: {}", e))?;

    // 发起双向流调用（带 metadata 和超时）
    log::info!("[gRPC Reflection v1] starting bidi stream call...");
    let mut request = tonic::Request::new(request_stream);
    set_metadata_on_request(&mut request, metadata)?;

    let response = tokio::time::timeout(
        std::time::Duration::from_secs(REFLECTION_TIMEOUT_SECS),
        client.server_reflection_info(request),
    )
    .await
    .map_err(|_| {
        log::error!("[gRPC Reflection v1] bidi stream connection timed out");
        "Server Reflection v1 connection timed out".to_string()
    })?
    .map_err(|e| {
        log::error!("[gRPC Reflection v1] bidi stream connection failed: {}", e);
        format!("Server Reflection v1 connection failed: {}", e)
    })?;
    let mut stream = response.into_inner();
    log::info!("[gRPC Reflection v1] bidi stream established, waiting for ListServices response...");

    // 等待 ListServices 响应
    let list_resp = tokio::time::timeout(
        std::time::Duration::from_secs(REFLECTION_TIMEOUT_SECS),
        stream.message(),
    )
    .await
    .map_err(|_| {
        log::error!("[gRPC Reflection v1] ListServices response timed out");
        "ListServices v1 response timed out".to_string()
    })?
    .map_err(|e| {
        log::error!("[gRPC Reflection v1] failed to read ListServices response: {}", e);
        format!("failed to read ListServices v1 response: {}", e)
    })?
    .ok_or_else(|| {
        log::error!("[gRPC Reflection v1] server closed the stream without returning a service list");
        "Server Reflection v1 returned no service list response".to_string()
    })?;

    let services = match list_resp.message_response {
        Some(server_reflection_response::MessageResponse::ListServicesResponse(resp)) => {
            resp.service
        }
        Some(server_reflection_response::MessageResponse::ErrorResponse(err)) => {
            return Err(format!("Server Reflection v1 Error: {}", err.error_message));
        }
        _ => return Err("Server Reflection v1 returned an unexpected response type".to_string()),
    };

    let service_names: Vec<String> = services
        .into_iter()
        .map(|s| s.name)
        .filter(|name| !is_reflection_service(name))
        .collect();

    if service_names.is_empty() {
        return Err("server does not expose any business gRPC services (only reflection service found)".to_string());
    }

    log::info!(
        "[gRPC Reflection v1] discovered {} services: {:?}",
        service_names.len(),
        service_names
    );

    // 获取文件描述符 + 递归解析依赖
    let all_fds = fetch_file_descriptors_v1(&service_names, &mut stream, &tx).await?;

    drop(tx);
    build_descriptor_pool(all_fds)
}

/// v1: 获取服务文件描述符并递归解析依赖
async fn fetch_file_descriptors_v1(
    service_names: &[String],
    stream: &mut tonic::codec::Streaming<tonic_reflection::pb::v1::ServerReflectionResponse>,
    tx: &tokio::sync::mpsc::Sender<tonic_reflection::pb::v1::ServerReflectionRequest>,
) -> Result<Vec<prost_types::FileDescriptorProto>, String> {
    use tonic_reflection::pb::v1::{
        server_reflection_request, server_reflection_response, ServerReflectionRequest,
    };

    let mut all_fds: Vec<prost_types::FileDescriptorProto> = Vec::new();
    let mut seen_files: HashSet<String> = HashSet::new();

    for service_name in service_names {
        tx.send(ServerReflectionRequest {
            host: String::new(),
            message_request: Some(
                server_reflection_request::MessageRequest::FileContainingSymbol(
                    service_name.clone(),
                ),
            ),
        })
        .await
        .map_err(|e| format!("failed to send FileContainingSymbol request: {}", e))?;

        let resp = tokio::time::timeout(
            std::time::Duration::from_secs(REFLECTION_TIMEOUT_SECS),
            stream.message(),
        )
        .await
        .map_err(|_| format!("timed out querying file descriptors for service '{}'", service_name))?
        .map_err(|e| format!("failed to query file descriptors for service '{}': {}", service_name, e))?
        .ok_or_else(|| {
            format!(
                "Server Reflection v1 closed the stream while querying service '{}'",
                service_name
            )
        })?;

        match resp.message_response {
            Some(server_reflection_response::MessageResponse::FileDescriptorResponse(fdr)) => {
                for fd_bytes in &fdr.file_descriptor_proto {
                    let fd: prost_types::FileDescriptorProto =
                        prost::Message::decode(fd_bytes.as_slice())
                            .map_err(|e| format!("failed to decode FileDescriptorProto: {}", e))?;

                    if let Some(name) = &fd.name {
                        if seen_files.insert(name.clone()) {
                            log::debug!("fetched file descriptor from reflection: {}", name);
                            all_fds.push(fd);
                        }
                    }
                }
            }
            Some(server_reflection_response::MessageResponse::ErrorResponse(err)) => {
                log::warn!(
                    "received error while querying file descriptors for service '{}': {}",
                    service_name,
                    err.error_message
                );
                continue;
            }
            _ => {
                log::warn!("received unexpected response while querying service '{}'", service_name);
                continue;
            }
        }
    }

    // 递归解析传递依赖
    resolve_dependencies_v1(&mut all_fds, &mut seen_files, stream, tx).await?;

    Ok(all_fds)
}

/// v1: 递归解析传递依赖
async fn resolve_dependencies_v1(
    all_fds: &mut Vec<prost_types::FileDescriptorProto>,
    seen_files: &mut HashSet<String>,
    stream: &mut tonic::codec::Streaming<tonic_reflection::pb::v1::ServerReflectionResponse>,
    tx: &tokio::sync::mpsc::Sender<tonic_reflection::pb::v1::ServerReflectionRequest>,
) -> Result<(), String> {
    use tonic_reflection::pb::v1::{
        server_reflection_request, server_reflection_response, ServerReflectionRequest,
    };

    let mut max_iterations = 50;
    loop {
        max_iterations -= 1;
        if max_iterations <= 0 {
            log::warn!("dependency resolution reached the max iteration limit, some dependencies may be unresolved");
            break;
        }

        let missing_deps: Vec<String> = all_fds
            .iter()
            .flat_map(|fd| fd.dependency.iter().cloned())
            .filter(|dep| !seen_files.contains(dep))
            .collect();

        if missing_deps.is_empty() {
            break;
        }

        log::debug!("found {} unresolved transitive dependencies", missing_deps.len());

        for dep_name in &missing_deps {
            tx.send(ServerReflectionRequest {
                host: String::new(),
                message_request: Some(server_reflection_request::MessageRequest::FileByFilename(
                    dep_name.clone(),
                )),
            })
            .await
            .map_err(|e| format!("failed to send FileByFilename request: {}", e))?;

            let resp = tokio::time::timeout(
                std::time::Duration::from_secs(REFLECTION_TIMEOUT_SECS),
                stream.message(),
            )
            .await
            .map_err(|_| format!("timed out querying dependency file '{}'", dep_name))?;

            let resp = match resp {
                Ok(Some(resp)) => resp,
                Ok(None) => {
                    log::warn!("stream closed while querying dependency file '{}'", dep_name);
                    seen_files.insert(dep_name.clone());
                    continue;
                }
                Err(e) => {
                    log::warn!("failed to query dependency file '{}': {}", dep_name, e);
                    seen_files.insert(dep_name.clone());
                    continue;
                }
            };

            match resp.message_response {
                Some(server_reflection_response::MessageResponse::FileDescriptorResponse(fdr)) => {
                    for fd_bytes in &fdr.file_descriptor_proto {
                        let fd: prost_types::FileDescriptorProto =
                            prost::Message::decode(fd_bytes.as_slice()).map_err(|e| {
                                format!("failed to decode dependency FileDescriptorProto: {}", e)
                            })?;
                        if let Some(name) = &fd.name {
                            if seen_files.insert(name.clone()) {
                                log::debug!("fetched dependency file from reflection: {}", name);
                                all_fds.push(fd);
                            }
                        }
                    }
                }
                Some(server_reflection_response::MessageResponse::ErrorResponse(err)) => {
                    log::warn!(
                        "received error while querying dependency file '{}': {}",
                        dep_name,
                        err.error_message
                    );
                    seen_files.insert(dep_name.clone());
                }
                _ => {
                    seen_files.insert(dep_name.clone());
                }
            }
        }
    }

    Ok(())
}

// ============================================================================
// 公共 API
// ============================================================================

/// 通过 Server Reflection 协议发现 gRPC 服务（v1 优先，自动回退到 v1alpha）
///
/// 策略: 先尝试 v1 协议，如果Failed（连接Error或 UNIMPLEMENTED），回退到 v1alpha。
/// 支持在反射请求中附带 metadata，用于需要认证的反射服务。
pub(crate) async fn reflect_services(
    channel: tonic::transport::Channel,
    metadata: Vec<(String, String)>,
) -> Result<prost_reflect::DescriptorPool, String> {
    // 先尝试 v1
    log::info!("[gRPC Reflection] trying v1 protocol...");
    match reflect_services_v1(channel.clone(), &metadata).await {
        Ok(pool) => {
            log::info!("[gRPC Reflection] v1 protocol succeeded");
            return Ok(pool);
        }
        Err(e) => {
            // 判断是否应该回退: 连接Error或 UNIMPLEMENTED 可回退
            let should_fallback = is_fallback_eligible_error(&e);
            if !should_fallback {
                log::error!("[gRPC Reflection] v1 failed (not fallback-eligible): {}", e);
                return Err(e);
            }
            log::info!(
                "[gRPC Reflection] v1 failed (fallback-eligible): {}, trying v1alpha...",
                e
            );
        }
    }

    // 回退到 v1alpha
    match reflect_services_v1alpha(channel, &metadata).await {
        Ok(pool) => {
            log::info!("[gRPC Reflection] v1alpha fallback succeeded");
            Ok(pool)
        }
        Err(e) => {
            log::error!("[gRPC Reflection] v1alpha also failed: {}", e);
            Err(format!(
                "Server Reflection failed (tried v1 and v1alpha): {}",
                e
            ))
        }
    }
}

/// 判断 v1 Error是否可以回退到 v1alpha
///
/// 默认回退 — 绝大多数Error都可以尝试 v1alpha
fn is_fallback_eligible_error(_error: &str) -> bool {
    true
}

// ============================================================================
// 辅助函数
// ============================================================================

/// 过滤掉反射服务自身，返回业务服务名列表
fn filter_services(services: Vec<tonic_reflection::pb::v1alpha::ServiceResponse>) -> Vec<String> {
    services
        .into_iter()
        .map(|s| s.name)
        .filter(|name| !is_reflection_service(name))
        .collect()
}

/// 设置 metadata 到 tonic 请求
fn set_metadata_on_request<T>(
    request: &mut tonic::Request<T>,
    metadata: &[(String, String)],
) -> Result<(), String> {
    for (key, value) in metadata {
        let meta_key = MetadataKey::from_bytes(key.as_bytes())
            .map_err(|e| format!("invalid reflection metadata key '{}': {:?}", key, e))?;
        let meta_val = MetadataValue::try_from(value.as_str())
            .map_err(|e| format!("invalid reflection metadata value '{}': {:?}", value, e))?;
        request.metadata_mut().insert(meta_key, meta_val);
    }
    Ok(())
}

/// 从文件描述符列表构建 `DescriptorPool`
fn build_descriptor_pool(
    all_fds: Vec<prost_types::FileDescriptorProto>,
) -> Result<prost_reflect::DescriptorPool, String> {
    if all_fds.is_empty() {
        return Err("Server Reflection returned no file descriptors".to_string());
    }

    let fds = prost_types::FileDescriptorSet { file: all_fds };
    prost_reflect::DescriptorPool::from_file_descriptor_set(fds)
        .map_err(|e| format!("failed to build DescriptorPool from reflection data: {}", e))
}

/// 判断是否为 gRPC 反射服务（应从结果中过滤）
pub(crate) fn is_reflection_service(name: &str) -> bool {
    matches!(
        name,
        "grpc.reflection.v1.ServerReflection" | "grpc.reflection.v1alpha.ServerReflection"
    )
}

/// 通过 Server Reflection 协议发现 gRPC 服务（不创建持久连接）
///
/// 用于设计时预发现服务，仅创建临时连接进行反射查询，
/// 返回服务列表但不将连接存入连接池。
///
/// 支持 `reflection_metadata` 参数，用于向需要认证的反射服务发送 metadata。
pub async fn discover_services_via_reflection(
    url: &str,
    use_tls: bool,
    tls_skip_verify: bool,
    connect_timeout_ms: u64,
    tls_ca_cert: Option<&str>,
    tls_client_cert: Option<&str>,
    tls_client_key: Option<&str>,
    tls_server_name_override: Option<&str>,
    reflection_metadata: Vec<(String, String)>,
) -> Result<Vec<GrpcServiceInfo>, String> {
    // 1. 构造 URL
    let url_with_scheme = if url.starts_with("http://") || url.starts_with("https://") {
        url.to_string()
    } else if use_tls {
        format!("https://{}", url)
    } else {
        format!("http://{}", url)
    };

    log::info!(
        "[gRPC] starting reflection discovery: url={}, use_tls={}, tls_skip_verify={}, timeout={}ms, metadata_count={}",
        url_with_scheme,
        use_tls,
        tls_skip_verify,
        connect_timeout_ms,
        reflection_metadata.len()
    );

    // 2. 创建 channel（含 TLS 配置，设计时不启用 keepalive）
    let channel = create_channel(
        &url_with_scheme,
        use_tls,
        tls_skip_verify,
        connect_timeout_ms,
        tls_ca_cert,
        tls_client_cert,
        tls_client_key,
        tls_server_name_override,
        None,
        None,
        None,
    )?;

    // 3. 通过反射发现服务（v1 优先，自动回退到 v1alpha）
    let pool = reflect_services(channel, reflection_metadata).await?;

    // 4. 提取服务信息
    let services = proto_parser::list_services(&pool);
    let message_definitions = Arc::new(proto_parser::get_message_definitions(&pool));
    Ok(services
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
        .collect())
}

/// 通过 Server Reflection 协议生成请求骨架（不创建持久连接）
///
/// 创建临时连接，通过反射获取 `DescriptorPool`，
/// 从中生成指定服务和方法的请求 JSON 骨架。
/// 适用于未提供 proto 文件的 Reflection 场景。
pub async fn generate_skeleton_via_reflection(
    url: &str,
    use_tls: bool,
    tls_skip_verify: bool,
    connect_timeout_ms: u64,
    tls_ca_cert: Option<&str>,
    tls_client_cert: Option<&str>,
    tls_client_key: Option<&str>,
    tls_server_name_override: Option<&str>,
    reflection_metadata: Vec<(String, String)>,
    service_name: &str,
    method_name: &str,
) -> Result<String, String> {
    // 1. 构造 URL
    let url_with_scheme = if url.starts_with("http://") || url.starts_with("https://") {
        url.to_string()
    } else if use_tls {
        format!("https://{}", url)
    } else {
        format!("http://{}", url)
    };

    log::info!(
        "[gRPC] generating skeleton via reflection: url={}, service={}, method={}",
        url_with_scheme,
        service_name,
        method_name
    );

    // 2. 创建 channel（含 TLS 配置，设计时不启用 keepalive）
    let channel = create_channel(
        &url_with_scheme,
        use_tls,
        tls_skip_verify,
        connect_timeout_ms,
        tls_ca_cert,
        tls_client_cert,
        tls_client_key,
        tls_server_name_override,
        None,
        None,
        None,
    )?;

    // 3. 通过反射获取 DescriptorPool
    let pool = reflect_services(channel, reflection_metadata).await?;

    // 4. 从 pool 生成骨架
    proto_parser::generate_skeleton_from_pool(&pool, service_name, method_name)
}

/// 通过解析 Proto 文件发现 gRPC 服务（不需要连接服务器）
///
/// 用于设计时预发现服务，解析用户提供的 .proto 文件，
/// 返回服务列表。
pub fn discover_services_via_proto(
    proto_files: &[(String, String)],
) -> Result<Vec<GrpcServiceInfo>, String> {
    let files: Vec<proto_parser::ProtoFile> = proto_files
        .iter()
        .map(|(path, content)| proto_parser::ProtoFile {
            path: path.clone(),
            content: content.clone(),
        })
        .collect();

    let pool = proto_parser::parse_proto_files(&files)
        .map_err(|e| format!("failed to parse proto files: {}", e))?;

    let services = proto_parser::list_services(&pool);
    let message_definitions = Arc::new(proto_parser::get_message_definitions(&pool));
    Ok(services
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
        .collect())
}
