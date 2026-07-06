//! TLS session setup for HTTPS connections.

use std::net::TcpStream;
use std::sync::Arc;

use rustls::{ClientConfig, ClientConnection, StreamOwned};
use rustls_pki_types::ServerName;
use rustls_pki_types::pem::PemObject;

use super::TransportError;

/// TLS options resolved from the CLI (`--verify`, `--ssl`, `--cert`, …).
#[derive(Debug, Clone, Default)]
pub struct TlsOptions {
    /// `--verify=no` turns certificate verification off entirely.
    pub verify: Verification,
    /// `--ssl`: pin the protocol version.
    pub version: Option<TlsVersion>,
    /// `--cert` (+ `--cert-key`): client identity, PEM paths.
    pub client_cert: Option<std::path::PathBuf>,
    pub client_key: Option<std::path::PathBuf>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub enum Verification {
    /// Platform trust store (the default).
    #[default]
    Platform,
    /// Skip verification (`--verify=no`).
    Insecure,
    /// A custom CA bundle path (`--verify=/path/to/ca.pem`).
    CaBundle(std::path::PathBuf),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TlsVersion {
    Tls12,
    Tls13,
}

pub type TlsStream = StreamOwned<ClientConnection, TcpStream>;

pub fn wrap(
    stream: TcpStream,
    host: &str,
    options: &TlsOptions,
) -> Result<TlsStream, TransportError> {
    let config = client_config(options)?;
    let server_name = ServerName::try_from(host.to_string())
        .map_err(|_| TransportError::Tls(format!("invalid server name: {host}")))?;
    let connection = ClientConnection::new(Arc::new(config), server_name)
        .map_err(|error| TransportError::Tls(error.to_string()))?;
    let mut tls = StreamOwned::new(connection, stream);
    // Drive the handshake now so TLS failures surface as TLS errors
    // rather than surfacing on the first write.
    tls.conn
        .complete_io(&mut tls.sock)
        .map_err(|error| TransportError::Tls(flatten_tls_error(&error)))?;
    Ok(tls)
}

fn flatten_tls_error(error: &std::io::Error) -> String {
    match error.get_ref() {
        Some(inner) => inner.to_string(),
        None => error.to_string(),
    }
}

fn protocol_versions(pinned: Option<TlsVersion>) -> Vec<&'static rustls::SupportedProtocolVersion> {
    match pinned {
        None => vec![&rustls::version::TLS12, &rustls::version::TLS13],
        Some(TlsVersion::Tls12) => vec![&rustls::version::TLS12],
        Some(TlsVersion::Tls13) => vec![&rustls::version::TLS13],
    }
}

fn client_config(options: &TlsOptions) -> Result<ClientConfig, TransportError> {
    let versions = protocol_versions(options.version);
    let builder = ClientConfig::builder_with_protocol_versions(&versions);

    let builder = match &options.verify {
        Verification::Platform => {
            let verifier = rustls_platform_verifier::Verifier::new(
                rustls::crypto::CryptoProvider::get_default()
                    .cloned()
                    .unwrap_or_else(|| Arc::new(rustls::crypto::aws_lc_rs::default_provider())),
            )
            .map_err(|error| TransportError::Tls(error.to_string()))?;
            builder
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(verifier))
        }
        Verification::Insecure => builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(danger::NoVerification)),
        Verification::CaBundle(path) => {
            let mut roots = rustls::RootCertStore::empty();
            let bundle = std::fs::read(path).map_err(|error| {
                TransportError::Tls(format!(
                    "could not read CA bundle {}: {error}",
                    path.display()
                ))
            })?;
            for cert in rustls_pki_types::CertificateDer::pem_slice_iter(&bundle) {
                let cert = cert
                    .map_err(|error| TransportError::Tls(format!("invalid CA bundle: {error}")))?;
                roots
                    .add(cert)
                    .map_err(|error| TransportError::Tls(error.to_string()))?;
            }
            builder.with_root_certificates(roots)
        }
    };

    let config = match (&options.client_cert, &options.client_key) {
        (Some(cert_path), key_path) => {
            let key_path = key_path.as_ref().unwrap_or(cert_path);
            let cert_bytes = std::fs::read(cert_path).map_err(|error| {
                TransportError::Tls(format!("{}: {error}", cert_path.display()))
            })?;
            let certs: Result<Vec<_>, _> =
                rustls_pki_types::CertificateDer::pem_slice_iter(&cert_bytes).collect();
            let certs = certs
                .map_err(|error| TransportError::Tls(format!("invalid client cert: {error}")))?;
            let key_bytes = std::fs::read(key_path)
                .map_err(|error| TransportError::Tls(format!("{}: {error}", key_path.display())))?;
            let key = rustls_pki_types::PrivateKeyDer::from_pem_slice(&key_bytes)
                .map_err(|error| TransportError::Tls(format!("invalid client key: {error}")))?;
            builder
                .with_client_auth_cert(certs, key)
                .map_err(|error| TransportError::Tls(error.to_string()))?
        }
        (None, _) => builder.with_no_client_auth(),
    };
    Ok(config)
}

mod danger {
    //! The `--verify=no` verifier: accepts anything, on explicit request.

    use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};

    #[derive(Debug)]
    pub struct NoVerification;

    impl ServerCertVerifier for NoVerification {
        fn verify_server_cert(
            &self,
            _end_entity: &rustls_pki_types::CertificateDer<'_>,
            _intermediates: &[rustls_pki_types::CertificateDer<'_>],
            _server_name: &rustls_pki_types::ServerName<'_>,
            _ocsp_response: &[u8],
            _now: rustls_pki_types::UnixTime,
        ) -> Result<ServerCertVerified, rustls::Error> {
            Ok(ServerCertVerified::assertion())
        }

        fn verify_tls12_signature(
            &self,
            _message: &[u8],
            _cert: &rustls_pki_types::CertificateDer<'_>,
            _dss: &rustls::DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, rustls::Error> {
            Ok(HandshakeSignatureValid::assertion())
        }

        fn verify_tls13_signature(
            &self,
            _message: &[u8],
            _cert: &rustls_pki_types::CertificateDer<'_>,
            _dss: &rustls::DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, rustls::Error> {
            Ok(HandshakeSignatureValid::assertion())
        }

        fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
            rustls::crypto::CryptoProvider::get_default()
                .map(|provider| {
                    provider
                        .signature_verification_algorithms
                        .supported_schemes()
                })
                .unwrap_or_default()
        }
    }
}
