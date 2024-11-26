//! If you're only using rust, of course you can hand-write prost structs, but if you
//! want to use a protosocket server with clients in other languages you'll want to
//! generate from protos.

use protosocket_rpc::ProtosocketControlCode;

#[derive(Clone, PartialEq, Eq, prost::Message)]
pub struct Request {
    #[prost(uint64, tag = "1")]
    pub request_id: u64,
    #[prost(uint32, tag = "2")]
    pub code: u32,
    #[prost(message, tag = "3")]
    pub body: Option<EchoRequest>,
    #[prost(enumeration = "ResponseBehavior", tag = "4")]
    pub response_behavior: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, prost::Enumeration)]
#[repr(i32)]
pub enum ResponseBehavior {
    Unary = 0,
    Stream = 1,
}

#[derive(Clone, PartialEq, Eq, prost::Message)]
pub struct EchoRequest {
    #[prost(string, tag = "1")]
    pub message: String,
    #[prost(uint64, tag = "2")]
    pub nanotime: u64,
}

#[derive(Clone, PartialEq, Eq, prost::Message)]
pub struct Response {
    #[prost(uint64, tag = "1")]
    pub request_id: u64,
    #[prost(uint32, tag = "2")]
    pub code: u32,
    #[prost(oneof = "EchoResponseKind", tags = "3, 4")]
    pub kind: Option<EchoResponseKind>,
}

#[derive(Clone, PartialEq, Eq, prost::Oneof)]
pub enum EchoResponseKind {
    #[prost(message, tag = "3")]
    Echo(EchoResponse),
    #[prost(message, tag = "4")]
    Stream(EchoStream),
}

#[derive(Clone, PartialEq, Eq, prost::Message)]
pub struct EchoResponse {
    #[prost(string, tag = "1")]
    pub message: String,
    #[prost(uint64, tag = "2")]
    pub nanotime: u64,
}

#[derive(Clone, PartialEq, Eq, prost::Message)]
pub struct EchoStream {
    #[prost(string, tag = "1")]
    pub message: String,
    #[prost(uint64, tag = "2")]
    pub nanotime: u64,
    #[prost(uint64, tag = "3")]
    pub sequence: u64,
}

impl protosocket_rpc::Message for Request {
    fn message_id(&self) -> u64 {
        self.request_id
    }

    fn control_code(&self) -> ProtosocketControlCode {
        ProtosocketControlCode::from_u8(self.code as u8)
    }

    fn cancelled(request_id: u64) -> Self {
        Request {
            request_id,
            code: ProtosocketControlCode::Cancel as u32,
            body: None,
            response_behavior: ResponseBehavior::Unary as i32,
        }
    }

    fn set_message_id(&mut self, message_id: u64) {
        self.request_id = message_id;
    }

    fn ended(request_id: u64) -> Self {
        Self {
            request_id,
            code: ProtosocketControlCode::End as u32,
            body: None,
            response_behavior: ResponseBehavior::Unary as i32,
        }
    }
}

impl protosocket_rpc::Message for Response {
    fn message_id(&self) -> u64 {
        self.request_id
    }

    fn control_code(&self) -> ProtosocketControlCode {
        ProtosocketControlCode::from_u8(self.code as u8)
    }

    fn cancelled(request_id: u64) -> Self {
        Response {
            request_id,
            code: ProtosocketControlCode::Cancel as u32,
            kind: None,
        }
    }

    fn set_message_id(&mut self, message_id: u64) {
        self.request_id = message_id
    }

    fn ended(request_id: u64) -> Self {
        Self {
            request_id,
            code: ProtosocketControlCode::End as u32,
            kind: None,
        }
    }
}
