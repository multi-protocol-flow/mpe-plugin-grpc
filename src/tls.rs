//! TLS 辅助函数
//!
//! 提供 gRPC 连接的 TLS 配置，支持以下模式：
//! - **明文**: 不使用 TLS
//! - **标准 TLS**: webpki-roots 根证书
//! - **自定义 CA**: 加载自定义 CA 证书
//! - **mTLS**: 客户端证书认证
//! - **SNI 覆盖**: 自定义 TLS 服务器名称
//! - **跳过验证**: 不验证服务端证书（仅开发/测试）
//! （从宿主 flow-engine-grpc 原样迁移）

use std::sync::Arc;

/// 不验证任何证书的 ServerCertVerifier（仅用于 `tls_skip_verify=true`）
///
/// **警告**: 仅用于开发/测试环境，切勿在生产环境使用。
#[derive(Debug)]
pub(crate) struct NoVerifier;

impl rustls::client::danger::ServerCertVerifier for NoVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![
            rustls::SignatureScheme::RSA_PKCS1_SHA256,
            rustls::SignatureScheme::RSA_PKCS1_SHA384,
            rustls::SignatureScheme::RSA_PKCS1_SHA512,
            rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
            rustls::SignatureScheme::ECDSA_NISTP384_SHA384,
            rustls::SignatureScheme::ECDSA_NISTP521_SHA512,
            rustls::SignatureScheme::RSA_PSS_SHA256,
            rustls::SignatureScheme::RSA_PSS_SHA384,
            rustls::SignatureScheme::RSA_PSS_SHA512,
            rustls::SignatureScheme::ED25519,
            rustls::SignatureScheme::ED448,
        ]
    }
}

/// 确保已安装 CryptoProvider（幂等，多次调用安全）
fn ensure_crypto_provider() {
    // install_default 只在首次调用时安装，后续调用返回 Err 但不影响程序
    let _ = rustls::crypto::ring::default_provider().install_default();
}

/// 配置端点的 TLS（标准验证模式）
///
/// 使用 tonic 内置的 webpki-roots 根证书信任链。
fn apply_standard_tls(
    endpoint: tonic::transport::Endpoint,
) -> Result<tonic::transport::Endpoint, String> {
    let tls_config = tonic::transport::ClientTlsConfig::new().with_webpki_roots();
    endpoint
        .tls_config(tls_config)
        .map_err(|e| format!("TLS configuration failed: {}", e))
}

/// 从 PEM 文件内容加载证书链
///
/// 读取 PEM 格式的证书数据，解析为 `CertificateDer` 列表。
fn load_certs_from_pem(
    pem_data: &str,
) -> Result<Vec<rustls::pki_types::CertificateDer<'static>>, String> {
    let mut reader = std::io::BufReader::new(pem_data.as_bytes());
    rustls_pemfile::certs(&mut reader)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("Failed to parse certificate PEM: {}", e))
}

/// 从 PEM 文件内容加载私钥
///
/// 支持 PKCS#8 和 PKCS#1 格式的 RSA/EC 私钥。
fn load_key_from_pem(pem_data: &str) -> Result<rustls::pki_types::PrivateKeyDer<'static>, String> {
    let mut reader = std::io::BufReader::new(pem_data.as_bytes());

    // 先尝试 PKCS#8 格式
    if let Some(key) = rustls_pemfile::private_key(&mut reader)
        .map_err(|e| format!("Failed to parse private key PEM: {}", e))?
    {
        return Ok(key);
    }

    Err("No valid private key found (supports PKCS#8/PKCS#1 format)".to_string())
}

/// 从文件路径读取内容
fn read_file_content(path: &str) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| format!("Failed to read file '{}': {}", path, e))
}

/// 构建包含自定义 CA 和/或客户端证书的 rustls `ClientConfig`
///
/// - 自定义 CA 证书会被添加到 RootCertStore（与系统根证书共存）
/// - 客户端证书+私钥用于 mTLS 双向认证
fn build_custom_tls_config(
    tls_ca_cert: Option<&str>,
    tls_client_cert: Option<&str>,
    tls_client_key: Option<&str>,
) -> Result<Arc<rustls::ClientConfig>, String> {
    ensure_crypto_provider();

    // 构建 RootCertStore: 以系统根证书为基础
    let mut root_store = rustls::RootCertStore::empty();

    // 加载 webpki 根证书
    root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

    // 加载自定义 CA 证书（追加到系统根证书）
    if let Some(ca_cert_path) = tls_ca_cert {
        let ca_pem = read_file_content(ca_cert_path)?;
        let ca_certs = load_certs_from_pem(&ca_pem)?;
        if ca_certs.is_empty() {
            return Err("CA certificate file does not contain a valid certificate".to_string());
        }
        for cert in ca_certs {
            root_store
                .add(cert)
                .map_err(|e| format!("Failed to add custom CA certificate: {}", e))?;
        }
    }

    // 构建 ClientConfig
    let client_config = if let (Some(cert_path), Some(key_path)) = (tls_client_cert, tls_client_key)
    {
        // mTLS: 加载客户端证书和私钥
        let cert_pem = read_file_content(cert_path)?;
        let key_pem = read_file_content(key_path)?;
        let client_certs = load_certs_from_pem(&cert_pem)?;
        let client_key = load_key_from_pem(&key_pem)?;

        if client_certs.is_empty() {
            return Err("Client certificate file does not contain a valid certificate".to_string());
        }

        rustls::ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_client_auth_cert(client_certs, client_key)
            .map_err(|e| format!("Failed to configure mTLS client certificate: {}", e))?
    } else {
        // 仅 CA 验证（无客户端证书）
        rustls::ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth()
    };

    let mut config = client_config;
    config.alpn_protocols = vec![b"h2".to_vec()];
    Ok(Arc::new(config))
}

/// 使用自定义 rustls `ClientConfig` 构建 gRPC channel
///
/// 支持：自定义 CA、mTLS、SNI 覆盖。
/// 通过 `connect_with_connector_lazy()` 注入自定义 TLS 连接器。
fn build_custom_tls_channel(
    url_with_scheme: &str,
    connect_timeout_ms: u64,
    tls_ca_cert: Option<&str>,
    tls_client_cert: Option<&str>,
    tls_client_key: Option<&str>,
    tls_server_name_override: Option<&str>,
) -> Result<tonic::transport::Channel, String> {
    // 解析 URL 获取 host 和 port
    let original_uri: http::Uri = url_with_scheme
        .parse()
        .map_err(|e| format!("Failed to parse URL: {}", e))?;
    let host = original_uri
        .host()
        .ok_or_else(|| format!("URL is missing host: {}", url_with_scheme))?;
    let port = original_uri.port_u16().unwrap_or(443);

    // SNI 名称：使用覆盖值或 URL host
    let sni_name = tls_server_name_override
        .map(|s| s.to_string())
        .unwrap_or_else(|| host.to_string());

    // 构建 TLS 配置
    let client_config = build_custom_tls_config(tls_ca_cert, tls_client_cert, tls_client_key)?;

    // TLS SNI 名称
    let server_name = rustls::pki_types::ServerName::try_from(sni_name.clone())
        .map_err(|e| format!("Invalid TLS server name '{}': {}", sni_name, e))?
        .to_owned();

    // 使用 http:// 方案创建端点（绕过 tonic 的内置 TLS）
    let http_url = format!("http://{}:{}", host, port);
    let mut endpoint = tonic::transport::Endpoint::from_shared(http_url)
        .map_err(|e| format!("Failed to create gRPC endpoint: {}", e))?;

    if connect_timeout_ms > 0 {
        endpoint = endpoint.connect_timeout(std::time::Duration::from_millis(connect_timeout_ms));
    }

    // 构建自定义连接器: TCP 连接 + TLS 握手
    let tls_connector = tokio_rustls::TlsConnector::from(client_config);

    let connector = tower::service_fn(move |uri: http::Uri| {
        let tls = tls_connector.clone();
        let sni = server_name.clone();

        async move {
            let host = uri.host().ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::InvalidInput, "URI is missing host")
            })?;
            let port = uri.port_u16().unwrap_or(443);
            let addr = format!("{}:{}", host, port);

            let tcp_stream = tokio::net::TcpStream::connect(&addr).await?;
            tcp_stream.set_nodelay(true)?;

            let tls_stream = tls.connect(sni.to_owned(), tcp_stream).await?;
            Ok::<_, std::io::Error>(hyper_util::rt::TokioIo::new(tls_stream))
        }
    });

    Ok(endpoint.connect_with_connector_lazy(connector))
}

/// 构建 gRPC channel（跳过证书验证模式）
///
/// 使用自定义 rustls `ClientConfig` + dangerous verifier，
/// 通过 `connect_with_connector_lazy()` 注入自定义 TLS 连接器。
fn build_skip_verify_channel(
    url_with_scheme: &str,
    connect_timeout_ms: u64,
    tls_server_name_override: Option<&str>,
) -> Result<tonic::transport::Channel, String> {
    ensure_crypto_provider();

    // 解析原始 URL 获取 host（用于 TLS SNI）
    let original_uri: http::Uri = url_with_scheme
        .parse()
        .map_err(|e| format!("Failed to parse URL: {}", e))?;
    let host = original_uri
        .host()
        .ok_or_else(|| format!("URL is missing host: {}", url_with_scheme))?;
    let port = original_uri.port_u16().unwrap_or(443);

    // SNI 名称：使用覆盖值或 URL host
    let sni_name = tls_server_name_override
        .map(|s| s.to_string())
        .unwrap_or_else(|| host.to_string());

    // 构建 dangerous verifier 的 rustls ClientConfig
    let client_config = {
        let mut cfg = rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(NoVerifier))
            .with_no_client_auth();
        cfg.alpn_protocols = vec![b"h2".to_vec()];
        Arc::new(cfg)
    };

    // TLS SNI 名称
    let server_name = rustls::pki_types::ServerName::try_from(sni_name.clone())
        .map_err(|e| format!("Invalid TLS server name '{}': {}", sni_name, e))?
        .to_owned();

    // 使用 http:// 方案创建端点（绕过 tonic 的内置 TLS）
    let http_url = format!("http://{}:{}", host, port);
    let mut endpoint = tonic::transport::Endpoint::from_shared(http_url)
        .map_err(|e| format!("Failed to create gRPC endpoint: {}", e))?;

    if connect_timeout_ms > 0 {
        endpoint = endpoint.connect_timeout(std::time::Duration::from_millis(connect_timeout_ms));
    }

    // 构建自定义连接器: TCP 连接 + TLS 握手
    let tls_connector = tokio_rustls::TlsConnector::from(client_config);

    let connector = tower::service_fn(move |uri: http::Uri| {
        let tls = tls_connector.clone();
        let sni = server_name.clone();

        async move {
            let host = uri.host().ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::InvalidInput, "URI is missing host")
            })?;
            let port = uri.port_u16().unwrap_or(443);
            let addr = format!("{}:{}", host, port);

            let tcp_stream = tokio::net::TcpStream::connect(&addr).await?;
            tcp_stream.set_nodelay(true)?;

            let tls_stream = tls.connect(sni.to_owned(), tcp_stream).await?;
            Ok::<_, std::io::Error>(hyper_util::rt::TokioIo::new(tls_stream))
        }
    });

    Ok(endpoint.connect_with_connector_lazy(connector))
}

/// 将 keepalive 配置应用到 endpoint
///
/// tonic 0.12 提供的 keepalive API:
/// - `http2_keep_alive_interval()` — PING 间隔
/// - `keep_alive_timeout()` — PING 超时
/// - `keep_alive_while_idle()` — 无活跃流时是否允许 PING
///
/// 注意: `permit_keep_alive_without_calls()` 在 tonic 0.12 中不存在。
fn apply_keepalive(
    mut endpoint: tonic::transport::Endpoint,
    keepalive_time_ms: Option<u64>,
    keepalive_timeout_ms: Option<u64>,
    keepalive_permit_without_streams: Option<bool>,
) -> tonic::transport::Endpoint {
    if let Some(interval) = keepalive_time_ms {
        endpoint = endpoint.http2_keep_alive_interval(std::time::Duration::from_millis(interval));
    }
    if let Some(timeout) = keepalive_timeout_ms {
        endpoint = endpoint.keep_alive_timeout(std::time::Duration::from_millis(timeout));
    }
    if Some(true) == keepalive_permit_without_streams {
        // keep_alive_while_idle(true) 表示在没有活跃 RPC 时仍发送 PING
        endpoint = endpoint.keep_alive_while_idle(true);
    }
    endpoint
}

/// 构建单个端点（明文模式）
///
/// 为负载均衡场景创建单个 Endpoint，应用超时和 keepalive 配置。
fn build_plaintext_endpoint(
    url: &str,
    connect_timeout_ms: u64,
    keepalive_time_ms: Option<u64>,
    keepalive_timeout_ms: Option<u64>,
    keepalive_permit_without_streams: Option<bool>,
) -> Result<tonic::transport::Endpoint, String> {
    let mut endpoint = tonic::transport::Endpoint::from_shared(url.to_string())
        .map_err(|e| format!("Failed to create gRPC endpoint '{}': {}", url, e))?;
    if connect_timeout_ms > 0 {
        endpoint = endpoint.connect_timeout(std::time::Duration::from_millis(connect_timeout_ms));
    }
    endpoint = apply_keepalive(
        endpoint,
        keepalive_time_ms,
        keepalive_timeout_ms,
        keepalive_permit_without_streams,
    );
    Ok(endpoint)
}

/// 构建单个端点（标准 TLS 模式）
fn build_standard_tls_endpoint(
    url: &str,
    connect_timeout_ms: u64,
    keepalive_time_ms: Option<u64>,
    keepalive_timeout_ms: Option<u64>,
    keepalive_permit_without_streams: Option<bool>,
) -> Result<tonic::transport::Endpoint, String> {
    let mut endpoint = tonic::transport::Endpoint::from_shared(url.to_string())
        .map_err(|e| format!("Failed to create gRPC endpoint '{}': {}", url, e))?;
    endpoint = apply_standard_tls(endpoint)?;
    if connect_timeout_ms > 0 {
        endpoint = endpoint.connect_timeout(std::time::Duration::from_millis(connect_timeout_ms));
    }
    endpoint = apply_keepalive(
        endpoint,
        keepalive_time_ms,
        keepalive_timeout_ms,
        keepalive_permit_without_streams,
    );
    Ok(endpoint)
}

/// 根据 TLS 配置创建带负载均衡的 tonic channel
///
/// 当提供多个端点时，使用 `Channel::balance_list()` 进行轮询负载均衡。
/// 所有端点共享相同的 TLS/keepalive/超时配置。
/// 仅支持明文和标准 TLS 模式（不支持 `skip_verify` 和自定义 TLS 的负载均衡）。
pub(crate) fn create_balanced_channel(
    primary_url: &str,
    additional_endpoints: &[String],
    use_tls: bool,
    tls_skip_verify: bool,
    connect_timeout_ms: u64,
    tls_ca_cert: Option<&str>,
    tls_client_cert: Option<&str>,
    tls_client_key: Option<&str>,
    tls_server_name_override: Option<&str>,
    keepalive_time_ms: Option<u64>,
    keepalive_timeout_ms: Option<u64>,
    keepalive_permit_without_streams: Option<bool>,
) -> Result<tonic::transport::Channel, String> {
    // 负载均衡不支持 skip_verify 和自定义 TLS（使用自定义 connector，无法拆分为多端点）
    let has_custom_tls = use_tls
        && (tls_skip_verify
            || tls_ca_cert.is_some()
            || tls_client_cert.is_some()
            || tls_client_key.is_some()
            || tls_server_name_override.is_some());

    if has_custom_tls {
        return Err("Load balancing mode does not support skipping certificate verification or custom TLS configuration (CA/mTLS/SNI). Please remove extra endpoints or simplify TLS configuration.".to_string());
    }

    // 规范化 URL（确保包含 scheme）
    let normalize_url = |raw: &str| -> Result<String, String> {
        if raw.starts_with("http://") || raw.starts_with("https://") {
            Ok(raw.to_string())
        } else if use_tls {
            Ok(format!("https://{}", raw))
        } else {
            Ok(format!("http://{}", raw))
        }
    };

    let primary = normalize_url(primary_url)?;

    if use_tls {
        // 标准 TLS 模式
        let primary_ep = build_standard_tls_endpoint(
            &primary,
            connect_timeout_ms,
            keepalive_time_ms,
            keepalive_timeout_ms,
            keepalive_permit_without_streams,
        )?;

        if additional_endpoints.is_empty() {
            return Ok(primary_ep.connect_lazy());
        }

        let mut endpoints: Vec<tonic::transport::Endpoint> = vec![primary_ep];
        for (i, ep_url) in additional_endpoints.iter().enumerate() {
            let normalized =
                normalize_url(ep_url).map_err(|e| format!("Extra endpoint[{}] {}", i, e))?;
            let ep = build_standard_tls_endpoint(
                &normalized,
                connect_timeout_ms,
                keepalive_time_ms,
                keepalive_timeout_ms,
                keepalive_permit_without_streams,
            )?;
            endpoints.push(ep);
        }

        Ok(tonic::transport::Channel::balance_list(
            endpoints.into_iter(),
        ))
    } else {
        // 明文模式
        let primary_ep = build_plaintext_endpoint(
            &primary,
            connect_timeout_ms,
            keepalive_time_ms,
            keepalive_timeout_ms,
            keepalive_permit_without_streams,
        )?;

        if additional_endpoints.is_empty() {
            return Ok(primary_ep.connect_lazy());
        }

        let mut endpoints: Vec<tonic::transport::Endpoint> = vec![primary_ep];
        for (i, ep_url) in additional_endpoints.iter().enumerate() {
            let normalized =
                normalize_url(ep_url).map_err(|e| format!("Extra endpoint[{}] {}", i, e))?;
            let ep = build_plaintext_endpoint(
                &normalized,
                connect_timeout_ms,
                keepalive_time_ms,
                keepalive_timeout_ms,
                keepalive_permit_without_streams,
            )?;
            endpoints.push(ep);
        }

        Ok(tonic::transport::Channel::balance_list(
            endpoints.into_iter(),
        ))
    }
}

/// 根据 TLS 配置创建 tonic channel
///
/// TLS 模式优先级:
/// 1. `!use_tls` → 明文连接
/// 2. `use_tls && tls_skip_verify` → 跳过证书验证
/// 3. `use_tls && (tls_ca_cert | tls_client_cert)` → 自定义 TLS（CA/mTLS/SNI）
/// 4. `use_tls` → 标准 TLS（webpki-roots 根证书）
pub(crate) fn create_channel(
    url_with_scheme: &str,
    use_tls: bool,
    tls_skip_verify: bool,
    connect_timeout_ms: u64,
    tls_ca_cert: Option<&str>,
    tls_client_cert: Option<&str>,
    tls_client_key: Option<&str>,
    tls_server_name_override: Option<&str>,
    keepalive_time_ms: Option<u64>,
    keepalive_timeout_ms: Option<u64>,
    keepalive_permit_without_streams: Option<bool>,
) -> Result<tonic::transport::Channel, String> {
    // 明文模式
    if !use_tls {
        let mut endpoint = tonic::transport::Endpoint::from_shared(url_with_scheme.to_string())
            .map_err(|e| format!("Failed to create gRPC endpoint: {}", e))?;
        if connect_timeout_ms > 0 {
            endpoint =
                endpoint.connect_timeout(std::time::Duration::from_millis(connect_timeout_ms));
        }
        endpoint = apply_keepalive(
            endpoint,
            keepalive_time_ms,
            keepalive_timeout_ms,
            keepalive_permit_without_streams,
        );
        return Ok(endpoint.connect_lazy());
    }

    // 跳过证书验证模式
    if tls_skip_verify {
        let channel = build_skip_verify_channel(
            url_with_scheme,
            connect_timeout_ms,
            tls_server_name_override,
        )?;
        // 注意: skip_verify 使用 connect_with_connector_lazy，无法直接在 endpoint 上设 keepalive。
        // HTTP/2 keepalive 需要在 endpoint 上配置后再 connect_lazy。
        // 此处保留 channel（keepalive 需要通过自定义 connector 逻辑实现，当前跳过）。
        return Ok(channel);
    }

    // 自定义 TLS 模式（自定义 CA / mTLS / SNI 覆盖）
    let has_custom_tls = tls_ca_cert.is_some()
        || tls_client_cert.is_some()
        || tls_client_key.is_some()
        || tls_server_name_override.is_some();

    if has_custom_tls {
        let channel = build_custom_tls_channel(
            url_with_scheme,
            connect_timeout_ms,
            tls_ca_cert,
            tls_client_cert,
            tls_client_key,
            tls_server_name_override,
        )?;
        // 同 skip_verify：自定义 TLS 使用 connect_with_connector_lazy，keepalive 暂不支持
        return Ok(channel);
    }

    // 标准 TLS 模式: webpki-roots + 系统原生证书
    let mut endpoint = tonic::transport::Endpoint::from_shared(url_with_scheme.to_string())
        .map_err(|e| format!("Failed to create gRPC endpoint: {}", e))?;
    endpoint = apply_standard_tls(endpoint)?;
    if connect_timeout_ms > 0 {
        endpoint = endpoint.connect_timeout(std::time::Duration::from_millis(connect_timeout_ms));
    }
    endpoint = apply_keepalive(
        endpoint,
        keepalive_time_ms,
        keepalive_timeout_ms,
        keepalive_permit_without_streams,
    );
    Ok(endpoint.connect_lazy())
}
