//! gRPC 测试服务器（集成测试用）。
//!
//! tonic server，暴露 echo.EchoService（Unary / ServerStreaming /
//! ClientStreaming / BidiStreaming）+ Server Reflection v1 + Health 服务。
//! 启动后向 stdout 打印一行 `LISTENING <addr>`（如 `LISTENING
//! 127.0.0.1:54321`），集成测试解析该行获取端口。
//!
//! 行为约定（集成测试断言依据）：
//! - `Unary(msg)` → 原样回显（text/n/attrs/tags）。
//! - `ServerStreaming(msg)` → 发送 `n` 条（n <= 0 时 3 条），第 i 条
//!   text = `{text}-{i}`（1 起），每条间隔 20ms。
//! - `ClientStreaming(msgs)` → 单条响应，text = 所有输入 text 用 `|`
//!   连接，n = 输入条数。
//! - `BidiStreaming` → 逐条原样回显。

use std::time::Duration;

use prost::Message;
use tonic::{transport::Server, Request, Response, Status};

include!(concat!(env!("OUT_DIR"), "/echo.rs"));

#[derive(Default)]
struct EchoService;

#[tonic::async_trait]
impl echo_service_server::EchoService for EchoService {
    async fn unary(
        &self,
        request: Request<EchoMessage>,
    ) -> Result<Response<EchoMessage>, Status> {
        Ok(Response::new(request.into_inner()))
    }

    type ServerStreamingStream =
        tokio_stream::wrappers::ReceiverStream<Result<EchoMessage, Status>>;

    async fn server_streaming(
        &self,
        request: Request<EchoMessage>,
    ) -> Result<Response<Self::ServerStreamingStream>, Status> {
        let msg = request.into_inner();
        let count = if msg.n > 0 { msg.n as usize } else { 3 };
        let (tx, rx) = tokio::sync::mpsc::channel(count);
        tokio::spawn(async move {
            for i in 1..=count {
                let mut echo = EchoMessage {
                    text: format!("{}-{}", msg.text, i),
                    n: msg.n,
                    attrs: msg.attrs.clone(),
                    tags: msg.tags.clone(),
                };
                if msg.attrs.contains_key("echo_attrs") {
                    echo.attrs.insert("stream_index".to_string(), i.to_string());
                }
                if tx.send(Ok(echo)).await.is_err() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        });
        Ok(Response::new(tokio_stream::wrappers::ReceiverStream::new(rx)))
    }

    async fn client_streaming(
        &self,
        request: Request<tonic::Streaming<EchoMessage>>,
    ) -> Result<Response<EchoMessage>, Status> {
        let mut stream = request.into_inner();
        let mut texts = Vec::new();
        let mut count = 0usize;
        while let Some(msg) = stream.message().await? {
            texts.push(msg.text);
            count += 1;
        }
        Ok(Response::new(EchoMessage {
            text: texts.join("|"),
            n: count as i32,
            attrs: Default::default(),
            tags: Default::default(),
        }))
    }

    type BidiStreamingStream =
        tokio_stream::wrappers::ReceiverStream<Result<EchoMessage, Status>>;

    async fn bidi_streaming(
        &self,
        request: Request<tonic::Streaming<EchoMessage>>,
    ) -> Result<Response<Self::BidiStreamingStream>, Status> {
        let mut stream = request.into_inner();
        let (tx, rx) = tokio::sync::mpsc::channel(16);
        tokio::spawn(async move {
            while let Ok(Some(msg)) = stream.message().await {
                if tx.send(Ok(msg)).await.is_err() {
                    break;
                }
            }
        });
        Ok(Response::new(tokio_stream::wrappers::ReceiverStream::new(rx)))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 绑定随机空闲端口
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    println!("LISTENING {}", addr);
    eprintln!("grpc_test_server listening on {}", addr);

    // Server Reflection v1（描述符经 protox 运行时编译 echo.proto）
    let fds = protox::compile(["echo.proto"], ["src/bin/"])?;
    let reflection = tonic_reflection::server::Builder::configure()
        .register_encoded_file_descriptor_set(&fds.encode_to_vec())
        .build_v1()?;

    // Health 服务（上报 echo.EchoService 为 SERVING）
    let (mut health_reporter, health_service) = tonic_health::server::health_reporter();
    health_reporter
        .set_serving::<echo_service_server::EchoServiceServer<EchoService>>()
        .await;

    Server::builder()
        .add_service(echo_service_server::EchoServiceServer::new(EchoService))
        .add_service(reflection)
        .add_service(health_service)
        .serve_with_incoming_shutdown(
            tokio_stream::wrappers::TcpListenerStream::new(listener),
            async {
                // 收到 stdin 关闭即退出（集成测试结束通过 drop 子进程）
                let _ = tokio::signal::ctrl_c().await;
            },
        )
        .await?;

    Ok(())
}
