//! Minimal mDNS / DNS-SD browsing (WLED, Elgato, Philips Hue, Nanoleaf
//! announce themselves this way).
//!
//! One PTR question per service type is sent as a "legacy unicast" query
//! (RFC 6762 §6.7): from an ordinary port, so the responders answer
//! straight back to it. The same question also goes out from port 5353
//! when that port can be shared (Avahi, systemd-resolved or Windows hold
//! it), for the few responders that only ever multicast their answers.

use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

const GROUP: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 251);
const PORT: u16 = 5353;

const TYPE_A: u16 = 1;
const TYPE_PTR: u16 = 12;
const TYPE_SRV: u16 = 33;

/// One service instance that answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Service {
    /// The service type asked for, as given (`_elg._tcp.local`).
    pub service: String,
    /// The instance's own name (`Elgato Key Light 1A2B`), may be empty.
    pub instance: String,
    pub ip: IpAddr,
    /// Port from its SRV record, when it came along.
    pub port: Option<u16>,
}

/// The instances of `services` answering within `timeout`, one per
/// service type and address.
pub fn browse(services: &[&str], timeout: Duration) -> Vec<Service> {
    let Ok(unicast) = UdpSocket::bind(("0.0.0.0", 0)) else {
        return Vec::new();
    };
    let group = SocketAddr::from((GROUP, PORT));
    let packet = query(0x5458, services);
    if let Err(e) = unicast.send_to(&packet, group) {
        tracing::debug!("mDNS query: {e}");
    }
    let shared = shared_listener();
    if let Some(s) = &shared {
        // A plain multicast query (id 0): answers go to the group.
        let _ = s.send_to(&query(0, services), group);
    }
    let sockets: Vec<&UdpSocket> = std::iter::once(&unicast).chain(shared.as_ref()).collect();
    let deadline = Instant::now() + timeout;
    let mut found: Vec<Service> = Vec::new();
    let mut buf = [0u8; 9000];
    while Instant::now() < deadline {
        for s in &sockets {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            let _ = s.set_read_timeout(Some(left.min(Duration::from_millis(40))));
            let Ok((n, from)) = s.recv_from(&mut buf) else {
                continue;
            };
            for svc in answers(&buf[..n], from.ip(), services) {
                if !found
                    .iter()
                    .any(|f| f.service == svc.service && f.ip == svc.ip)
                {
                    found.push(svc);
                }
            }
        }
    }
    found
}

/// Port 5353, shared with the system's own mDNS responder, member of the
/// mDNS group (`None` if it cannot be shared).
fn shared_listener() -> Option<UdpSocket> {
    use socket2::{Domain, Protocol, Socket, Type};
    let s = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP)).ok()?;
    s.set_reuse_address(true).ok()?;
    s.bind(&SocketAddr::from((Ipv4Addr::UNSPECIFIED, PORT)).into())
        .ok()?;
    s.join_multicast_v4(&GROUP, &Ipv4Addr::UNSPECIFIED).ok()?;
    let _ = s.set_multicast_loop_v4(false);
    Some(s.into())
}

/// A DNS query with one PTR question per service (unicast answer asked).
pub fn query(id: u16, services: &[&str]) -> Vec<u8> {
    let mut p = Vec::with_capacity(12 + services.len() * 32);
    p.extend(id.to_be_bytes());
    p.extend(0u16.to_be_bytes()); // standard query
    p.extend((services.len() as u16).to_be_bytes());
    p.extend([0u8; 6]); // no answers, authority, additional
    for s in services {
        for label in s.trim_end_matches('.').split('.') {
            let bytes = &label.as_bytes()[..label.len().min(63)];
            p.push(bytes.len() as u8);
            p.extend(bytes);
        }
        p.push(0);
        p.extend(TYPE_PTR.to_be_bytes());
        // Class IN, "unicast response" bit for the multicast copy.
        p.extend((0x8001u16).to_be_bytes());
    }
    p
}

/// The service instances one response describes (`from`: its sender,
/// the address used when no A record came along).
pub fn answers(packet: &[u8], from: IpAddr, services: &[&str]) -> Vec<Service> {
    let Some(records) = parse(packet) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (owner, target) in &records.ptr {
        let Some(service) = services.iter().find(|s| same_name(owner, s)) else {
            continue;
        };
        let suffix_len = service.trim_end_matches('.').len();
        let cut = target.len().saturating_sub(suffix_len);
        let instance = match (target.get(..cut.saturating_sub(1)), target.get(cut..)) {
            (Some(name), Some(suffix)) if cut > 1 && same_name(suffix, service) => name.to_owned(),
            _ => String::new(),
        };
        let srv = records.srv.iter().find(|(o, _, _)| same_name(o, target));
        let ip = srv
            .and_then(|(_, _, host)| records.a.iter().find(|(o, _)| same_name(o, host)))
            .map(|(_, ip)| IpAddr::V4(*ip))
            .unwrap_or(from);
        out.push(Service {
            service: (*service).to_owned(),
            instance,
            ip,
            port: srv.map(|(_, port, _)| *port),
        });
    }
    out
}

fn same_name(a: &str, b: &str) -> bool {
    a.trim_end_matches('.')
        .eq_ignore_ascii_case(b.trim_end_matches('.'))
}

#[derive(Default)]
struct Records {
    /// (owner, target)
    ptr: Vec<(String, String)>,
    /// (owner, port, target)
    srv: Vec<(String, u16, String)>,
    /// (owner, address)
    a: Vec<(String, Ipv4Addr)>,
}

fn u16_at(p: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*p.get(at)?, *p.get(at + 1)?]))
}

/// The records of a response (`None`: not a response, or malformed).
fn parse(p: &[u8]) -> Option<Records> {
    let flags = u16_at(p, 2)?;
    if flags & 0x8000 == 0 {
        return None; // a query
    }
    let questions = u16_at(p, 4)?;
    let records = u16_at(p, 6)? as usize + u16_at(p, 8)? as usize + u16_at(p, 10)? as usize;
    let mut at = 12;
    for _ in 0..questions {
        let (_, next) = name(p, at)?;
        at = next + 4;
    }
    let mut out = Records::default();
    for _ in 0..records {
        let (owner, next) = name(p, at)?;
        let kind = u16_at(p, next)?;
        let len = u16_at(p, next + 8)? as usize;
        let data = next + 10;
        if data + len > p.len() {
            return None;
        }
        match kind {
            TYPE_PTR => out.ptr.push((owner, name(p, data)?.0)),
            TYPE_SRV => {
                let port = u16_at(p, data + 4)?;
                out.srv.push((owner, port, name(p, data + 6)?.0));
            }
            TYPE_A if len == 4 => out.a.push((
                owner,
                Ipv4Addr::new(p[data], p[data + 1], p[data + 2], p[data + 3]),
            )),
            _ => {}
        }
        at = data + len;
    }
    Some(out)
}

/// A (possibly compressed) name at `at`: the name and where the bytes
/// after it start.
fn name(p: &[u8], mut at: usize) -> Option<(String, usize)> {
    let mut labels: Vec<String> = Vec::new();
    let mut end = None;
    let mut jumps = 0;
    loop {
        let len = *p.get(at)? as usize;
        match len {
            0 => {
                end.get_or_insert(at + 1);
                break;
            }
            l if l & 0xC0 == 0xC0 => {
                let pointer = (u16_at(p, at)? & 0x3FFF) as usize;
                end.get_or_insert(at + 2);
                jumps += 1;
                if jumps > 16 || pointer >= p.len() {
                    return None;
                }
                at = pointer;
            }
            l if l < 64 => {
                let bytes = p.get(at + 1..at + 1 + l)?;
                labels.push(String::from_utf8_lossy(bytes).into_owned());
                at += 1 + l;
            }
            _ => return None,
        }
    }
    Some((labels.join("."), end?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode(name: &str) -> Vec<u8> {
        let mut out = Vec::new();
        for l in name.split('.') {
            out.push(l.len() as u8);
            out.extend(l.as_bytes());
        }
        out.push(0);
        out
    }

    fn record(owner: &[u8], kind: u16, data: &[u8]) -> Vec<u8> {
        let mut r = owner.to_vec();
        r.extend(kind.to_be_bytes());
        r.extend(1u16.to_be_bytes());
        r.extend(120u32.to_be_bytes());
        r.extend((data.len() as u16).to_be_bytes());
        r.extend(data);
        r
    }

    /// An Elgato-style answer: PTR, then SRV and A as additional records,
    /// with a compression pointer to the service name.
    fn elgato_answer() -> Vec<u8> {
        let mut p = vec![0x54, 0x58, 0x84, 0x00, 0, 0, 0, 1, 0, 0, 0, 2];
        let service_at = p.len();
        let service = encode("_elg._tcp.local");
        // PTR: owner = service, target = "Key Light Air 1A2B" + pointer.
        let mut target = vec![18];
        target.extend(b"Key Light Air 1A2B");
        target.extend([0xC0, service_at as u8]);
        p.extend(record(&service, TYPE_PTR, &target));
        let instance = {
            let mut i = vec![18];
            i.extend(b"Key Light Air 1A2B");
            i.extend([0xC0, service_at as u8]);
            i
        };
        let host = encode("elgato-key-light-air-1a2b.local");
        let mut srv = vec![0, 0, 0, 0];
        srv.extend(9123u16.to_be_bytes());
        srv.extend(&host);
        p.extend(record(&instance, TYPE_SRV, &srv));
        p.extend(record(&host, TYPE_A, &[192, 168, 1, 77]));
        p
    }

    #[test]
    fn queries_are_well_formed() {
        let q = query(7, &["_wled._tcp.local", "_hue._tcp.local"]);
        assert_eq!(&q[..6], &[0, 7, 0, 0, 0, 2]);
        let mut expected = encode("_wled._tcp.local");
        expected.extend([0, 12, 0x80, 0x01]);
        assert_eq!(&q[12..12 + expected.len()], &expected[..]);
    }

    #[test]
    fn answers_give_name_address_and_port() {
        let found = answers(
            &elgato_answer(),
            IpAddr::V4(Ipv4Addr::new(192, 168, 1, 99)),
            &["_elg._tcp.local", "_hue._tcp.local"],
        );
        assert_eq!(
            found,
            vec![Service {
                service: "_elg._tcp.local".into(),
                instance: "Key Light Air 1A2B".into(),
                ip: IpAddr::V4(Ipv4Addr::new(192, 168, 1, 77)),
                port: Some(9123),
            }]
        );
        // Another service type: nothing.
        assert!(answers(&elgato_answer(), found[0].ip, &["_wled._tcp.local"]).is_empty());
        // Our own query looping back: ignored.
        assert!(
            answers(
                &query(1, &["_elg._tcp.local"]),
                found[0].ip,
                &["_elg._tcp.local"]
            )
            .is_empty()
        );
    }

    #[test]
    fn malformed_packets_are_refused() {
        let mut p = elgato_answer();
        p.truncate(p.len() - 3);
        assert!(parse(&p).is_none());
        // A compression loop.
        let mut l = vec![0, 0, 0x84, 0, 0, 1, 0, 0, 0, 0, 0, 0];
        l.extend([0xC0, 12]);
        assert!(parse(&l).is_none());
        assert!(parse(&[]).is_none());
    }

    #[test]
    fn a_responder_on_the_loopback_is_found() {
        // A fake responder answering the legacy unicast query directly.
        let responder = UdpSocket::bind("127.0.0.1:0").unwrap();
        let addr = responder.local_addr().unwrap();
        let t = std::thread::spawn(move || {
            let mut buf = [0u8; 512];
            let (_, from) = responder.recv_from(&mut buf).unwrap();
            responder.send_to(&elgato_answer(), from).unwrap();
        });
        let s = UdpSocket::bind("127.0.0.1:0").unwrap();
        s.send_to(&query(1, &["_elg._tcp.local"]), addr).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let mut buf = [0u8; 2048];
        let (n, from) = s.recv_from(&mut buf).unwrap();
        t.join().unwrap();
        let found = answers(&buf[..n], from.ip(), &["_elg._tcp.local"]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].port, Some(9123));
    }
}
