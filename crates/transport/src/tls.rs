//! TLS 1.3 mútuo com certificados efêmeros e código de verificação (SAS).
//!
//! Nenhum certificado é confiável por si só. A autenticação acontece de duas formas:
//! 1. Primeiro contato: ambos os usuários comparam o SAS, derivado do segredo
//!    exportado da sessão TLS. Um intermediário teria chaves diferentes em cada
//!    lado e os códigos divergiriam.
//! 2. Reconexão: a impressão digital do par, já confirmada, fica fixada ([`Pinned`]).

use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{ring as ring_provider, WebPkiSupportedAlgorithms};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{
    ClientConfig, DigitallySignedStruct, DistinguishedName, ServerConfig, SignatureScheme,
};
use sha2::{Digest, Sha256};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;
use tokio_rustls::{TlsAcceptor, TlsConnector, TlsStream};

use crate::TransportError;

pub type Fingerprint = [u8; 32];
/// Impressão digital do par já confirmado pelo usuário (compartilhada entre tarefas).
pub type Pinned = Arc<Mutex<Option<Fingerprint>>>;

const SAS_LABEL: &[u8] = b"windows-flow/sas/v1";
const TLS_SETUP_TIMEOUT: Duration = Duration::from_secs(10);

/// Identidade efêmera (par de chaves + certificado autoassinado) deste processo.
pub struct Identity {
    cert: CertificateDer<'static>,
    key: Vec<u8>,
}

impl Identity {
    pub fn generate() -> Result<Self, TransportError> {
        let ck = rcgen::generate_simple_self_signed(vec!["windows-flow".to_string()])
            .map_err(|e| TransportError::Tls(e.to_string()))?;
        Ok(Self {
            cert: ck.cert.der().clone(),
            key: ck.signing_key.serialize_der(),
        })
    }

    pub fn fingerprint(&self) -> Fingerprint {
        Sha256::digest(self.cert.as_ref()).into()
    }

    fn key_der(&self) -> PrivateKeyDer<'static> {
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(self.key.clone()))
    }
}

/// Canal TLS estabelecido, ainda não confiável até o usuário confirmar o SAS.
pub struct SecureChannel {
    pub io: TlsStream<TcpStream>,
    /// Código de 6 dígitos ("123 456") que ambos os usuários comparam.
    pub sas: String,
    pub peer_fingerprint: Fingerprint,
    pub peer_addr: SocketAddr,
    /// Verdadeiro se o par bate com a impressão digital fixada (dispensa confirmação).
    pub pinned_match: bool,
}

#[derive(Debug)]
struct PinVerifier {
    pinned: Option<Fingerprint>,
    algs: WebPkiSupportedAlgorithms,
}

impl PinVerifier {
    fn new(pinned: Option<Fingerprint>) -> Arc<Self> {
        Arc::new(Self {
            pinned,
            algs: ring_provider::default_provider().signature_verification_algorithms,
        })
    }

    fn check(&self, cert: &CertificateDer<'_>) -> Result<(), rustls::Error> {
        let fp: Fingerprint = Sha256::digest(cert.as_ref()).into();
        match self.pinned {
            Some(p) if p != fp => Err(rustls::Error::General(
                "impressão digital do par não confere".into(),
            )),
            _ => Ok(()),
        }
    }
}

impl ServerCertVerifier for PinVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        self.check(end_entity)
            .map(|_| ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.algs)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.algs)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algs.supported_schemes()
    }
}

impl ClientCertVerifier for PinVerifier {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> Result<ClientCertVerified, rustls::Error> {
        self.check(end_entity)
            .map(|_| ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.algs)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.algs)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algs.supported_schemes()
    }
}

/// Endereços aceitos: loopback, privados, link-local e ULA IPv6. Nunca a internet.
pub fn is_lan(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback() || v4.is_private() || v4.is_link_local(),
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_lan(IpAddr::V4(v4));
            }
            let seg0 = v6.segments()[0];
            v6.is_loopback() || (seg0 & 0xfe00) == 0xfc00 || (seg0 & 0xffc0) == 0xfe80
        }
    }
}

fn sas_from(secret: &[u8; 32]) -> String {
    let n = u32::from_be_bytes([secret[0], secret[1], secret[2], secret[3]]) % 1_000_000;
    format!("{:03} {:03}", n / 1000, n % 1000)
}

fn peer_fp(certs: Option<&[CertificateDer<'static>]>) -> Result<Fingerprint, TransportError> {
    let cert = certs
        .and_then(|c| c.first())
        .ok_or_else(|| TransportError::Tls("par sem certificado".into()))?;
    Ok(Sha256::digest(cert.as_ref()).into())
}

const TLS13_ONLY: &[&rustls::SupportedProtocolVersion] = &[&rustls::version::TLS13];

fn tls13_versions() -> &'static [&'static rustls::SupportedProtocolVersion] {
    TLS13_ONLY
}

/// Conecta a um par. `pinned` exige que o par seja exatamente aquele já confirmado.
pub async fn connect(
    addr: SocketAddr,
    identity: &Identity,
    pinned: Option<Fingerprint>,
) -> Result<SecureChannel, TransportError> {
    if !is_lan(addr.ip()) {
        return Err(TransportError::NotLan);
    }
    let provider = Arc::new(ring_provider::default_provider());
    let config = ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(tls13_versions())?
        .dangerous()
        .with_custom_certificate_verifier(PinVerifier::new(pinned))
        .with_client_auth_cert(vec![identity.cert.clone()], identity.key_der())?;
    let connector = TlsConnector::from(Arc::new(config));
    let name = ServerName::try_from("windows-flow").expect("nome estático válido");

    let tcp = timeout(TLS_SETUP_TIMEOUT, TcpStream::connect(addr))
        .await
        .map_err(|_| TransportError::Timeout)??;
    tcp.set_nodelay(true)?;
    let tls = timeout(TLS_SETUP_TIMEOUT, connector.connect(name, tcp))
        .await
        .map_err(|_| TransportError::Timeout)??;

    let (_, conn) = tls.get_ref();
    let secret = conn.export_keying_material([0u8; 32], SAS_LABEL, None)?;
    let fp = peer_fp(conn.peer_certificates())?;
    Ok(SecureChannel {
        sas: sas_from(&secret),
        pinned_match: pinned == Some(fp),
        peer_fingerprint: fp,
        peer_addr: addr,
        io: TlsStream::Client(tls),
    })
}

/// Escuta conexões de pares na LAN.
pub struct Listener {
    tcp: TcpListener,
    cert: CertificateDer<'static>,
    key: Vec<u8>,
}

impl Listener {
    pub async fn bind(addr: SocketAddr, identity: &Identity) -> Result<Self, TransportError> {
        Ok(Self {
            tcp: TcpListener::bind(addr).await?,
            cert: identity.cert.clone(),
            key: identity.key.clone(),
        })
    }

    pub fn local_addr(&self) -> Result<SocketAddr, TransportError> {
        Ok(self.tcp.local_addr()?)
    }

    /// Aceita o próximo par válido. Conexões de fora da LAN ou com handshake
    /// inválido (inclusive impressão digital divergente da fixada) são descartadas.
    pub async fn accept(&self, pinned: &Pinned) -> Result<SecureChannel, TransportError> {
        loop {
            let (tcp, addr) = self.tcp.accept().await?;
            if !is_lan(addr.ip()) {
                continue;
            }
            let pin = *pinned.lock().unwrap_or_else(|e| e.into_inner());
            if let Ok(chan) = self.handshake(tcp, addr, pin).await {
                return Ok(chan);
            }
        }
    }

    async fn handshake(
        &self,
        tcp: TcpStream,
        addr: SocketAddr,
        pinned: Option<Fingerprint>,
    ) -> Result<SecureChannel, TransportError> {
        tcp.set_nodelay(true)?;
        let provider = Arc::new(ring_provider::default_provider());
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(self.key.clone()));
        let config = ServerConfig::builder_with_provider(provider)
            .with_protocol_versions(tls13_versions())?
            .with_client_cert_verifier(PinVerifier::new(pinned))
            .with_single_cert(vec![self.cert.clone()], key)?;
        let acceptor = TlsAcceptor::from(Arc::new(config));
        let tls = timeout(TLS_SETUP_TIMEOUT, acceptor.accept(tcp))
            .await
            .map_err(|_| TransportError::Timeout)??;

        let (_, conn) = tls.get_ref();
        let secret = conn.export_keying_material([0u8; 32], SAS_LABEL, None)?;
        let fp = peer_fp(conn.peer_certificates())?;
        Ok(SecureChannel {
            sas: sas_from(&secret),
            pinned_match: pinned == Some(fp),
            peer_fingerprint: fp,
            peer_addr: addr,
            io: TlsStream::Server(tls),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loopback() -> SocketAddr {
        "127.0.0.1:0".parse().unwrap()
    }

    #[tokio::test]
    async fn both_sides_derive_the_same_sas() {
        let (id_a, id_b) = (Identity::generate().unwrap(), Identity::generate().unwrap());
        let listener = Listener::bind(loopback(), &id_a).await.unwrap();
        let addr = listener.local_addr().unwrap();
        let pinned: Pinned = Arc::default();
        let server = tokio::spawn(async move { listener.accept(&pinned).await.unwrap() });
        let client = connect(addr, &id_b, None).await.unwrap();
        let server = server.await.unwrap();

        assert_eq!(client.sas, server.sas);
        assert_eq!(client.sas.len(), 7);
        assert_eq!(client.peer_fingerprint, id_a.fingerprint());
        assert_eq!(server.peer_fingerprint, id_b.fingerprint());
        assert!(!client.pinned_match && !server.pinned_match);
    }

    #[tokio::test]
    async fn pinned_peer_is_recognized() {
        let (id_a, id_b) = (Identity::generate().unwrap(), Identity::generate().unwrap());
        let listener = Listener::bind(loopback(), &id_a).await.unwrap();
        let addr = listener.local_addr().unwrap();
        let pinned: Pinned = Arc::new(Mutex::new(Some(id_b.fingerprint())));
        let server = tokio::spawn(async move { listener.accept(&pinned).await.unwrap() });
        let client = connect(addr, &id_b, Some(id_a.fingerprint()))
            .await
            .unwrap();
        assert!(client.pinned_match);
        assert!(server.await.unwrap().pinned_match);
    }

    #[tokio::test]
    async fn client_rejects_wrong_pinned_server() {
        let (id_a, id_b) = (Identity::generate().unwrap(), Identity::generate().unwrap());
        let listener = Listener::bind(loopback(), &id_a).await.unwrap();
        let addr = listener.local_addr().unwrap();
        let pinned: Pinned = Arc::default();
        let _server = tokio::spawn(async move { listener.accept(&pinned).await });
        let res = connect(addr, &id_b, Some([7u8; 32])).await;
        assert!(res.is_err());
    }

    #[test]
    fn lan_filter() {
        for ip in [
            "127.0.0.1",
            "192.168.1.5",
            "10.1.2.3",
            "172.16.0.9",
            "169.254.1.1",
            "::1",
            "fd00::1",
            "fe80::1",
        ] {
            assert!(is_lan(ip.parse().unwrap()), "{ip}");
        }
        for ip in [
            "8.8.8.8",
            "172.32.0.1",
            "2001:4860:4860::8888",
            "::ffff:8.8.8.8",
        ] {
            assert!(!is_lan(ip.parse().unwrap()), "{ip}");
        }
    }

    #[test]
    fn sas_format() {
        assert_eq!(sas_from(&[0; 32]), "000 000");
    }
}
