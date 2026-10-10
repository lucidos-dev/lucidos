# 0160: A bundled helper process carries its own entitlements; the outer .app's never reach it

- **Status**: Accepted
- **Date**: 2026-08-29

## Context

`Lucidos.app` ships three executables in `Contents/Resources`: the engine, the
gateway and the CLI. None of them is the app binary. The gateway is launched as
a **separate process**, and it launches the engine as another one.

`sign_app_bundle` signs every loose Mach-O in the bundle with `--options
runtime`, and applied `Entitlements.plist` to the outer `.app` alone. Its
comment argued that was least privilege, because "the ~200 files above capture
nothing". True of the camera, and the reasoning silently generalised to every
capability.

The engine embeds wasmtime and compiles a signer module to native code on the
proxy auth path. Under the hardened runtime, macOS forbids executing a page no
signature covers. Release 0.32.0 therefore SIGKILLed its own engine the instant
any proxy with a Wasm signer was called: `EXC_BAD_ACCESS`, termination namespace
`CODESIGNING`, indicator `Invalid Page`. The agent retried on resume and killed
it again, once per turn.

## Decision

**A bundled executable that needs a hardened-runtime capability gets its own
entitlements file, applied to that binary by name.** The outer `.app` signature
covers the app process and nothing else.

The engine gets `crates/lucidos-app/EngineEntitlements.plist`, holding
`com.apple.security.cs.allow-unsigned-executable-memory` and nothing else.
`sign_app_bundle` passes it for the one path whose basename is `lucidos-engine`.

**And the entitlement is proved by running the signed bytes**, not by reading
them back. The engine answers `--wasm-selftest`, which compiles a tiny module
with the signer's own wasmtime config and calls it. The build runs it against
the just-signed binary and fails on any non-zero status.

## Rationale

Entitlements are a property of a code signature, and a signature covers one
Mach-O file. A separate process is governed by its own. So "the app claims it"
says nothing about the engine, whatever the bundle layout suggests. The old
comment's least-privilege framing hid that behind a conclusion that happened to
be right for the camera.

The readback alone would not have been enough, and this is the load-bearing
half. Two measurements say why.

First, `com.apple.security.cs.allow-jit` is the obvious key by name and it does
not work here. Under a Developer ID signature and the hardened runtime, a probe
reproducing wasmtime's exact allocation still dies with `SIGKILL` given
`allow-jit`, and runs given `allow-unsigned-executable-memory`. `allow-jit`
covers `MAP_JIT` mappings; wasmtime maps anonymous memory and `mprotect`s it to
read plus execute. A readback gate would have passed the wrong key and shipped a
second crashing release.

Second, an **ad-hoc** signature (`codesign -s -`) does not honour these
entitlements the way a real certificate does. The first pass measured the matrix
ad-hoc, read `allow-jit` as sufficient, and was wrong. Any future measurement
here has to use a real identity.

Running the artifact costs about a second and cannot be fooled by either
mistake.

## Consequences

- The engine may create executable pages from unsigned memory. That is a real
  widening of what the engine process may do, and it is the price of embedding a
  JIT. It is confined to the engine: the gateway, the CLI and the app keep none
  of it, and the build refuses a `cs.allow-` key on the outer bundle.
- The macOS headless tarball is fixed by the same change, because it copies the
  already-signed engine out of the `.app` with `ditto`.
- Every DMG build now executes the engine once. It exits before constructing
  anything, so it needs no database, no workspace and no network.
- Adding a bundled executable that JITs, sandboxes, or needs any other
  hardened-runtime capability means adding its own plist and its own proof. Do
  not extend the engine's.

## Alternatives considered

**Put the key in the outer `Entitlements.plist`.** Simplest edit, and it fixes
nothing: the engine is a different process with a different signature. It would
also widen what the app process may do, for a capability the app does not use.

**Sign the engine without `--options runtime`.** The hardened runtime is what
forbids the page, so dropping it would work. Notarization requires it, so this
trades a crash for an unnotarizable bundle.

**Grant `allow-jit`, alone or beside the working key.** Alone it is measured not
to work. Beside it, it is dead weight that reads as though it were doing
something, which is worse than absent: the next person here would have no way to
tell which key is load-bearing.

**Make wasmtime use `MAP_JIT` so `allow-jit` suffices.** The narrower
entitlement is genuinely preferable, and wasmtime offers no such switch. It
would mean patching a dependency to narrow one key.

**Stop running Wasm in the engine.** The signer sandbox is the reason wasmtime
is there. Removing it to avoid an entitlement is a much larger decision than
this one, and nothing about the crash argues for it.

**Gate on the entitlement readback only, with no selftest.** Rejected on
measurement rather than principle: the readback passes for `allow-jit`, which
crashes. A static check here reports what was asked for, never what macOS
allowed.
