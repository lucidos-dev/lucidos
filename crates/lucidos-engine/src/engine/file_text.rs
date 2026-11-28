pub(crate) const BINARY_SNIFF_BYTES: usize = 8 * 1024;

pub fn looks_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(BINARY_SNIFF_BYTES).any(|b| *b == 0)
}

pub(crate) const ELLIPSIS: char = '…';

pub fn cap_chars(s: &str, max_chars: usize) -> String {
    let mut chars = s.chars();
    let mut head: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        head.push(ELLIPSIS);
    }
    head
}
