//! Descoberta de dispositivos na LAN via mDNS (Fase 3). Anúncios nunca provam identidade.

/// Endereço de um par candidato; só vira confiável após pareamento.
pub struct Candidate {
    pub addr: std::net::SocketAddr,
}
