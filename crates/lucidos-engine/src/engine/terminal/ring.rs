//! The bounded output ring of one thread terminal, addressed by absolute byte
//! offset. It lives in engine memory only (ADR 0356 decision 4).

use std::collections::VecDeque;

pub struct Ring {
    bytes: VecDeque<u8>,
    start: u64,
    capacity: usize,
}

impl Ring {
    pub fn new(capacity: usize) -> Ring {
        Ring {
            bytes: VecDeque::with_capacity(capacity),
            start: 0,
            capacity,
        }
    }

    /// The absolute offset one past the last byte written.
    pub fn end(&self) -> u64 {
        self.start + self.bytes.len() as u64
    }

    pub fn push(&mut self, data: &[u8]) {
        let trimmed = data.len().saturating_sub(self.capacity);
        let data = &data[trimmed..];
        let overflow = (self.bytes.len() + data.len()).saturating_sub(self.capacity);
        self.bytes.drain(..overflow);
        self.start += (overflow + trimmed) as u64;
        self.bytes.extend(data);
    }

    /// The bytes from `offset` to the end, or `None` when `offset` lies
    /// outside the ring and only a reset can bring a client current.
    pub fn since(&self, offset: u64) -> Option<Vec<u8>> {
        if offset < self.start || offset > self.end() {
            return None;
        }
        let skip = (offset - self.start) as usize;
        Some(self.bytes.iter().skip(skip).copied().collect())
    }

    /// The whole ring and the offset of its first byte.
    pub fn snapshot(&self) -> (u64, Vec<u8>) {
        (self.start, self.bytes.iter().copied().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_resume_inside_the_ring_gets_exactly_the_missing_bytes() {
        let mut ring = Ring::new(8);
        ring.push(b"abcd");
        ring.push(b"ef");
        assert_eq!(ring.since(4).as_deref(), Some(&b"ef"[..]));
        assert_eq!(ring.since(6).as_deref(), Some(&b""[..]));
        assert_eq!(ring.since(0).as_deref(), Some(&b"abcdef"[..]));
    }

    #[test]
    fn wraparound_moves_the_start_and_refuses_offsets_before_it() {
        let mut ring = Ring::new(4);
        ring.push(b"abcdef");
        assert_eq!(ring.end(), 6);
        assert_eq!(ring.snapshot(), (2, b"cdef".to_vec()));
        assert_eq!(ring.since(1), None);
        assert_eq!(ring.since(3).as_deref(), Some(&b"def"[..]));
        ring.push(b"gh");
        assert_eq!(ring.snapshot(), (4, b"efgh".to_vec()));
    }

    #[test]
    fn an_offset_past_the_end_needs_a_reset() {
        let mut ring = Ring::new(4);
        ring.push(b"ab");
        assert_eq!(ring.since(3), None);
    }
}
