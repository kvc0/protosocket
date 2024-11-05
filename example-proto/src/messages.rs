use protosocket_rpc_client::ProtosocketControlCode;

#[derive(Clone, PartialEq, Eq, prost::Message)]
pub struct Request {
    #[prost(uint64, tag = "1")]
    pub request_id: u64,
    #[prost(message, tag = "2")]
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
    #[prost(message, tag = "2")]
    pub body: Option<EchoResponse>,
}

#[derive(Clone, PartialEq, Eq, prost::Message)]
pub struct EchoResponse {
    #[prost(string, tag = "1")]
    pub message: String,
    #[prost(uint64, tag = "2")]
    pub nanotime: u64,
}

impl protosocket_rpc_client::Message for Request {
    fn message_id(&self) -> u64 {
        self.request_id
    }

    fn control_code(&self) -> ProtosocketControlCode {
        // I'm treating the absence of a body as a cancellation. You can model this however best suits your messages.
        match self.body {
            Some(_) => ProtosocketControlCode::Normal,
            None => ProtosocketControlCode::Cancel,
        }
    }

    fn cancelled(message_id: u64) -> Self {
        Request {
            request_id: message_id,
            body: None,
        }
    }
}

impl protosocket_rpc_client::Message for Response {
    fn message_id(&self) -> u64 {
        self.request_id
    }

    fn control_code(&self) -> ProtosocketControlCode {
        // I'm treating the absence of a body as a cancellation. You can model this however best suits your messages.
        match self.body {
            Some(_) => ProtosocketControlCode::Normal,
            None => ProtosocketControlCode::Cancel,
        }
    }

    fn cancelled(message_id: u64) -> Self {
        Response {
            request_id: message_id,
            body: None,
        }
    }
}
