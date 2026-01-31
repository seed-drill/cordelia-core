//! TLS configuration for QUIC transport.
//!
//! Self-signed X.509 certificates from Ed25519 keypairs via rcgen.
//! ALPN protocol: "cordelia/1".
//! Client skips server cert verification (peer auth at handshake layer).

use std::sync::Arc;

/// ALPN protocol identifier.
pub const ALPN_CORDELIA: &[u8] = b"cordelia/1";

/// Generate a self-signed X.509 certificate from an Ed25519 PKCS#8 DER keypair.
///
/// Returns (certificate DER bytes, private key DER bytes).
pub fn generate_self_signed_cert(
    pkcs8_der: &[u8],
) -> Result<(Vec<u8>, Vec<u8>), Box<dyn std::error::Error + Send + Sync>> {
    let pkcs8_key = rustls::pki_types::PrivatePkcs8KeyDer::from(pkcs8_der.to_vec());
    let key_pair = rcgen::KeyPair::from_pkcs8_der_and_sign_algo(&pkcs8_key, &rcgen::PKCS_ED25519)?;

    let mut params = rcgen::CertificateParams::new(vec!["cordelia-node.local".to_string()])?;
    params.distinguished_name = rcgen::DistinguishedName::new();
    params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "cordelia-node");

    let cert = params.self_signed(&key_pair)?;

    Ok((cert.der().to_vec(), pkcs8_der.to_vec()))
}

/// Build a QUIC server config with the given certificate and private key.
pub fn build_server_config(
    cert_der: Vec<u8>,
    key_der: Vec<u8>,
) -> Result<quinn::ServerConfig, Box<dyn std::error::Error + Send + Sync>> {
    let cert = rustls::pki_types::CertificateDer::from(cert_der);
    let key = rustls::pki_types::PrivateKeyDer::try_from(key_der)
        .map_err(|e| format!("invalid private key DER: {e}"))?;

    let mut server_crypto = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert], key)?;

    server_crypto.alpn_protocols = vec![ALPN_CORDELIA.to_vec()];

    let mut transport = quinn::TransportConfig::default();
    transport.max_idle_timeout(Some(
        quinn::IdleTimeout::try_from(std::time::Duration::from_secs(
            crate::QUIC_IDLE_TIMEOUT_SECS,
        ))
        .expect("idle timeout within bounds"),
    ));
    transport.keep_alive_interval(Some(std::time::Duration::from_secs(
        crate::KEEPALIVE_INTERVAL_SECS / 2,
    )));

    let mut server_config = quinn::ServerConfig::with_crypto(Arc::new(
        quinn::crypto::rustls::QuicServerConfig::try_from(server_crypto)?,
    ));
    server_config.transport_config(Arc::new(transport));

    Ok(server_config)
}

/// Build a QUIC client config that skips server certificate verification.
///
/// We authenticate peers at the handshake mini-protocol layer, not TLS.
pub fn build_client_config() -> Result<quinn::ClientConfig, Box<dyn std::error::Error + Send + Sync>>
{
    let mut client_crypto = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(SkipServerVerification))
        .with_no_client_auth();

    client_crypto.alpn_protocols = vec![ALPN_CORDELIA.to_vec()];

    let mut transport = quinn::TransportConfig::default();
    transport.max_idle_timeout(Some(
        quinn::IdleTimeout::try_from(std::time::Duration::from_secs(
            crate::QUIC_IDLE_TIMEOUT_SECS,
        ))
        .expect("idle timeout within bounds"),
    ));
    transport.keep_alive_interval(Some(std::time::Duration::from_secs(
        crate::KEEPALIVE_INTERVAL_SECS / 2,
    )));

    let mut client_config = quinn::ClientConfig::new(Arc::new(
        quinn::crypto::rustls::QuicClientConfig::try_from(client_crypto)?,
    ));
    client_config.transport_config(Arc::new(transport));

    Ok(client_config)
}

/// Certificate verifier that accepts any server certificate.
/// Peer identity is verified at the Cordelia handshake layer.
#[derive(Debug)]
struct SkipServerVerification;

impl rustls::client::danger::ServerCertVerifier for SkipServerVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![rustls::SignatureScheme::ED25519]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_pkcs8_keypair() -> Vec<u8> {
        use ring::rand::SystemRandom;
        use ring::signature::Ed25519KeyPair;
        let rng = SystemRandom::new();
        Ed25519KeyPair::generate_pkcs8(&rng)
            .unwrap()
            .as_ref()
            .to_vec()
    }

    #[test]
    fn test_generate_self_signed_cert() {
        let pkcs8 = test_pkcs8_keypair();
        let (cert, key) = generate_self_signed_cert(&pkcs8).unwrap();
        assert!(!cert.is_empty());
        assert!(!key.is_empty());
    }

    #[test]
    fn test_build_server_config() {
        let pkcs8 = test_pkcs8_keypair();
        let (cert, key) = generate_self_signed_cert(&pkcs8).unwrap();
        let config = build_server_config(cert, key);
        assert!(config.is_ok());
    }

    #[test]
    fn test_build_client_config() {
        let config = build_client_config();
        assert!(config.is_ok());
    }
}
