//! Binance-style HMAC-SHA256 signer.
//!
//! Wire contract (matches `proxy_wasm_signer::SignInput` /
//! `proxy_wasm_signer::SignOutput`):
//!  - exports `alloc(len: i32) -> i32` for the host to write `SignInput`
//!    JSON into linear memory before calling `sign`.
//!  - exports `sign(in_ptr: i32, in_len: i32) -> i64` — return packs
//!    `(out_ptr << 32) | out_len`.
//!
//! Algorithm:
//!  1. Extract the existing query string from `SignInput.url`.
//!  2. Build canonical = `<existing>&timestamp=<ms>` (or just
//!     `timestamp=<ms>` if no existing query).
//!  3. HMAC-SHA256(canonical, api_secret) via host import.
//!  4. Hex-encode the 32-byte digest via host import.
//!  5. Emit `SignOutput { add_query: [["timestamp", ms], ["signature", hex]] }`.
//!
//! Manifest declares `secret_handles = ["api_secret"]`, so the host's
//! per-call secret table holds it at index 0; the signer never sees the
//! raw secret bytes.
#![no_std]

use core::panic::PanicInfo;

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    core::arch::wasm32::unreachable()
}

// ---- Bump allocator over a static heap --------------------------------

const HEAP_SIZE: usize = 256 * 1024;
static mut HEAP: [u8; HEAP_SIZE] = [0; HEAP_SIZE];
static mut HEAP_OFFSET: usize = 0;

#[no_mangle]
pub extern "C" fn alloc(size: i32) -> i32 {
    if size < 0 {
        return 0;
    }
    let need = size as usize;
    unsafe {
        let offset = core::ptr::addr_of!(HEAP_OFFSET).read();
        if offset + need > HEAP_SIZE {
            return 0;
        }
        let ptr = core::ptr::addr_of!(HEAP) as usize + offset;
        core::ptr::addr_of_mut!(HEAP_OFFSET).write(offset + need);
        ptr as i32
    }
}

// ---- Host imports (provided by lucidos-engine proxy_wasm_host) --------

extern "C" {
    fn current_time_ns() -> i64;
    fn hmac_sha256(secret_id: i32, data_ptr: i32, data_len: i32, out_ptr: i32) -> i32;
    fn hex_encode(in_ptr: i32, in_len: i32, out_ptr: i32, out_cap: i32) -> i32;
}

const SECRET_ID_API_SECRET: i32 = 0;
const MAX_CANONICAL_BYTES: usize = 8192;
const MAX_OUTPUT_BYTES: usize = 8192;

#[no_mangle]
pub extern "C" fn sign(in_ptr: i32, in_len: i32) -> i64 {
    if in_len <= 0 {
        return 0;
    }
    let input = unsafe { core::slice::from_raw_parts(in_ptr as *const u8, in_len as usize) };

    let existing_query = find_url_query(input).unwrap_or(&[]);
    let ts_ms = (unsafe { current_time_ns() } / 1_000_000) as u64;

    // Build canonical string into a stack buffer: <existing>&timestamp=<ms>
    let mut canonical = [0u8; MAX_CANONICAL_BYTES];
    let mut canonical_len = 0usize;
    if !existing_query.is_empty() {
        if existing_query.len() + 1 > canonical.len() {
            return 0;
        }
        canonical[..existing_query.len()].copy_from_slice(existing_query);
        canonical_len = existing_query.len();
        canonical[canonical_len] = b'&';
        canonical_len += 1;
    }
    let ts_prefix = b"timestamp=";
    if canonical_len + ts_prefix.len() > canonical.len() {
        return 0;
    }
    canonical[canonical_len..canonical_len + ts_prefix.len()].copy_from_slice(ts_prefix);
    canonical_len += ts_prefix.len();
    let written = match write_u64_into(&mut canonical[canonical_len..], ts_ms) {
        Some(n) => n,
        None => return 0,
    };
    canonical_len += written;

    // Copy canonical bytes into module memory at an alloc'd ptr — host
    // import reads from there. (We can't pass &canonical[0] directly: WASM
    // sees that as a host-side reference, not a guest pointer; alloc is
    // the contract.)
    let canonical_ptr = alloc(canonical_len as i32);
    if canonical_ptr == 0 {
        return 0;
    }
    unsafe {
        core::ptr::copy_nonoverlapping(
            canonical.as_ptr(),
            canonical_ptr as *mut u8,
            canonical_len,
        );
    }

    // HMAC-SHA256 → 32-byte digest in module memory.
    let digest_ptr = alloc(32);
    if digest_ptr == 0 {
        return 0;
    }
    let dig_written = unsafe {
        hmac_sha256(
            SECRET_ID_API_SECRET,
            canonical_ptr,
            canonical_len as i32,
            digest_ptr,
        )
    };
    if dig_written != 32 {
        return 0;
    }

    // Hex-encode the digest → 64 ASCII bytes.
    let hex_ptr = alloc(64);
    if hex_ptr == 0 {
        return 0;
    }
    let hex_written = unsafe { hex_encode(digest_ptr, 32, hex_ptr, 64) };
    if hex_written != 64 {
        return 0;
    }

    // Build SignOutput JSON: {"add_query":[["timestamp","<ms>"],["signature","<hex>"]]}
    let mut out = [0u8; MAX_OUTPUT_BYTES];
    let mut out_len = 0usize;
    let prefix = br#"{"add_query":[["timestamp",""#;
    if !push_slice(&mut out, &mut out_len, prefix) {
        return 0;
    }
    let ts_written = match write_u64_into(&mut out[out_len..], ts_ms) {
        Some(n) => n,
        None => return 0,
    };
    out_len += ts_written;
    let mid = br#""],["signature",""#;
    if !push_slice(&mut out, &mut out_len, mid) {
        return 0;
    }
    let hex_bytes = unsafe { core::slice::from_raw_parts(hex_ptr as *const u8, 64) };
    if !push_slice(&mut out, &mut out_len, hex_bytes) {
        return 0;
    }
    let suffix = br#""]]}"#;
    if !push_slice(&mut out, &mut out_len, suffix) {
        return 0;
    }

    // Copy out into a final alloc'd region whose pointer we return.
    let out_ptr = alloc(out_len as i32);
    if out_ptr == 0 {
        return 0;
    }
    unsafe {
        core::ptr::copy_nonoverlapping(out.as_ptr(), out_ptr as *mut u8, out_len);
    }
    ((out_ptr as i64) << 32) | (out_len as i64)
}

/// Find the value of `"url":"..."` in `input` and return everything after
/// the first `?` in that URL. None if either lookup fails.
///
/// Crude byte scan — sufficient for `SignInput`'s shape (the host
/// JSON-serializes a struct, so the field appears verbatim and the URL
/// value never embeds `"` characters).
fn find_url_query(input: &[u8]) -> Option<&[u8]> {
    let key = br#""url":""#;
    let i = find_subslice(input, key)?;
    let mut j = i + key.len();
    let url_start = j;
    while j < input.len() && input[j] != b'"' {
        j += 1;
    }
    let url = &input[url_start..j];
    let mut q = 0;
    while q < url.len() {
        if url[q] == b'?' {
            return Some(&url[q + 1..]);
        }
        q += 1;
    }
    None
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.len() > haystack.len() {
        return None;
    }
    let mut i = 0;
    while i + needle.len() <= haystack.len() {
        let mut j = 0;
        while j < needle.len() && haystack[i + j] == needle[j] {
            j += 1;
        }
        if j == needle.len() {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn write_u64_into(buf: &mut [u8], mut n: u64) -> Option<usize> {
    if n == 0 {
        if buf.is_empty() {
            return None;
        }
        buf[0] = b'0';
        return Some(1);
    }
    let mut tmp = [0u8; 20];
    let mut len = 0;
    while n > 0 {
        if len >= tmp.len() {
            return None;
        }
        tmp[len] = b'0' + (n % 10) as u8;
        n /= 10;
        len += 1;
    }
    if buf.len() < len {
        return None;
    }
    for i in 0..len {
        buf[i] = tmp[len - 1 - i];
    }
    Some(len)
}

fn push_slice(buf: &mut [u8], cursor: &mut usize, src: &[u8]) -> bool {
    if *cursor + src.len() > buf.len() {
        return false;
    }
    buf[*cursor..*cursor + src.len()].copy_from_slice(src);
    *cursor += src.len();
    true
}

// memory export — `cdylib` builds emit one by default, and the WasmSignerLayer
// loader looks it up by the standard name `memory`.
