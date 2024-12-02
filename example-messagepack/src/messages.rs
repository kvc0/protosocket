//! If you're only using rust, of course you can hand-write prost structs, but if you
//! want to use a protosocket server with clients in other languages you'll want to
//! generate from protos.

use protosocket_rpc::ProtosocketControlCode;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Request {
    pub request_id: u64,
    pub code: u32,
    pub body: Option<EchoRequest>,
    pub response_behavior: ResponseBehavior,
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[repr(i32)]
pub enum ResponseBehavior {
    Unary = 0,
    Stream = 1,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EchoRequest {
    pub message: String,
    pub nanotime: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Response {
    pub request_id: u64,
    pub code: u32,
    pub kind: Option<EchoResponseKind>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum EchoResponseKind {
    Echo(EchoResponse),
    Stream(EchoStream),
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EchoResponse {
    pub message: String,
    pub nanotime: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EchoStream {
    pub message: String,
    pub nanotime: u64,
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
            response_behavior: ResponseBehavior::Unary,
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
            response_behavior: ResponseBehavior::Unary,
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
