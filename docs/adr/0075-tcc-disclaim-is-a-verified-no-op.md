# 0075: A post-fork TCC responsibility disclaim is a verified no-op; the engine's stable signing identity is the TCC fix

- **Status**: Accepted
- **Date**: 2026-08-13

## Context

macOS attributes a privacy prompt to the process it holds *responsible*, and a
child inherits its parent's responsibility. So Claude Code's prompts inside a
coding-agent session read "lucidos-engine would like to access …", naming a
binary the user never launched. The obvious fix is to disclaim responsibility at
spawn time, so the child is attributed to itself.

The engine tried it: `build_command` in
`crates/lucidos-engine/src/runtime/claude_code.rs` called
`responsibility_set_caller_responsible_for_self()` from a `pre_exec` hook.

## Decision

**The engine attempts no TCC responsibility disclaim.** The prompt keeps naming
`lucidos-engine`. What we do fix is prompt *recurrence*, by signing the engine
binary with a stable self-signed identity at build time
(`scripts/lib/codesign.sh`).

## Rationale

The disclaim cannot work from where Rust puts us. macOS captures TCC
responsibility off the `posix_spawnattr_t` at the exec syscall, and the only
effective knob is `responsibility_spawnattrs_setdisclaim`, set on that attribute
struct. Asking for a `pre_exec` hook forces Rust down the `fork()` plus
`execvp()` path, so no `posix_spawn` runs and the attribute is never consulted.
The call compiles, returns success, and changes nothing.

Both post-fork APIs were measured, not assumed:
`responsibility_set_caller_responsible_for_self()` and
`responsibility_set_pid_responsible_for_pid(self, self)`. Each leaves the child
attributed to the engine in every context tested: a forked child, a running
process, and a parent-side reassignment.

What the user actually complained about is being re-prompted after every
rebuild, and that has a different cause. A `cargo build` binary is
`adhoc, linker-signed`, so its CDHash changes on each build and macOS discards
the prior grant. A stable Designated Requirement (identifier plus certificate
leaf, no CDHash and no path) makes one Allow click stick across rebuilds. That
is a complete fix for the recurrence, and it is orthogonal to which name the
prompt shows.

## Consequences

- The prompt names `lucidos-engine` rather than Claude Code. We accept that.
- One Allow click persists, once `./scripts/dev-codesign-setup.sh` has run.
- A future attempt has to change the spawn mechanism, not add a call. It needs a
  `posix_spawn` path that sets `responsibility_spawnattrs_setdisclaim` before
  exec, which `std::process::Command` with a `pre_exec` hook cannot give.

## Alternatives considered

**Call the disclaim API from `pre_exec` anyway.** This is what shipped and was
then removed. It is inert for the reason above, and an inert call is worse than
no call: it reads as a solved problem.

**Reassign responsibility from the parent after the fork.**
`responsibility_set_pid_responsible_for_pid(self, self)` was tested and left the
attribution unchanged.

**Spawn Claude Code through a small helper binary named for the user.** That
moves the prompt's name to the helper, at the cost of a second binary to sign,
ship and keep in step with the engine. Not worth it for a cosmetic string, when
the recurrence is already fixed.
