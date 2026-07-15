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
    #[prost(message, tag = "3")]
    pub body: Option<EchoStream>,
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
            body: None,
        }
    }

    fn set_message_id(&mut self, message_id: u64) {
        self.request_id = message_id
    }

    fn ended(request_id: u64) -> Self {
        Self {
            request_id,
            code: ProtosocketControlCode::End as u32,
            body: None,
        }
    }
}
