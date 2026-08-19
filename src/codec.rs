//! 动态 gRPC 编解码器
//!
//! 使用 prost-reflect 的 `DynamicMessage` 实现 tonic 的 Codec trait，
//! 从而支持对任意 protobuf 消息类型的动态编解码。
//! （从宿主 flow-engine-grpc 原样迁移）

use prost_reflect::{DynamicMessage, MethodDescriptor};
use tonic::codec::{Codec, DecodeBuf, Decoder, EncodeBuf, Encoder};

/// 动态 gRPC 编解码器
///
/// 使用 prost-reflect 的 `DynamicMessage` 实现 tonic 的 Codec trait，
/// 从而支持对任意 protobuf 消息类型的动态编解码。
#[derive(Clone)]
pub(crate) struct DynamicCodec {
    /// 方法描述符（包含输入输出类型信息）
    method: MethodDescriptor,
}

impl DynamicCodec {
    /// 创建新的动态编解码器
    pub(crate) fn new(method: MethodDescriptor) -> Self {
        Self { method }
    }
}

impl Codec for DynamicCodec {
    type Encode = DynamicMessage;
    type Decode = DynamicMessage;
    type Encoder = Self;
    type Decoder = Self;

    fn encoder(&mut self) -> Self::Encoder {
        self.clone()
    }

    fn decoder(&mut self) -> Self::Decoder {
        self.clone()
    }
}

impl Encoder for DynamicCodec {
    type Item = DynamicMessage;
    type Error = tonic::Status;

    fn encode(&mut self, item: Self::Item, dst: &mut EncodeBuf<'_>) -> Result<(), Self::Error> {
        prost::Message::encode(&item, dst)
            .map_err(|e| tonic::Status::internal(format!("failed to encode message: {}", e)))?;
        Ok(())
    }
}

impl Decoder for DynamicCodec {
    type Item = DynamicMessage;
    type Error = tonic::Status;

    fn decode(&mut self, src: &mut DecodeBuf<'_>) -> Result<Option<Self::Item>, Self::Error> {
        let mut msg = DynamicMessage::new(self.method.output());
        prost::Message::merge(&mut msg, src)
            .map_err(|e| tonic::Status::internal(format!("failed to decode message: {}", e)))?;
        Ok(Some(msg))
    }
}
