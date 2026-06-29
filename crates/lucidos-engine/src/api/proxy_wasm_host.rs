//! Host imports exposed to WASM signer modules: time, crypto primitives,
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

use base64::Engine;
use hmac::{Hmac, Mac};
use sha1::Sha1;
use sha2::{Digest, Sha256, Sha512};
use wasmtime::{Caller, Linker, Memory};

/// Per-call state seen by host imports. The pipeline allocates one of these
/// per signer invocation and threads it through wasmtime's `Store` so the
/// secret table is fresh each time and never leaks across callers.
pub struct HostState {
    /// Opaque secret table — indexed by `secret_id` from the module side.
    /// The module knows its handles by name (declared in `WasmManifest`),
    /// the host translates name → index when building the `SignInput`.
    pub secrets: Vec<Vec<u8>>,
    /// Module name for log prefixing.
    pub module_name: String,
    /// Sensitive substrings that must be scrubbed from anything the module
    /// emits through the `log` host import — the resolved secret material (in
    /// its various encodings) plus the auth-header/token values handed to this
    /// signer via `prior_layer_outputs`. Without this a buggy/hostile signer
    /// could `log()` an upstream `script_handshake` auth token into engine logs.
    pub log_redactions: Vec<String>,
}

// ---- Pure-Rust primitive helpers (no wasmtime types) -------------------
//
// These are the actual crypto + encoding work, lifted out of the host
// imports so they can be tested directly against published vectors without
// spinning up a WASM instance.

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

// ---- Memory access helpers --------------------------------------------

/// Look up the module's exported `memory` and return it. Modules built from
/// our signer template export their memory under the standard name.
fn lookup_memory(caller: &mut Caller<'_, HostState>) -> Result<Memory, wasmtime::Error> {
    caller
        .get_export("memory")
        .and_then(|e| e.into_memory())
        .ok_or_else(|| wasmtime::Error::msg("module did not export `memory`"))
}

fn read_bytes(
    mem: &Memory,
    caller: &mut Caller<'_, HostState>,
    ptr: u32,
    len: u32,
) -> Result<Vec<u8>, wasmtime::Error> {
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

// ---- Linker registration ----------------------------------------------

/// Wire all host imports into the given `Linker`. Call once per `Engine`
/// (the linker is reusable across instantiations as long as they share
/// the same engine). The `replace_body` capability gate lives in
/// `WasmSignerLayer::apply`, not here; capability-gated *imports* (e.g. a
/// raw-secret-access primitive) would conditionally skip the
/// corresponding `func_wrap` call below — none defined yet.
pub fn register_host_imports(linker: &mut Linker<HostState>) -> Result<(), wasmtime::Error> {
    // current_time_ns / current_time_secs — wall-clock from the host.
    linker.func_wrap("env", "current_time_ns", |_caller: Caller<'_, HostState>| -> i64 {
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    })?;
    linker.func_wrap("env", "current_time_secs", |_caller: Caller<'_, HostState>| -> i64 {
        chrono::Utc::now().timestamp()
    })?;

    // random_bytes(out_ptr, out_len) -> i32  (returns bytes written, -1 on error)
    linker.func_wrap(
        "env",
        "random_bytes",
        |mut caller: Caller<'_, HostState>, out_ptr: i32, out_len: i32| -> i32 {
            use rand::RngCore;
            if out_len <= 0 {
                return -1;
            }
            let mut buf = vec![0u8; out_len as usize];
            rand::thread_rng().fill_bytes(&mut buf);
            let mem = match lookup_memory(&mut caller) {
                Ok(m) => m,
                Err(_) => return -1,
            };
            match write_bytes(&mem, &mut caller, out_ptr as u32, &buf) {
                Ok(()) => out_len,
                Err(_) => -1,
            }
        },
    )?;

    // sha1/sha256/sha512(data_ptr, data_len, out_ptr) -> i32 (hash length, -1 on error)
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

    // hmac_sha*(secret_id, data_ptr, data_len, out_ptr) -> i32
    linker.func_wrap(
        "env",
        "hmac_sha1",
        |mut caller: Caller<'_, HostState>,
         secret_id: i32,
         data_ptr: i32,
         data_len: i32,
         out_ptr: i32|
         -> i32 {
            run_hmac(&mut caller, secret_id, data_ptr, data_len, out_ptr, |s, d| {
                hmac_sha1_digest(s, d).to_vec()
            })
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
            run_hmac(&mut caller, secret_id, data_ptr, data_len, out_ptr, |s, d| {
                hmac_sha256_digest(s, d).to_vec()
            })
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
            run_hmac(&mut caller, secret_id, data_ptr, data_len, out_ptr, |s, d| {
                hmac_sha512_digest(s, d).to_vec()
            })
        },
    )?;

    // hex_encode(in_ptr, in_len, out_ptr, out_cap) -> i32
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

    // base64_encode(in_ptr, in_len, out_ptr, out_cap) -> i32
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

    // log(ptr, len) — host-side log line, prefixed with the module name.
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
            let bytes = match read_bytes(&mem, &mut caller, ptr as u32, len as u32) {
                Ok(b) => b,
                Err(_) => return,
            };
            let msg = String::from_utf8_lossy(&bytes);
            // Scrub secret material before it reaches the log — a signer can
            // see upstream auth tokens (via prior_layer_outputs) and could echo
            // them here.
            let msg = crate::core::redact_secret_values(&msg, &caller.data().log_redactions);
            let module_name = caller.data().module_name.clone();
            crate::log!("[wasm-signer:{}] {}", module_name, msg);
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
    // Pull the secret out of the host state by index. Cloned because we then
    // need a mutable borrow of the Store for memory ops.
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
    use crate::api::proxy_hex::hex_lower;

    // Test vectors below come from RFC 4231 (HMAC-SHA-2) and RFC 3174 (SHA-1)
    // / FIPS 180-2 (SHA-256/512).

    #[test]
    fn sha1_known_vector() {
        // "abc" → A9993E364706816ABA3E25717850C26C9CD0D89D (FIPS 180-2)
        let h = sha1_digest(b"abc");
        let hex = hex_lower(&h);
        assert_eq!(hex, "a9993e364706816aba3e25717850c26c9cd0d89d");
    }

    #[test]
    fn sha256_known_vector() {
        // "abc" → ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad
        let h = sha256_digest(b"abc");
        let hex = hex_lower(&h);
        assert_eq!(
            hex,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn sha512_known_vector() {
        // "abc" → ddaf35a193617aba... (full hex below)
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
        // RFC 4231, test case 1: key=20×0x0b, data="Hi There"
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
        // RFC 2202, test case 1: same key+data shape
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
}
