//! Descoberta de PCs com o app na rede local, por anúncios UDP em broadcast.
//!
//! Os anúncios só dizem "estou aqui" (nome, porta, versão): **nunca provam identidade**. Quem
//! quiser conectar ainda passa pelo código de verificação (SAS) do pareamento.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::net::UdpSocket;
use tokio::sync::mpsc::UnboundedSender;
use tokio::time::{interval, MissedTickBehavior};

/// Porta UDP dos anúncios.
pub const DISCOVERY_PORT: u16 = 47_801;
const MAGIC: &str = "windows-flow";
const ANNOUNCE_EVERY: Duration = Duration::from_secs(2);
/// Sem anúncio por este tempo, o PC é considerado fora da rede.
const EXPIRE_AFTER: Duration = Duration::from_secs(8);
const MAX_PACKET: usize = 512;
const MAX_NAME: usize = 64;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct Announcement {
    app: String,
    proto: u16,
    id: String,
    name: String,
    port: u16,
}

/// PC visto na rede.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Peer {
    pub id: String,
    pub name: String,
    pub ip: Ipv4Addr,
    pub port: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiscoveryEvent {
    /// Apareceu (ou voltou depois de sumir).
    Found(Peer),
    /// Parou de anunciar.
    Lost(String),
    /// Diagnóstico periódico: escuta ativa, interfaces usadas e anúncios de outros PCs recebidos.
    Status {
        listening: bool,
        interfaces: usize,
        received: u64,
    },
}

fn lan(ip: Ipv4Addr) -> bool {
    ip.is_private() || ip.is_link_local()
}

pub fn encode(id: &str, name: &str, port: u16, proto: u16) -> Vec<u8> {
    let name: String = name
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_NAME)
        .collect();
    serde_json::to_vec(&Announcement {
        app: MAGIC.into(),
        proto,
        id: id.into(),
        name,
        port,
    })
    .unwrap_or_default()
}

/// Valida um pacote recebido de `src`. Descarta o que não for do app, de outra versão do
/// protocolo, de fora da LAN, malformado ou do próprio PC (`own_id`).
pub fn parse(bytes: &[u8], src: IpAddr, proto: u16, own_id: &str) -> Option<Peer> {
    if bytes.len() > MAX_PACKET {
        return None;
    }
    let IpAddr::V4(ip) = src else {
        return None;
    };
    if !lan(ip) {
        return None;
    }
    let a: Announcement = serde_json::from_slice(bytes).ok()?;
    let name_ok = !a.name.is_empty()
        && a.name.chars().count() <= MAX_NAME
        && !a.name.chars().any(char::is_control);
    if a.app != MAGIC
        || a.proto != proto
        || a.id == own_id
        || a.id.is_empty()
        || a.id.len() > 64
        || a.port == 0
        || !name_ok
    {
        return None;
    }
    Some(Peer {
        id: a.id,
        name: a.name,
        ip,
        port: a.port,
    })
}

/// Tabela de PCs vistos: decide quando um anúncio é novidade e quando um PC sumiu.
#[derive(Default)]
pub struct Table {
    seen: HashMap<String, (Peer, Instant)>,
}

impl Table {
    /// `true` se o PC é novo ou mudou de endereço/nome.
    pub fn observe(&mut self, p: Peer, now: Instant) -> bool {
        let changed = self.seen.get(&p.id).is_none_or(|(old, _)| *old != p);
        self.seen.insert(p.id.clone(), (p, now));
        changed
    }

    /// Remove e devolve os ids que não anunciam há mais de `after`.
    pub fn expire(&mut self, now: Instant, after: Duration) -> Vec<String> {
        let gone: Vec<String> = self
            .seen
            .iter()
            .filter(|(_, (_, t))| now.duration_since(*t) > after)
            .map(|(id, _)| id.clone())
            .collect();
        for id in &gone {
            self.seen.remove(id);
        }
        gone
    }
}

/// Endereço IPv4 da LAN de uma placa de rede.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct LanAddr {
    pub name: String,
    pub ip: Ipv4Addr,
}

/// Placas virtuais (Hyper-V/WSL, VMware, VirtualBox, Docker, VPN) têm IPs privados que o outro
/// PC não alcança: ficam por último.
pub fn is_virtual_adapter(name: &str) -> bool {
    let n = name.to_lowercase();
    [
        "vethernet",
        "vmware",
        "virtualbox",
        "vbox",
        "hyper-v",
        "wsl",
        "docker",
        "vpn",
        "tap-",
        "tun",
        "tailscale",
        "zerotier",
        "loopback",
        "pseudo",
        "bluetooth",
        "npcap",
    ]
    .iter()
    .any(|k| n.contains(k))
}

/// IP da interface que sai pela rota padrão (UDP "conectado" não envia nenhum pacote).
fn default_route_ipv4() -> Option<Ipv4Addr> {
    for target in ["8.8.8.8:53", "1.1.1.1:53", "192.168.255.255:9"] {
        let Ok(s) = std::net::UdpSocket::bind("0.0.0.0:0") else {
            continue;
        };
        if s.connect(target).is_err() {
            continue;
        }
        if let Some(IpAddr::V4(ip)) = s.local_addr().ok().map(|a| a.ip()) {
            if lan(ip) {
                return Some(ip);
            }
        }
    }
    None
}

/// Endereços da LAN deste PC, do mais provável (o da rota padrão, em placa real) ao menos.
pub fn lan_ipv4s() -> Vec<LanAddr> {
    let Ok(ifs) = if_addrs::get_if_addrs() else {
        return Vec::new();
    };
    let mut all: Vec<LanAddr> = ifs
        .into_iter()
        .filter_map(|i| match i.addr {
            if_addrs::IfAddr::V4(a) if !i.is_loopback() && lan(a.ip) => Some(LanAddr {
                name: i.name,
                ip: a.ip,
            }),
            _ => None,
        })
        .collect();
    let route = default_route_ipv4();
    // Placa real antes de virtual; dentro disso, a da rota padrão primeiro.
    all.sort_by_key(|a| (is_virtual_adapter(&a.name), Some(a.ip) != route));
    all
}

/// O endereço que o outro PC deve usar para chegar a este.
pub fn primary_ipv4() -> Option<Ipv4Addr> {
    lan_ipv4s()
        .first()
        .map(|a| a.ip)
        .or_else(default_route_ipv4)
}

/// Endereços de broadcast de cada interface IPv4 da LAN (um por placa/Wi-Fi/VPN): o
/// broadcast limitado (255.255.255.255) só sai por uma placa e perde o outro PC em máquinas
/// com várias interfaces (VPN, WSL, Hyper-V).
fn lan_broadcasts() -> Vec<(Ipv4Addr, Ipv4Addr)> {
    let Ok(ifs) = if_addrs::get_if_addrs() else {
        return Vec::new();
    };
    ifs.into_iter()
        .filter_map(|i| match i.addr {
            if_addrs::IfAddr::V4(a) if !i.is_loopback() && lan(a.ip) => {
                let bcast = a
                    .broadcast
                    .unwrap_or_else(|| Ipv4Addr::from(u32::from(a.ip) | !u32::from(a.netmask)));
                Some((a.ip, bcast))
            }
            _ => None,
        })
        .collect()
}

/// Envia o anúncio por cada interface. Retorna quantas interfaces foram usadas.
fn announce_all(packet: &[u8]) -> usize {
    let mut used = 0;
    for (ip, bcast) in lan_broadcasts() {
        let Ok(sock) = std::net::UdpSocket::bind((ip, 0)) else {
            continue;
        };
        let _ = sock.set_broadcast(true);
        let ok = sock.send_to(packet, (bcast, DISCOVERY_PORT)).is_ok();
        let _ = sock.send_to(packet, (Ipv4Addr::BROADCAST, DISCOVERY_PORT));
        if ok {
            used += 1;
        }
    }
    used
}

/// Anuncia este PC e escuta os outros até a tarefa ser abortada. Se a porta estiver ocupada
/// (outro app/instância), informa no `Status` (`listening: false`); a conexão manual por IP
/// continua valendo.
pub async fn run(
    own_id: String,
    name: String,
    port: u16,
    proto: u16,
    tx: UnboundedSender<DiscoveryEvent>,
) {
    let sock = UdpSocket::bind(("0.0.0.0", DISCOVERY_PORT)).await.ok();
    let _ = tx.send(DiscoveryEvent::Status {
        listening: sock.is_some(),
        interfaces: 0,
        received: 0,
    });
    let packet = encode(&own_id, &name, port, proto);
    let mut table = Table::default();
    let mut announce = interval(ANNOUNCE_EVERY);
    announce.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut sweep = interval(Duration::from_secs(1));
    sweep.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut report = interval(Duration::from_secs(5));
    report.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let (mut received, mut used) = (0u64, 0usize);
    let mut buf = [0u8; MAX_PACKET + 1];

    loop {
        tokio::select! {
            _ = announce.tick() => {
                used = announce_all(&packet);
            }
            r = async {
                match &sock {
                    Some(s) => s.recv_from(&mut buf).await,
                    None => std::future::pending().await,
                }
            } => {
                let Ok((n, src)) = r else { continue };
                if let Some(peer) = parse(&buf[..n], src.ip(), proto, &own_id) {
                    received += 1;
                    if table.observe(peer.clone(), Instant::now())
                        && tx.send(DiscoveryEvent::Found(peer)).is_err()
                    {
                        return;
                    }
                }
            }
            _ = sweep.tick() => {
                for id in table.expire(Instant::now(), EXPIRE_AFTER) {
                    if tx.send(DiscoveryEvent::Lost(id)).is_err() {
                        return;
                    }
                }
            }
            _ = report.tick() => {
                let st = DiscoveryEvent::Status { listening: sock.is_some(), interfaces: used, received };
                if tx.send(st).is_err() {
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: IpAddr = IpAddr::V4(Ipv4Addr::new(192, 168, 0, 20));

    #[test]
    fn roundtrip_and_filters() {
        let pkt = encode("abc", "PC-SALA", 47_800, 3);
        let p = parse(&pkt, SRC, 3, "me").unwrap();
        assert_eq!(
            (p.name.as_str(), p.port, p.ip),
            ("PC-SALA", 47_800, Ipv4Addr::new(192, 168, 0, 20))
        );
        // O próprio anúncio, outra versão do protocolo e IP fora da LAN são ignorados.
        assert!(parse(&pkt, SRC, 3, "abc").is_none());
        assert!(parse(&pkt, SRC, 2, "me").is_none());
        assert!(parse(&pkt, IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)), 3, "me").is_none());
        assert!(parse(&pkt, "::1".parse().unwrap(), 3, "me").is_none());
    }

    #[test]
    fn rejects_garbage_and_bad_fields() {
        assert!(parse(b"nada", SRC, 3, "me").is_none());
        assert!(parse(&[b' '; 600], SRC, 3, "me").is_none());
        let other = br#"{"app":"outro","proto":3,"id":"x","name":"n","port":1}"#;
        assert!(parse(other, SRC, 3, "me").is_none());
        let zero_port = br#"{"app":"windows-flow","proto":3,"id":"x","name":"n","port":0}"#;
        assert!(parse(zero_port, SRC, 3, "me").is_none());
        let ctrl = br#"{"app":"windows-flow","proto":3,"id":"x","name":"a\u0007b","port":5}"#;
        assert!(parse(ctrl, SRC, 3, "me").is_none());
    }

    #[test]
    fn table_reports_new_changed_and_expired() {
        let mut t = Table::default();
        let peer = Peer {
            id: "a".into(),
            name: "A".into(),
            ip: Ipv4Addr::new(10, 0, 0, 2),
            port: 47_800,
        };
        let t0 = Instant::now();
        assert!(t.observe(peer.clone(), t0));
        assert!(!t.observe(peer.clone(), t0), "repetido não é novidade");
        let moved = Peer {
            ip: Ipv4Addr::new(10, 0, 0, 3),
            ..peer.clone()
        };
        assert!(t.observe(moved, t0));
        assert!(t.expire(t0, Duration::from_secs(8)).is_empty());
        assert_eq!(
            t.expire(t0 + Duration::from_secs(9), Duration::from_secs(8)),
            vec!["a"]
        );
        assert!(
            t.observe(peer, t0 + Duration::from_secs(10)),
            "voltou à rede"
        );
    }
}

#[cfg(test)]
mod net_tests {
    use super::*;

    #[test]
    fn finds_at_least_one_broadcast_when_on_a_lan() {
        // Em máquinas sem rede privada a lista pode ser vazia; só confere a consistência.
        for (ip, b) in lan_broadcasts() {
            assert!(lan(ip));
            assert_eq!(
                u32::from(b) & u32::from(ip) & 0xff00_0000,
                u32::from(ip) & 0xff00_0000
            );
        }
    }
}

#[cfg(test)]
mod ip_tests {
    use super::*;

    #[test]
    fn virtual_adapters_are_recognised() {
        for n in [
            "vEthernet (WSL)",
            "VMware Network Adapter VMnet8",
            "VirtualBox Host-Only",
            "Tailscale",
        ] {
            assert!(is_virtual_adapter(n), "{n}");
        }
        for n in ["Ethernet", "Wi-Fi", "Ethernet 2"] {
            assert!(!is_virtual_adapter(n), "{n}");
        }
    }

    #[test]
    fn real_adapters_come_before_virtual_ones() {
        let list = lan_ipv4s();
        let first_virtual = list.iter().position(|a| is_virtual_adapter(&a.name));
        let last_real = list.iter().rposition(|a| !is_virtual_adapter(&a.name));
        if let (Some(v), Some(r)) = (first_virtual, last_real) {
            assert!(r < v, "{list:?}");
        }
    }
}
