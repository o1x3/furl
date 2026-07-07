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
    /// `--ciphers`: colon-separated cipher-suite names to allow.
    pub ciphers: Option<String>,
    /// `--cert-key-pass`: passphrase for an encrypted client key,
    /// possibly collected from a terminal prompt at request-build time.
    pub cert_key_pass: Option<String>,
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

/// Wrap any byte stream in TLS (a plain TCP connection, or an
/// established proxy tunnel for TLS-in-TLS).
pub fn wrap<S: std::io::Read + std::io::Write>(
    stream: S,
    host: &str,
    options: &TlsOptions,
) -> Result<StreamOwned<ClientConnection, S>, TransportError> {
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

/// The pre-request passphrase for the `--cert-key` file: an explicit
/// `--cert-key-pass` wins; otherwise a PKCS#8-encrypted key file prompts
/// on the terminal, unless prompting is disabled (`--ignore-stdin`).
pub fn resolve_key_passphrase(
    key_path: Option<&std::path::Path>,
    explicit: Option<String>,
    allow_prompt: bool,
) -> Option<String> {
    if explicit.is_some() {
        return explicit;
    }
    if !allow_prompt {
        return None;
    }
    let path = key_path?;
    // Unreadable files fall through: the transport reports the read
    // error itself once the connection is attempted.
    let content = std::fs::read_to_string(path).ok()?;
    if !content.contains(PEM_PKCS8_ENCRYPTED_BEGIN) {
        return None;
    }
    let prompt = format!("furl: passphrase for {}: ", path.display());
    rpassword::prompt_password(prompt).ok()
}

const PEM_PKCS8_ENCRYPTED_BEGIN: &str = "-----BEGIN ENCRYPTED PRIVATE KEY-----";
const PEM_PKCS8_ENCRYPTED_END: &str = "-----END ENCRYPTED PRIVATE KEY-----";

/// Restrict the crypto provider to the `--ciphers` selection: a
/// colon-separated list of cipher-suite names, matched case-insensitively
/// and with or without the `TLS_`/`TLS13_` prefix. Unknown names are
/// skipped; an empty selection is an error naming the unmatched input.
fn restrict_ciphers(
    base: &rustls::crypto::CryptoProvider,
    spec: &str,
) -> Result<rustls::crypto::CryptoProvider, TransportError> {
    let mut selected: Vec<rustls::SupportedCipherSuite> = Vec::new();
    let mut unmatched: Vec<&str> = Vec::new();
    for token in spec.split(':').filter(|token| !token.is_empty()) {
        let mut hit = false;
        for suite in &base.cipher_suites {
            let name = suite.suite().as_str().unwrap_or_default();
            if cipher_name_matches(token, name) {
                hit = true;
                if !selected.iter().any(|s| s.suite() == suite.suite()) {
                    selected.push(*suite);
                }
            }
        }
        if !hit {
            unmatched.push(token);
        }
    }
    if selected.is_empty() {
        let detail = if unmatched.is_empty() {
            spec.to_string()
        } else {
            unmatched.join(":")
        };
        return Err(TransportError::Tls(format!(
            "no cipher can be selected: no supported cipher suite matches '{detail}'"
        )));
    }
    let mut provider = base.clone();
    provider.cipher_suites = selected;
    Ok(provider)
}

/// `--ciphers` name matching: case-insensitive, `-` and `_` are
/// interchangeable, and the `TLS_`/`TLS13_` prefix is optional (so the
/// IANA name `TLS_AES_256_GCM_SHA384` selects `TLS13_AES_256_GCM_SHA384`).
fn cipher_name_matches(token: &str, suite_name: &str) -> bool {
    let token = token.to_ascii_uppercase().replace('-', "_");
    token == suite_name || strip_tls_prefix(&token) == strip_tls_prefix(suite_name)
}

fn strip_tls_prefix(name: &str) -> &str {
    name.strip_prefix("TLS13_")
        .or_else(|| name.strip_prefix("TLS_"))
        .unwrap_or(name)
}

/// Load the client private key, decrypting a PKCS#8-encrypted PEM with
/// the resolved passphrase. Legacy OpenSSL encrypted PEM (`DEK-Info`)
/// is not supported.
fn load_client_key(
    key_bytes: &[u8],
    key_path: &std::path::Path,
    passphrase: Option<&str>,
) -> Result<rustls_pki_types::PrivateKeyDer<'static>, TransportError> {
    let text = String::from_utf8_lossy(key_bytes);
    if text.contains(PEM_PKCS8_ENCRYPTED_BEGIN) {
        return decrypt_pkcs8_key(&text, key_path, passphrase);
    }
    if text.contains("Proc-Type: 4,ENCRYPTED") {
        return Err(TransportError::Tls(format!(
            "client key {} uses the legacy OpenSSL encrypted PEM format; \
             only PKCS#8 encrypted keys are supported \
             (re-encrypt with: openssl pkcs8 -topk8 -v2 aes-256-cbc)",
            key_path.display()
        )));
    }
    rustls_pki_types::PrivateKeyDer::from_pem_slice(key_bytes)
        .map_err(|error| TransportError::Tls(format!("invalid client key: {error}")))
}

fn decrypt_pkcs8_key(
    text: &str,
    key_path: &std::path::Path,
    passphrase: Option<&str>,
) -> Result<rustls_pki_types::PrivateKeyDer<'static>, TransportError> {
    let Some(passphrase) = passphrase else {
        return Err(TransportError::Tls(format!(
            "client key {} is encrypted and no passphrase is available; \
             pass --cert-key-pass or allow the passphrase prompt",
            key_path.display()
        )));
    };
    let block =
        pem_block(text, PEM_PKCS8_ENCRYPTED_BEGIN, PEM_PKCS8_ENCRYPTED_END).ok_or_else(|| {
            TransportError::Tls(format!(
                "invalid encrypted client key {}: truncated PEM block",
                key_path.display()
            ))
        })?;
    let (_, document) = pkcs8::SecretDocument::from_pem(block).map_err(|error| {
        TransportError::Tls(format!(
            "invalid encrypted client key {}: {error}",
            key_path.display()
        ))
    })?;
    let info = pkcs8::EncryptedPrivateKeyInfo::try_from(document.as_bytes()).map_err(|error| {
        TransportError::Tls(format!(
            "invalid encrypted client key {}: {error}",
            key_path.display()
        ))
    })?;
    let decrypted = info.decrypt(passphrase).map_err(|_| {
        TransportError::Tls(format!(
            "could not decrypt client key {}: wrong passphrase or unsupported encryption",
            key_path.display()
        ))
    })?;
    let key = rustls_pki_types::PrivatePkcs8KeyDer::from(decrypted.as_bytes().to_vec());
    Ok(rustls_pki_types::PrivateKeyDer::Pkcs8(key))
}

/// The first `begin`…`end` section of a PEM file, boundaries included.
fn pem_block<'a>(text: &'a str, begin: &str, end: &str) -> Option<&'a str> {
    let start = text.find(begin)?;
    let stop = text[start..].find(end)? + start + end.len();
    Some(&text[start..stop])
}

fn client_config(options: &TlsOptions) -> Result<ClientConfig, TransportError> {
    let versions = protocol_versions(options.version);
    let provider = rustls::crypto::CryptoProvider::get_default()
        .cloned()
        .unwrap_or_else(|| Arc::new(rustls::crypto::aws_lc_rs::default_provider()));
    let provider = match &options.ciphers {
        Some(spec) => Arc::new(restrict_ciphers(&provider, spec)?),
        None => provider,
    };
    let builder = ClientConfig::builder_with_provider(provider.clone())
        .with_protocol_versions(&versions)
        .map_err(|error| TransportError::Tls(error.to_string()))?;

    let builder = match &options.verify {
        Verification::Platform => {
            let verifier = rustls_platform_verifier::Verifier::new(provider)
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
            let key = load_client_key(&key_bytes, key_path, options.cert_key_pass.as_deref())?;
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

#[cfg(test)]
pub(crate) mod fixtures {
    //! TLS material generated once with the openssl CLI (EC P-256 keys,
    //! 20-year validity): a `localhost`/`127.0.0.1` server identity, a
    //! self-signed client identity, plus PKCS#8-encrypted and legacy
    //! OpenSSL-encrypted copies of the client key.

    pub(crate) const SERVER_KEY: &str = "-----BEGIN PRIVATE KEY-----
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgJ4v1G5kfantVyqPM
MTP0smsD+jHrj8HxulE6njzYZh+hRANCAATTBoliRirRbmWRfjFFfEL97XniT/uN
+xHvSG3v+E2uGqtv+/hvEAGOLFSfb+qieCGcHqXNxsBJrlBpp3T40+Rm
-----END PRIVATE KEY-----
";

    pub(crate) const SERVER_CERT: &str = "-----BEGIN CERTIFICATE-----
MIIBljCCATygAwIBAgIUVAL6g3IsCzwAiAGZOofB5tsv/BowCgYIKoZIzj0EAwIw
FDESMBAGA1UEAwwJbG9jYWxob3N0MB4XDTI2MDcwNzA1MjkwOVoXDTQ2MDcwMjA1
MjkwOVowFDESMBAGA1UEAwwJbG9jYWxob3N0MFkwEwYHKoZIzj0CAQYIKoZIzj0D
AQcDQgAE0waJYkYq0W5lkX4xRXxC/e154k/7jfsR70ht7/hNrhqrb/v4bxABjixU
n2/qonghnB6lzcbASa5Qaad0+NPkZqNsMGowHQYDVR0OBBYEFOlanzxISsQNDfdM
DnDTeUZRF3dtMB8GA1UdIwQYMBaAFOlanzxISsQNDfdMDnDTeUZRF3dtMBoGA1Ud
EQQTMBGCCWxvY2FsaG9zdIcEfwAAATAMBgNVHRMBAf8EAjAAMAoGCCqGSM49BAMC
A0gAMEUCIFRpdd78EiqmT7Rmo16iWa5xsN9Vltvh04hnRleCnXKiAiEAlm2vclmu
nWitABeUgPcxx62jECDG4WvoYOP5a/6/AjI=
-----END CERTIFICATE-----
";

    pub(crate) const CLIENT_KEY: &str = "-----BEGIN PRIVATE KEY-----
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgBb3/a0Qt0LWanDu6
b2b4QuqGNBjzv3m8/1ksjcibf6ehRANCAASNGhCkri5So214Eou7uQpCMYBU0NLn
CAsPwBGGnWan3zJePNJksgMwt377N+in4zze41QIaqnWcD/eFIrLzOHB
-----END PRIVATE KEY-----
";

    pub(crate) const CLIENT_CERT: &str = "-----BEGIN CERTIFICATE-----
MIIBjDCCATGgAwIBAgIUBjqVUfF1s4xLg4QnYqQIv2gta9YwCgYIKoZIzj0EAwIw
GzEZMBcGA1UEAwwQZnVybC10ZXN0LWNsaWVudDAeFw0yNjA3MDcwNTE5MzRaFw00
NjA3MDIwNTE5MzRaMBsxGTAXBgNVBAMMEGZ1cmwtdGVzdC1jbGllbnQwWTATBgcq
hkjOPQIBBggqhkjOPQMBBwNCAASNGhCkri5So214Eou7uQpCMYBU0NLnCAsPwBGG
nWan3zJePNJksgMwt377N+in4zze41QIaqnWcD/eFIrLzOHBo1MwUTAdBgNVHQ4E
FgQUAHj9qukXPqccjwRBGrd9/O+8rD0wHwYDVR0jBBgwFoAUAHj9qukXPqccjwRB
Grd9/O+8rD0wDwYDVR0TAQH/BAUwAwEB/zAKBggqhkjOPQQDAgNJADBGAiEAm71h
smjX43I+mKEQolgdai+KHtFbrQw2cxhkUe9TmywCIQD+oJGtWIdXRePWjXt3l07V
cBz2ClEJ9JqY0ymD/1tpjQ==
-----END CERTIFICATE-----
";

    /// `CLIENT_KEY` as PKCS#8 EncryptedPrivateKeyInfo
    /// (`openssl pkcs8 -topk8 -v2 aes-256-cbc`), passphrase `secret`.
    pub(crate) const CLIENT_KEY_ENCRYPTED: &str = "-----BEGIN ENCRYPTED PRIVATE KEY-----
MIH0MF8GCSqGSIb3DQEFDTBSMDEGCSqGSIb3DQEFDDAkBBBTDWU2XOw2bt5eLOL4
AhQ6AgIIADAMBggqhkiG9w0CCQUAMB0GCWCGSAFlAwQBKgQQ9KFeXkQ7UORse2hZ
niG6TwSBkCbYDxB7ztxrLWh2UPRmq67DH0893x4AyiLujNy3UPOg3lHxzgiKpkb5
+TNKVyjAR/+kvLrjgJpIfb99/AAb7UCNjl/nBAwBRkw35JRSmIPmSUCZ7sY2kxMo
grXXTjTZLAwXEOnGKZWxqV8GnnK0oLMPmp9zQcFH/+Fcl4iIJhRWpK9GIAFH8cvl
Y8gw/QYHRg==
-----END ENCRYPTED PRIVATE KEY-----
";

    /// `CLIENT_KEY` in the legacy OpenSSL encrypted PEM format
    /// (`openssl ec -aes256`), passphrase `secret`. Unsupported.
    pub(crate) const CLIENT_KEY_LEGACY: &str = "-----BEGIN EC PRIVATE KEY-----
Proc-Type: 4,ENCRYPTED
DEK-Info: AES-256-CBC,7C76DB168ADCCEB50C9C6125EB704163

Rjoooi0ImZTL74SmqufXA/xLlcIrNEJq6uy1I0ofNsfCy+9lSEc2QG2V1rMerKNh
ql/eK1i6vKAfLY8nlsSZs3sYyxUbvCqXP3oJ3jY58Pks6aVm6VzqrFfS/zAunjma
EmSYXxpru3QuSvTbu3XwokGJG6eKUPAKXvQmhEEDB4c=
-----END EC PRIVATE KEY-----
";

    pub(crate) const CLIENT_KEY_PASSPHRASE: &str = "secret";
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider() -> rustls::crypto::CryptoProvider {
        rustls::crypto::aws_lc_rs::default_provider()
    }

    fn key_path() -> &'static std::path::Path {
        std::path::Path::new("client.key")
    }

    #[test]
    fn cipher_tokens_match_loosely() {
        let suite = "TLS13_AES_256_GCM_SHA384";
        assert!(cipher_name_matches("TLS13_AES_256_GCM_SHA384", suite));
        // The IANA name for the same suite.
        assert!(cipher_name_matches("TLS_AES_256_GCM_SHA384", suite));
        assert!(cipher_name_matches("tls_aes_256_gcm_sha384", suite));
        assert!(cipher_name_matches("AES-256-GCM-SHA384", suite));
        assert!(!cipher_name_matches("AES_128_GCM_SHA256", suite));
        assert!(cipher_name_matches(
            "ECDHE_RSA_WITH_AES_128_GCM_SHA256",
            "TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256"
        ));
    }

    #[test]
    fn cipher_filter_keeps_only_the_selection() {
        let restricted = restrict_ciphers(&provider(), "tls_aes_256_gcm_sha384:BOGUS").unwrap();
        assert_eq!(restricted.cipher_suites.len(), 1);
        assert_eq!(
            restricted.cipher_suites[0].suite().as_str(),
            Some("TLS13_AES_256_GCM_SHA384")
        );
    }

    #[test]
    fn cipher_filter_with_no_match_names_the_input() {
        let error = restrict_ciphers(&provider(), "BOGUS:ALSO_BOGUS").unwrap_err();
        let TransportError::Tls(message) = error else {
            panic!("expected a TLS error");
        };
        assert!(message.contains("no cipher can be selected"), "{message}");
        assert!(message.contains("BOGUS:ALSO_BOGUS"), "{message}");
    }

    #[test]
    fn encrypted_pkcs8_key_decrypts_to_the_plain_key() {
        let decrypted = load_client_key(
            fixtures::CLIENT_KEY_ENCRYPTED.as_bytes(),
            key_path(),
            Some(fixtures::CLIENT_KEY_PASSPHRASE),
        )
        .expect("decrypt");
        let plain = load_client_key(fixtures::CLIENT_KEY.as_bytes(), key_path(), None)
            .expect("plain key parses");
        assert_eq!(decrypted.secret_der(), plain.secret_der());
    }

    #[test]
    fn wrong_passphrase_is_a_clear_tls_error() {
        let error = load_client_key(
            fixtures::CLIENT_KEY_ENCRYPTED.as_bytes(),
            key_path(),
            Some("not-the-passphrase"),
        )
        .unwrap_err();
        let TransportError::Tls(message) = error else {
            panic!("expected a TLS error");
        };
        assert!(message.contains("could not decrypt"), "{message}");
    }

    #[test]
    fn encrypted_key_without_passphrase_points_at_the_flag() {
        let error = load_client_key(fixtures::CLIENT_KEY_ENCRYPTED.as_bytes(), key_path(), None)
            .unwrap_err();
        let TransportError::Tls(message) = error else {
            panic!("expected a TLS error");
        };
        assert!(message.contains("--cert-key-pass"), "{message}");
    }

    #[test]
    fn legacy_openssl_encrypted_keys_are_rejected() {
        let error = load_client_key(
            fixtures::CLIENT_KEY_LEGACY.as_bytes(),
            key_path(),
            Some(fixtures::CLIENT_KEY_PASSPHRASE),
        )
        .unwrap_err();
        let TransportError::Tls(message) = error else {
            panic!("expected a TLS error");
        };
        assert!(message.contains("PKCS#8"), "{message}");
    }

    #[test]
    fn explicit_passphrase_wins_without_touching_the_file() {
        let pass = resolve_key_passphrase(
            Some(std::path::Path::new("/nonexistent/enc.key")),
            Some("from-flag".to_string()),
            true,
        );
        assert_eq!(pass.as_deref(), Some("from-flag"));
    }

    #[test]
    fn suppressed_prompt_resolves_to_no_passphrase() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("enc.key");
        std::fs::write(&path, fixtures::CLIENT_KEY_ENCRYPTED).unwrap();
        assert!(resolve_key_passphrase(Some(&path), None, false).is_none());
    }

    #[test]
    fn plain_keys_never_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("plain.key");
        std::fs::write(&path, fixtures::CLIENT_KEY).unwrap();
        assert!(resolve_key_passphrase(Some(&path), None, true).is_none());
    }
}
