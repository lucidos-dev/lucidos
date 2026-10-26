//! A reverse lookup sent straight to Tailscale's own resolver, `100.100.100.100`.
//!
//! The system resolver asks it only while "Use Tailscale DNS settings" is on.
//! With that off, the OS lookup finds no name, yet the resolver still answers
//! every tailnet node. So this is the fallback that recovers the name.
//!
//! One PTR question over UDP, built and parsed by hand, because the crate may
//! depend on nothing but `libc`.

use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};
use std::time::{Duration, Instant};

const QUAD100: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::new(100, 100, 100, 100), 53);
const TYPE_PTR: u16 = 12;
const CLASS_IN: u16 = 1;
const HEADER_LEN: usize = 12;
/// Every legal name fits in fewer hops than this, so a pointer loop ends here.
const MAX_LABELS: usize = 128;

/// Blocking. Only ever called from the bounded worker in `magic_dns_name`.
pub(crate) fn reverse_lookup(addr: Ipv4Addr, timeout: Duration) -> Option<String> {
    let deadline = Instant::now() + timeout;
    // Connected, so the kernel drops datagrams from any other sender.
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    socket.connect(QUAD100).ok()?;
    let id = query_id();
    socket.send(&ptr_query(addr, id)).ok()?;
    let mut buf = [0u8; 512];
    loop {
        let left = deadline
            .checked_duration_since(Instant::now())
            .filter(|d| !d.is_zero())?;
        socket.set_read_timeout(Some(left)).ok()?;
        let len = socket.recv(&mut buf).ok()?;
        // A late reply to an earlier query on this port is not ours: keep waiting.
        if buf[..len].starts_with(&id.to_be_bytes()) {
            return ptr_answer(&buf[..len], id);
        }
    }
}

/// Any value that differs between calls. It only pairs a reply with its query.
fn query_id() -> u16 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    (nanos ^ (nanos >> 16)) as u16
}

fn ptr_query(addr: Ipv4Addr, id: u16) -> Vec<u8> {
    let mut q = Vec::with_capacity(48);
    q.extend_from_slice(&id.to_be_bytes());
    q.extend_from_slice(&0x0100u16.to_be_bytes()); // recursion desired
    q.extend_from_slice(&1u16.to_be_bytes()); // one question
    q.extend_from_slice(&[0; 6]); // no answer, authority or additional records
    let [a, b, c, d] = addr.octets();
    for label in [d, c, b, a]
        .map(|o| o.to_string())
        .iter()
        .map(String::as_str)
        .chain(["in-addr", "arpa"])
    {
        q.push(label.len() as u8);
        q.extend_from_slice(label.as_bytes());
    }
    q.push(0);
    q.extend_from_slice(&TYPE_PTR.to_be_bytes());
    q.extend_from_slice(&CLASS_IN.to_be_bytes());
    q
}

/// The first PTR target in a reply to query `id`, without the trailing dot.
fn ptr_answer(msg: &[u8], id: u16) -> Option<String> {
    let header = msg.get(..HEADER_LEN)?;
    let is_reply = header[2] & 0x80 != 0;
    let rcode = header[3] & 0x0f;
    if u16::from_be_bytes([header[0], header[1]]) != id || !is_reply || rcode != 0 {
        return None;
    }
    let questions = u16::from_be_bytes([header[4], header[5]]);
    let answers = u16::from_be_bytes([header[6], header[7]]);
    let mut pos = HEADER_LEN;
    for _ in 0..questions {
        pos = read_name(msg, pos)?.1 + 4;
    }
    for _ in 0..answers {
        pos = read_name(msg, pos)?.1;
        let fixed = msg.get(pos..pos + 10)?;
        let rtype = u16::from_be_bytes([fixed[0], fixed[1]]);
        let rdlen = u16::from_be_bytes([fixed[8], fixed[9]]) as usize;
        pos += 10;
        if rtype == TYPE_PTR {
            let name = read_name(msg, pos)?.0;
            return (!name.is_empty()).then_some(name);
        }
        pos += rdlen;
    }
    None
}

/// The dotted name at `pos`, following compression pointers, and the offset
/// just past the name where it was first met.
fn read_name(msg: &[u8], mut pos: usize) -> Option<(String, usize)> {
    let mut labels: Vec<&str> = Vec::new();
    let mut end = None;
    for _ in 0..MAX_LABELS {
        let len = *msg.get(pos)? as usize;
        match len & 0xc0 {
            0x00 if len == 0 => {
                return Some((labels.join("."), end.unwrap_or(pos + 1)));
            }
            0x00 => {
                let label = std::str::from_utf8(msg.get(pos + 1..pos + 1 + len)?).ok()?;
                labels.push(label);
                pos += 1 + len;
            }
            0xc0 => {
                let target = ((len & 0x3f) << 8) | *msg.get(pos + 1)? as usize;
                end.get_or_insert(pos + 2);
                pos = target;
            }
            _ => return None,
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: u16 = 0x2a2a;

    /// A reply as a resolver builds it: the question echoed, then one answer
    /// whose owner name points back at the question.
    fn reply_to(query: &[u8], answer_type: u16, target: &str) -> Vec<u8> {
        let mut r = query.to_vec();
        r[2] |= 0x80; // this is a reply
        r[7] = 1; // one answer
        r.extend_from_slice(&[0xc0, HEADER_LEN as u8]);
        r.extend_from_slice(&answer_type.to_be_bytes());
        r.extend_from_slice(&CLASS_IN.to_be_bytes());
        r.extend_from_slice(&600u32.to_be_bytes());
        let mut rdata = Vec::new();
        for label in target.split('.') {
            rdata.push(label.len() as u8);
            rdata.extend_from_slice(label.as_bytes());
        }
        rdata.push(0);
        r.extend_from_slice(&(rdata.len() as u16).to_be_bytes());
        r.extend_from_slice(&rdata);
        r
    }

    fn query() -> Vec<u8> {
        ptr_query(Ipv4Addr::new(100, 64, 0, 7), ID)
    }

    #[test]
    fn query_asks_for_the_reversed_address_under_in_addr_arpa() {
        let q = query();
        let (name, end) = read_name(&q, HEADER_LEN).unwrap();
        assert_eq!(name, "7.0.64.100.in-addr.arpa");
        assert_eq!(&q[end..], &[0, 12, 0, 1]);
    }

    #[test]
    fn reads_the_ptr_target_without_a_trailing_dot() {
        let r = reply_to(&query(), TYPE_PTR, "mymac.tailnet-name.ts.net");
        assert_eq!(
            ptr_answer(&r, ID).as_deref(),
            Some("mymac.tailnet-name.ts.net")
        );
    }

    #[test]
    fn rejects_a_reply_to_another_query() {
        let r = reply_to(&query(), TYPE_PTR, "mymac.tailnet-name.ts.net");
        assert_eq!(ptr_answer(&r, ID + 1), None);
    }

    #[test]
    fn rejects_an_error_reply_and_a_non_ptr_answer() {
        let mut nxdomain = reply_to(&query(), TYPE_PTR, "mymac.tailnet-name.ts.net");
        nxdomain[3] |= 3;
        assert_eq!(ptr_answer(&nxdomain, ID), None);
        let cname = reply_to(&query(), 5, "mymac.tailnet-name.ts.net");
        assert_eq!(ptr_answer(&cname, ID), None);
    }

    #[test]
    fn survives_truncation_and_a_pointer_loop() {
        let r = reply_to(&query(), TYPE_PTR, "mymac.tailnet-name.ts.net");
        for len in 0..r.len() {
            assert_eq!(ptr_answer(&r[..len], ID), None, "truncated at {len}");
        }
        let looped = [0xc0, 0x00];
        assert_eq!(read_name(&looped, 0), None);
    }
}
