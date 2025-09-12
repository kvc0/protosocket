use std::{future::Future, net::SocketAddr, sync::Arc};

use protosocket::Connection;
use tokio::{net::TcpStream, sync::mpsc};
use tokio_rustls::rustls::pki_types::ServerName;

use crate::{
    client::reactor::completion_reactor::{DoNothingMessageHandler, RpcCompletionReactor},
    Message,
};

use super::{reactor::completion_reactor::RpcCompletionConnectionBindings, RpcClient};

pub trait StreamConnector: std::fmt::Debug {
    type Stream: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Unpin + 'static;

    fn connect_stream(
        &self,
        stream: TcpStream,
    ) -> impl Future<Output = std::io::Result<Self::Stream>> + Send;
}

/// A `StreamConnector` for bare TCP streams.
#[derive(Debug)]
pub struct TcpStreamConnector;
impl StreamConnector for TcpStreamConnector {
    type Stream = TcpStream;

    fn connect_stream(
        &self,
        stream: TcpStream,
    ) -> impl Future<Output = std::io::Result<Self::Stream>> + Send {
        std::future::ready(Ok(stream))
    }
}

/// A `StreamConnector` for PKI TLS streams.
pub struct WebpkiTlsStreamConnector {
    connector: tokio_rustls::TlsConnector,
    servername: ServerName<'static>,
}
impl WebpkiTlsStreamConnector {
    /// Create a new `TlsStreamConnector` for a server
    pub fn new(servername: ServerName<'static>) -> Self {
        let client_config = Arc::new(
            tokio_rustls::rustls::ClientConfig::builder_with_protocol_versions(&[
                &tokio_rustls::rustls::version::TLS13,
            ])
            .with_root_certificates(tokio_rustls::rustls::RootCertStore::from_iter(
                webpki_roots::TLS_SERVER_ROOTS.iter().cloned(),
            ))
            .with_no_client_auth(),
        );
        let connector = tokio_rustls::TlsConnector::from(client_config);
        Self {
            connector,
            servername,
        }
    }
}
impl std::fmt::Debug for WebpkiTlsStreamConnector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TlsStreamConnector").finish_non_exhaustive()
    }
}
impl StreamConnector for WebpkiTlsStreamConnector {
    type Stream = tokio_rustls::client::TlsStream<TcpStream>;

    fn connect_stream(
        &self,
        stream: TcpStream,
    ) -> impl Future<Output = std::io::Result<Self::Stream>> + Send {
        self.connector
            .clone()
            .connect(self.servername.clone(), stream)
    }
}

/// A `StreamConnector` for self-signed server TLS streams. No host certificate validation is performed.
pub struct UnverifiedTlsStreamConnector {
    connector: tokio_rustls::TlsConnector,
    servername: ServerName<'static>,
}
impl UnverifiedTlsStreamConnector {
    /// Create a new `UnverifiedTlsStreamConnector` for a server.
    /// This connector does not perform any certificate validation.
    pub fn new(servername: ServerName<'static>) -> Self {
        let client_config = Arc::new(
            tokio_rustls::rustls::ClientConfig::builder_with_protocol_versions(&[
                &tokio_rustls::rustls::version::TLS13,
            ])
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(DoNothingVerifier))
            .with_no_client_auth(),
        );
        let connector = tokio_rustls::TlsConnector::from(client_config);
        Self {
            connector,
            servername,
        }
    }
}
impl std::fmt::Debug for UnverifiedTlsStreamConnector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UnverifiedTlsStreamConnector")
            .finish_non_exhaustive()
    }
}
impl StreamConnector for UnverifiedTlsStreamConnector {
    type Stream = tokio_rustls::client::TlsStream<TcpStream>;

    fn connect_stream(
        &self,
        stream: TcpStream,
    ) -> impl Future<Output = std::io::Result<Self::Stream>> + Send {
        self.connector
            .clone()
            .connect(self.servername.clone(), stream)
    }
}

// You don't need this if you use a real certificate
#[derive(Debug)]
struct DoNothingVerifier;
impl tokio_rustls::rustls::client::danger::ServerCertVerifier for DoNothingVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls_pki_types::CertificateDer<'_>,
        _intermediates: &[rustls_pki_types::CertificateDer<'_>],
        _server_name: &rustls_pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls_pki_types::UnixTime,
    ) -> Result<tokio_rustls::rustls::client::danger::ServerCertVerified, tokio_rustls::rustls::Error>
    {
        Ok(tokio_rustls::rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls_pki_types::CertificateDer<'_>,
        _dss: &tokio_rustls::rustls::DigitallySignedStruct,
    ) -> Result<
        tokio_rustls::rustls::client::danger::HandshakeSignatureValid,
        tokio_rustls::rustls::Error,
    > {
        Ok(tokio_rustls::rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls_pki_types::CertificateDer<'_>,
        _dss: &tokio_rustls::rustls::DigitallySignedStruct,
    ) -> Result<
        tokio_rustls::rustls::client::danger::HandshakeSignatureValid,
        tokio_rustls::rustls::Error,
    > {
        Ok(tokio_rustls::rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<tokio_rustls::rustls::SignatureScheme> {
        tokio_rustls::rustls::crypto::aws_lc_rs::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// Configuration for a `protosocket` rpc client.
#[derive(Debug, Clone)]
pub struct Configuration<TStreamConnector> {
    max_buffer_length: usize,
    max_queued_outbound_messages: usize,
    stream_connector: TStreamConnector,
}

impl<TStreamConnector> Configuration<TStreamConnector>
where
    TStreamConnector: StreamConnector,
{
    pub fn new(stream_connector: TStreamConnector) -> Self {
        log::trace!("new client configuration");
        Self {
            max_buffer_length: 4 * (1 << 20), // 4 MiB
            max_queued_outbound_messages: 256,
            stream_connector,
        }
    }

    /// Max buffer length limits the max message size. Try to use a buffer length that is at least 4 times the largest message you want to support.
    ///
    /// Default: 4MiB
    pub fn max_buffer_length(&mut self, max_buffer_length: usize) {
        self.max_buffer_length = max_buffer_length;
    }

    /// Max messages that will be queued up waiting for send on the client channel.
    ///
    /// Default: 256
    pub fn max_queued_outbound_messages(&mut self, max_queued_outbound_messages: usize) {
        self.max_queued_outbound_messages = max_queued_outbound_messages;
    }
}

/// Connect a new protosocket rpc client to a server
pub async fn connect<Serializer, Deserializer, TStreamConnector>(
    address: SocketAddr,
    configuration: &Configuration<TStreamConnector>,
) -> Result<
    (
        RpcClient<Serializer::Message, Deserializer::Message>,
        protosocket::Connection<
            RpcCompletionConnectionBindings<Serializer, Deserializer, TStreamConnector::Stream>,
        >,
    ),
    crate::Error,
>
where
    Deserializer: protosocket::Deserializer + Default + 'static,
    Serializer: protosocket::Serializer + Default + 'static,
    Deserializer::Message: Message,
    Serializer::Message: Message,
    TStreamConnector: StreamConnector,
{
    log::trace!("new client {address}, {configuration:?}");

    let stream = tokio::net::TcpStream::connect(address).await?;
    stream.set_nodelay(true)?;

    let message_reactor: RpcCompletionReactor<
        Deserializer::Message,
        DoNothingMessageHandler<Deserializer::Message>,
    > = RpcCompletionReactor::new(Default::default());
    let (outbound, outbound_messages) = mpsc::channel(configuration.max_queued_outbound_messages);
    let rpc_client = RpcClient::new(outbound, &message_reactor);
    let stream = configuration
        .stream_connector
        .connect_stream(stream)
        .await?;

    // Tie outbound_messages to message_reactor via a protosocket::Connection
    let connection = Connection::<
        RpcCompletionConnectionBindings<Serializer, Deserializer, TStreamConnector::Stream>,
    >::new(
        stream,
        address,
        Deserializer::default(),
        Serializer::default(),
        configuration.max_buffer_length,
        configuration.max_queued_outbound_messages,
        outbound_messages,
        message_reactor,
    );

    Ok((rpc_client, connection))
}
