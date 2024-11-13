use protosocket_rpc::ProtosocketControlCode;

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

impl protosocket_rpc::Message for Request {
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

    fn cancelled() -> Self {
        Request {
            request_id: 0,
            body: None,
        }
    }

    fn set_message_id(&mut self, message_id: u64) {
        self.request_id = message_id;
    }

    fn ended() -> Self {
        Self {
            request_id: 0,
            body: None,
        }
    }
}

impl protosocket_rpc::Message for Response {
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

    fn cancelled() -> Self {
        Response {
            request_id: 0,
            body: None,
        }
    }

    fn set_message_id(&mut self, message_id: u64) {
        self.request_id = message_id
    }

    fn ended() -> Self {
        Self {
            request_id: 0,
            body: None,
        }
    }
}
