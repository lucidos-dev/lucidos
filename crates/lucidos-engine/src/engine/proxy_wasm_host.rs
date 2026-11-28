//! INVARIANT: Host imports exposed to WASM signer modules: time, crypto primitives,
//! opaque secret handle resolution, optional capability-gated escape hatches.
//!
//! Design points:
//!  - Crypto primitives operate on linear-memory pointers — modules write
//!    input into their own memory at `data_ptr`, host reads it, writes the
//!    result back at `out_ptr`. Same shape as classic WASI imports.
//!  - Secrets are passed by **opaque handle** (an index into the per-call
//!    `HostState::secrets` table). The module never sees secret bytes.
//!  - Sync host imports (despite `Config::async_support(true)`): every
//!    primitive is CPU-bound and finishes in microseconds. wasmtime allows
//!    sync imports under async support as long as they don't block.

use crate::engine::proxy_wasm_signer::SignerLimits;
use base64::Engine;
use hmac::{Hmac, Mac};
use sha1::Sha1;
use sha2::{Digest, Sha256, Sha512};
use wasmtime::{Caller, Linker, Memory};

/// INVARIANT: Per-call state seen by host imports. The pipeline allocates one of these
/// per signer invocation and threads it through wasmtime's `Store` so the
/// secret table is fresh each time and never leaks across callers.
pub struct HostState {
    pub secrets: Vec<Vec<u8>>,
    pub module_name: String,
    /// INVARIANT: Sensitive substrings to scrub from anything the module emits through the
    /// `log` host import: the resolved secret material (in its encodings) plus
    /// every auth-header value an earlier layer published.
    ///
    /// This is the second of two defences, and the narrower one. A signer with
    /// no `read_prior_headers` grant is not handed those values at all (see
    /// `proxy_wasm_signer::prepare_prior_outputs`). This list closes the log for
    /// a signer that IS granted them.
    pub log_redactions: Vec<String>,
    pub limits: SignerLimits,
    pub log_budget: LogBudget,
}

pub const LOG_LINE_MAX_BYTES: usize = 4 * 1024;
pub const LOG_LINES_PER_INVOCATION: u32 = 32;
pub const LOG_BYTES_PER_INVOCATION: usize = 16 * 1024;
pub const LOG_BUDGET_EXHAUSTED: &str =
    "log budget exhausted, dropping further log lines from this invocation";

/// INVARIANT: Per-invocation allowance for the `log` host import.
///
/// Every printed line lands in `engine.log`, and nothing rotates that file.
/// Without this, a signer logging in a loop fills the disk, and a full disk
/// takes Postgres down with it.
#[derive(Debug, Default)]
pub struct LogBudget {
    lines: u32,
    bytes_read: usize,
    exhausted: bool,
}

impl LogBudget {
    pub fn lines_printed(&self) -> u32 {
        self.lines
    }

    pub fn bytes_read(&self) -> usize {
        self.bytes_read
    }

    pub fn is_exhausted(&self) -> bool {
        self.exhausted
    }

    pub fn next_line<'m>(
        &mut self,
        requested: usize,
        redactions: &[String],
        read: impl FnOnce(usize) -> Option<&'m [u8]>,
    ) -> Option<String> {
        if self.exhausted {
            return None;
        }
        let remaining = LOG_BYTES_PER_INVOCATION - self.bytes_read;
        if self.lines == LOG_LINES_PER_INVOCATION || remaining == 0 {
            self.exhausted = true;
            return Some(LOG_BUDGET_EXHAUSTED.to_string());
        }
        let take = requested.min(LOG_LINE_MAX_BYTES).min(remaining);
        let bytes = read(take)?;
        self.lines += 1;
        self.bytes_read += take;
        Some(format_log_line(bytes, requested - take, redactions))
    }
}

fn format_log_line(bytes: &[u8], dropped: usize, redactions: &[String]) -> String {
    let text = String::from_utf8_lossy(bytes);
    if dropped == 0 {
        return crate::core::redact_secret_values(&text, redactions);
    }
    // INVARIANT: A cut inside a character leaves U+FFFD, which would hide a secret's
    // start from `without_partial_secret_tail`.
    let kept = without_partial_secret_tail(text.trim_end_matches('\u{FFFD}'), redactions);
    let redacted = crate::core::redact_secret_values(kept, redactions);
    format!("{redacted} [truncated, {dropped} more bytes]")
}

fn without_partial_secret_tail<'t>(text: &'t str, redactions: &[String]) -> &'t str {
    let cut = redactions
        .iter()
        .filter_map(|secret| {
            (1..secret.len())
                .rev()
                .filter(|&n| secret.is_char_boundary(n))
                .find(|&n| text.ends_with(&secret[..n]))
                .map(|n| text.len() - n)
        })
        .min()
        .unwrap_or(text.len());
    &text[..cut]
}

pub fn sha1_digest(data: &[u8]) -> [u8; 20] {
    let mut h = Sha1::new();
    h.update(data);
    h.finalize().into()
}

pub fn sha256_digest(data: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(data);
    h.finalize().into()
}

pub fn sha512_digest(data: &[u8]) -> [u8; 64] {
    let mut h = Sha512::new();
    h.update(data);
    let out = h.finalize();
    let mut buf = [0u8; 64];
    buf.copy_from_slice(&out);
    buf
}

pub fn hmac_sha1_digest(secret: &[u8], data: &[u8]) -> [u8; 20] {
    let mut mac = Hmac::<Sha1>::new_from_slice(secret).expect("HMAC accepts any key length");
    mac.update(data);
    let out = mac.finalize().into_bytes();
    let mut buf = [0u8; 20];
    buf.copy_from_slice(&out);
    buf
}

pub fn hmac_sha256_digest(secret: &[u8], data: &[u8]) -> [u8; 32] {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret).expect("HMAC accepts any key length");
    mac.update(data);
    let out = mac.finalize().into_bytes();
    let mut buf = [0u8; 32];
    buf.copy_from_slice(&out);
    buf
}

pub fn hmac_sha512_digest(secret: &[u8], data: &[u8]) -> [u8; 64] {
    let mut mac = Hmac::<Sha512>::new_from_slice(secret).expect("HMAC accepts any key length");
    mac.update(data);
    let out = mac.finalize().into_bytes();
    let mut buf = [0u8; 64];
    buf.copy_from_slice(&out);
    buf
}

pub fn hex_encode_into(input: &[u8], out: &mut [u8]) -> Option<usize> {
    let needed = input.len() * 2;
    if out.len() < needed {
        return None;
    }
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for (i, byte) in input.iter().enumerate() {
        out[i * 2] = HEX[(byte >> 4) as usize];
        out[i * 2 + 1] = HEX[(byte & 0x0f) as usize];
    }
    Some(needed)
}

pub fn base64_encode_into(input: &[u8], out: &mut [u8]) -> Option<usize> {
    let needed = base64::encoded_len(input.len(), true).unwrap_or(0);
    if out.len() < needed {
        return None;
    }
    let written = base64::engine::general_purpose::STANDARD
        .encode_slice(input, out)
        .ok()?;
    Some(written)
}

fn lookup_memory(caller: &mut Caller<'_, HostState>) -> Result<Memory, wasmtime::Error> {
    caller
        .get_export("memory")
        .and_then(|e| e.into_memory())
        .ok_or_else(|| wasmtime::Error::msg("module did not export `memory`"))
}

/// INVARIANT: True when `[ptr, ptr+len)` lies inside the module's own linear memory.
///
/// Every host import below takes a length straight off the WASM stack, so it is
/// whatever the module put there (up to 2 GiB). Allocating first and letting the
/// subsequent `Memory::read` / `Memory::write` reject afterwards means a buggy or
/// hostile signer can OOM-abort the whole engine from inside the sandbox, which
/// is precisely what the sandbox exists to prevent. Checking first rejects
/// nothing a working module does: its buffers are in its own memory by
/// construction.
fn range_in_memory(mem: &Memory, caller: &Caller<'_, HostState>, ptr: u32, len: u32) -> bool {
    (ptr as usize)
        .checked_add(len as usize)
        .is_some_and(|end| end <= mem.data_size(caller))
}

fn read_bytes(
    mem: &Memory,
    caller: &mut Caller<'_, HostState>,
    ptr: u32,
    len: u32,
) -> Result<Vec<u8>, wasmtime::Error> {
    if !range_in_memory(mem, caller, ptr, len) {
        return Err(wasmtime::Error::msg(format!(
            "memory read at {ptr}/{len} is out of bounds"
        )));
    }
    let mut buf = vec![0u8; len as usize];
    mem.read(caller, ptr as usize, &mut buf)
        .map_err(|e| wasmtime::Error::msg(format!("memory read at {ptr}/{len}: {e}")))?;
    Ok(buf)
}

fn write_bytes(
    mem: &Memory,
    caller: &mut Caller<'_, HostState>,
    ptr: u32,
    bytes: &[u8],
) -> Result<(), wasmtime::Error> {
    mem.write(caller, ptr as usize, bytes)
        .map_err(|e| wasmtime::Error::msg(format!("memory write at {ptr}: {e}")))
}

pub fn register_host_imports(linker: &mut Linker<HostState>) -> Result<(), wasmtime::Error> {
    linker.func_wrap(
        "env",
        "current_time_ns",
        |_caller: Caller<'_, HostState>| -> i64 {
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        },
    )?;
    linker.func_wrap(
        "env",
        "current_time_secs",
        |_caller: Caller<'_, HostState>| -> i64 { chrono::Utc::now().timestamp() },
    )?;

    linker.func_wrap(
        "env",
        "random_bytes",
        |mut caller: Caller<'_, HostState>, out_ptr: i32, out_len: i32| -> i32 {
            use rand::RngCore;
            if out_len <= 0 {
                return -1;
            }
            let mem = match lookup_memory(&mut caller) {
                Ok(m) => m,
                Err(_) => return -1,
            };
            // INVARIANT: Bound the fill BEFORE allocating (see `range_in_memory`): the
            // destination has to fit in the module's own memory, so a 2 GiB
            // `out_len` is a bad call, not a buffer to zero and randomize.
            if !range_in_memory(&mem, &caller, out_ptr as u32, out_len as u32) {
                return -1;
            }
            let mut buf = vec![0u8; out_len as usize];
            rand::thread_rng().fill_bytes(&mut buf);
            match write_bytes(&mem, &mut caller, out_ptr as u32, &buf) {
                Ok(()) => out_len,
                Err(_) => -1,
            }
        },
    )?;

    linker.func_wrap(
        "env",
        "sha1",
        |mut caller: Caller<'_, HostState>, data_ptr: i32, data_len: i32, out_ptr: i32| -> i32 {
            run_hash(&mut caller, data_ptr, data_len, out_ptr, |bytes| {
                sha1_digest(bytes).to_vec()
            })
        },
    )?;
    linker.func_wrap(
        "env",
        "sha256",
        |mut caller: Caller<'_, HostState>, data_ptr: i32, data_len: i32, out_ptr: i32| -> i32 {
            run_hash(&mut caller, data_ptr, data_len, out_ptr, |bytes| {
                sha256_digest(bytes).to_vec()
            })
        },
    )?;
    linker.func_wrap(
        "env",
        "sha512",
        |mut caller: Caller<'_, HostState>, data_ptr: i32, data_len: i32, out_ptr: i32| -> i32 {
            run_hash(&mut caller, data_ptr, data_len, out_ptr, |bytes| {
                sha512_digest(bytes).to_vec()
            })
        },
    )?;

    linker.func_wrap(
        "env",
        "hmac_sha1",
        |mut caller: Caller<'_, HostState>,
         secret_id: i32,
         data_ptr: i32,
         data_len: i32,
         out_ptr: i32|
         -> i32 {
            run_hmac(
                &mut caller,
                secret_id,
                data_ptr,
                data_len,
                out_ptr,
                |s, d| hmac_sha1_digest(s, d).to_vec(),
            )
        },
    )?;
    linker.func_wrap(
        "env",
        "hmac_sha256",
        |mut caller: Caller<'_, HostState>,
         secret_id: i32,
         data_ptr: i32,
         data_len: i32,
         out_ptr: i32|
         -> i32 {
            run_hmac(
                &mut caller,
                secret_id,
                data_ptr,
                data_len,
                out_ptr,
                |s, d| hmac_sha256_digest(s, d).to_vec(),
            )
        },
    )?;
    linker.func_wrap(
        "env",
        "hmac_sha512",
        |mut caller: Caller<'_, HostState>,
         secret_id: i32,
         data_ptr: i32,
         data_len: i32,
         out_ptr: i32|
         -> i32 {
            run_hmac(
                &mut caller,
                secret_id,
                data_ptr,
                data_len,
                out_ptr,
                |s, d| hmac_sha512_digest(s, d).to_vec(),
            )
        },
    )?;

    linker.func_wrap(
        "env",
        "hex_encode",
        |mut caller: Caller<'_, HostState>,
         in_ptr: i32,
         in_len: i32,
         out_ptr: i32,
         out_cap: i32|
         -> i32 {
            if in_len < 0 || out_cap < 0 {
                return -1;
            }
            let mem = match lookup_memory(&mut caller) {
                Ok(m) => m,
                Err(_) => return -1,
            };
            let input = match read_bytes(&mem, &mut caller, in_ptr as u32, in_len as u32) {
                Ok(b) => b,
                Err(_) => return -1,
            };
            if !range_in_memory(&mem, &caller, out_ptr as u32, out_cap as u32) {
                return -1;
            }
            let mut buf = vec![0u8; out_cap as usize];
            let written = match hex_encode_into(&input, &mut buf) {
                Some(n) => n,
                None => return -1,
            };
            match write_bytes(&mem, &mut caller, out_ptr as u32, &buf[..written]) {
                Ok(()) => written as i32,
                Err(_) => -1,
            }
        },
    )?;

    linker.func_wrap(
        "env",
        "base64_encode",
        |mut caller: Caller<'_, HostState>,
         in_ptr: i32,
         in_len: i32,
         out_ptr: i32,
         out_cap: i32|
         -> i32 {
            if in_len < 0 || out_cap < 0 {
                return -1;
            }
            let mem = match lookup_memory(&mut caller) {
                Ok(m) => m,
                Err(_) => return -1,
            };
            let input = match read_bytes(&mem, &mut caller, in_ptr as u32, in_len as u32) {
                Ok(b) => b,
                Err(_) => return -1,
            };
            if !range_in_memory(&mem, &caller, out_ptr as u32, out_cap as u32) {
                return -1;
            }
            let mut buf = vec![0u8; out_cap as usize];
            let written = match base64_encode_into(&input, &mut buf) {
                Some(n) => n,
                None => return -1,
            };
            match write_bytes(&mem, &mut caller, out_ptr as u32, &buf[..written]) {
                Ok(()) => written as i32,
                Err(_) => -1,
            }
        },
    )?;

    linker.func_wrap(
        "env",
        "log",
        |mut caller: Caller<'_, HostState>, ptr: i32, len: i32| {
            if len <= 0 {
                return;
            }
            let mem = match lookup_memory(&mut caller) {
                Ok(m) => m,
                Err(_) => return,
            };
            let (memory, state) = mem.data_and_store_mut(&mut caller);
            // INVARIANT: `next_line` scrubs secret material before it reaches the log.
            // `log` is deliberately ungated. An ungranted signer is never
            // handed an upstream auth value, and a granted one cannot print it
            // past this line. Gating the import would only cost a signer
            // author the one channel they have for debugging.
            let line = state
                .log_budget
                .next_line(len as usize, &state.log_redactions, |n| {
                    memory.get(ptr as u32 as usize..)?.get(..n)
                });
            if let Some(line) = line {
                crate::log_legacy!("[wasm-signer:{}] {}", state.module_name, line);
            }
        },
    )?;

    Ok(())
}

fn run_hash<F>(
    caller: &mut Caller<'_, HostState>,
    data_ptr: i32,
    data_len: i32,
    out_ptr: i32,
    f: F,
) -> i32
where
    F: FnOnce(&[u8]) -> Vec<u8>,
{
    if data_len < 0 {
        return -1;
    }
    let mem = match lookup_memory(caller) {
        Ok(m) => m,
        Err(_) => return -1,
    };
    let input = match read_bytes(&mem, caller, data_ptr as u32, data_len as u32) {
        Ok(b) => b,
        Err(_) => return -1,
    };
    let digest = f(&input);
    match write_bytes(&mem, caller, out_ptr as u32, &digest) {
        Ok(()) => digest.len() as i32,
        Err(_) => -1,
    }
}

fn run_hmac<F>(
    caller: &mut Caller<'_, HostState>,
    secret_id: i32,
    data_ptr: i32,
    data_len: i32,
    out_ptr: i32,
    f: F,
) -> i32
where
    F: FnOnce(&[u8], &[u8]) -> Vec<u8>,
{
    if data_len < 0 || secret_id < 0 {
        return -1;
    }
    let secret = match caller.data().secrets.get(secret_id as usize).cloned() {
        Some(s) => s,
        None => return -1,
    };
    let mem = match lookup_memory(caller) {
        Ok(m) => m,
        Err(_) => return -1,
    };
    let input = match read_bytes(&mem, caller, data_ptr as u32, data_len as u32) {
        Ok(b) => b,
        Err(_) => return -1,
    };
    let digest = f(&secret, &input);
    match write_bytes(&mem, caller, out_ptr as u32, &digest) {
        Ok(()) => digest.len() as i32,
        Err(_) => -1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::hex::hex_lower;

    #[test]
    fn sha1_known_vector() {
        let h = sha1_digest(b"abc");
        let hex = hex_lower(&h);
        assert_eq!(hex, "a9993e364706816aba3e25717850c26c9cd0d89d");
    }

    #[test]
    fn sha256_known_vector() {
        let h = sha256_digest(b"abc");
        let hex = hex_lower(&h);
        assert_eq!(
            hex,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn sha512_known_vector() {
        let h = sha512_digest(b"abc");
        let hex = hex_lower(&h);
        assert_eq!(
            hex,
            "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a\
             2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f"
        );
    }

    #[test]
    fn hmac_sha256_rfc4231_test_case_1() {
        let key = [0x0b; 20];
        let mac = hmac_sha256_digest(&key, b"Hi There");
        let hex = hex_lower(&mac);
        assert_eq!(
            hex,
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
    }

    #[test]
    fn hmac_sha512_rfc4231_test_case_1() {
        let key = [0x0b; 20];
        let mac = hmac_sha512_digest(&key, b"Hi There");
        let hex = hex_lower(&mac);
        assert_eq!(
            hex,
            "87aa7cdea5ef619d4ff0b4241a1d6cb02379f4e2ce4ec2787ad0b30545e17cde\
             daa833b7d6b8a702038b274eaea3f4e4be9d914eeb61f1702e696c203a126854"
        );
    }

    #[test]
    fn hmac_sha1_rfc2202_test_case_1() {
        let key = [0x0b; 20];
        let mac = hmac_sha1_digest(&key, b"Hi There");
        let hex = hex_lower(&mac);
        assert_eq!(hex, "b617318655057264e28bc0b6fb378c8ef146be00");
    }

    #[test]
    fn hex_encode_known_input() {
        let mut out = [0u8; 8];
        let written = hex_encode_into(b"\x01\x23\xab\xcd", &mut out).unwrap();
        assert_eq!(written, 8);
        assert_eq!(&out[..written], b"0123abcd");
    }

    #[test]
    fn hex_encode_returns_none_when_capacity_insufficient() {
        let mut out = [0u8; 3];
        assert!(hex_encode_into(b"\x01\x23", &mut out).is_none());
    }

    #[test]
    fn base64_encode_known_input() {
        let mut out = [0u8; 64];
        let written = base64_encode_into(b"hello", &mut out).unwrap();
        assert_eq!(&out[..written], b"aGVsbG8=");
    }

    #[test]
    fn base64_encode_returns_none_when_capacity_insufficient() {
        let mut out = [0u8; 4];
        assert!(base64_encode_into(b"hello", &mut out).is_none());
    }

    const TRUNCATION_MARKER_ALLOWANCE: usize = 64;

    fn log_lines(budget: &mut LogBudget, memory: &[u8], calls: usize, len: usize) -> Vec<String> {
        (0..calls)
            .filter_map(|_| {
                budget.next_line(len, &[], |n| {
                    assert!(n <= LOG_LINE_MAX_BYTES, "host asked for {n} bytes");
                    memory.get(..n)
                })
            })
            .collect()
    }

    #[test]
    fn a_tight_loop_of_huge_lines_stays_inside_the_log_budget() {
        let memory = vec![b'A'; 16 * 1024 * 1024];
        let mut budget = LogBudget::default();
        let lines = log_lines(&mut budget, &memory, 10_000, memory.len());

        let exhausted = lines.iter().filter(|l| *l == LOG_BUDGET_EXHAUSTED).count();
        assert_eq!(exhausted, 1);
        assert_eq!(lines.last().map(String::as_str), Some(LOG_BUDGET_EXHAUSTED));
        for line in &lines {
            assert!(line.len() <= LOG_LINE_MAX_BYTES + TRUNCATION_MARKER_ALLOWANCE);
        }
        let printed: usize = lines.iter().map(String::len).sum();
        assert!(printed <= LOG_BYTES_PER_INVOCATION + lines.len() * TRUNCATION_MARKER_ALLOWANCE);
        assert!(budget.bytes_read() <= LOG_BYTES_PER_INVOCATION);
        assert!(lines[0].ends_with(&format!(
            " [truncated, {} more bytes]",
            memory.len() - LOG_LINE_MAX_BYTES
        )));
    }

    #[test]
    fn a_tight_loop_of_short_lines_stops_at_the_line_cap() {
        let memory = b"retrying".to_vec();
        let mut budget = LogBudget::default();
        let lines = log_lines(&mut budget, &memory, 10_000, memory.len());

        assert_eq!(lines.len(), LOG_LINES_PER_INVOCATION as usize + 1);
        assert!(lines[..lines.len() - 1].iter().all(|l| l == "retrying"));
        assert_eq!(lines.last().map(String::as_str), Some(LOG_BUDGET_EXHAUSTED));
        assert!(budget.is_exhausted());
    }

    #[test]
    fn an_out_of_bounds_log_prints_nothing_and_costs_nothing() {
        let mut budget = LogBudget::default();
        assert_eq!(budget.next_line(10, &[], |_| None), None);
        assert_eq!((budget.lines_printed(), budget.bytes_read()), (0, 0));
    }

    #[test]
    fn a_capped_line_still_redacts_and_never_prints_half_a_secret() {
        let secret = "sk-live-0123456789".to_string();
        let redactions = std::slice::from_ref(&secret);
        let mut memory = "x".repeat(LOG_LINE_MAX_BYTES - 7 - secret.len());
        memory.push_str(&secret);
        memory.push_str(&secret);
        memory.push_str("tail");

        let mut budget = LogBudget::default();
        let line = budget
            .next_line(memory.len(), redactions, |n| memory.as_bytes().get(..n))
            .expect("first line prints");

        assert!(line.contains("[REDACTED]"));
        assert!(
            !line.contains(&secret[..7]),
            "leaked a secret prefix: {line}"
        );
    }

    #[test]
    fn a_cap_inside_a_character_of_a_secret_still_hides_its_start() {
        let secret = "ab\u{20AC}cdefgh".to_string();
        let mut memory = "x".repeat(LOG_LINE_MAX_BYTES - 3);
        memory.push_str(&secret);

        let mut budget = LogBudget::default();
        let line = budget
            .next_line(memory.len(), std::slice::from_ref(&secret), |n| {
                memory.as_bytes().get(..n)
            })
            .expect("first line prints");

        assert!(!line.contains("ab"), "leaked a secret prefix: {line}");
    }
}
