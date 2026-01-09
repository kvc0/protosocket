use std::{future::Future, sync::Arc};
use tokio::net::TcpStream;
use tokio_rustls::rustls::pki_types::ServerName;

/// An async handshake that provides an `AsyndRead`/`AsyncWrite` stream.
///
/// You could consider wrapping these if you need to hook stream connection, or of course
/// you can implement your own connector for your own stream type.
pub trait StreamConnector: std::fmt::Debug {
    /// The type of stream this connector will produce
    type Stream: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Unpin + 'static;

    /// Take a TCP stream and turn it into a new Stream type.
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
