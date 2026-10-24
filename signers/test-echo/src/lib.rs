//! Tiny WASM signer used by lucidos-engine integration tests.
//!
//! Wire contract (matches `proxy_wasm_signer::SignInput` /
//! `proxy_wasm_signer::SignOutput`):
//!  - exports `alloc(len: i32) -> i32` — host calls this to reserve space
//!    for the input JSON before writing.
//!  - exports `sign(in_ptr: i32, in_len: i32) -> i64` — host calls this
//!    after writing input. Return value packs `(out_ptr << 32) | out_len`.
//!
//! This signer ignores its input and returns a hardcoded
//! `{"add_headers":[["x-echo","ok"]]}` payload.
#![no_std]

use core::panic::PanicInfo;

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    // Trap the WASM module on panic. wasmtime surfaces this as a runtime
    // error which `WasmSignerLayer::apply` maps to 502.
    core::arch::wasm32::unreachable()
}

// ---- Bump allocator over a static heap --------------------------------
//
// Just enough for tests: write input + reserve output space inside a fixed
// 64KB region (one WASM page = 64KB after instantiation, so this fits in
// the default initial memory).

const HEAP_SIZE: usize = 64 * 1024;
static mut HEAP: [u8; HEAP_SIZE] = [0; HEAP_SIZE];
static mut HEAP_OFFSET: usize = 0;

/// Reserve `size` bytes from the bump heap. Returns the absolute pointer.
/// Test signer never frees — each instance is short-lived (Phase 4's
/// per-call Store + instance lifecycle). WASM is single-threaded inside an
/// instance, so unsynchronized access to `HEAP_OFFSET` is sound.
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

const RESPONSE: &[u8] = b"{\"add_headers\":[[\"x-echo\",\"ok\"]]}";

#[no_mangle]
pub extern "C" fn sign(_in_ptr: i32, _in_len: i32) -> i64 {
    let len = RESPONSE.len();
    let ptr = alloc(len as i32);
    if ptr == 0 {
        return 0;
    }
    unsafe {
        core::ptr::copy_nonoverlapping(RESPONSE.as_ptr(), ptr as *mut u8, len);
    }
    ((ptr as i64) << 32) | (len as i64)
}
