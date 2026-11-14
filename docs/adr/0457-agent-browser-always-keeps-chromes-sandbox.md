# 0457: The agent browser always runs inside Chrome's sandbox; no opt-out, no runtime probe

- **Status**: Accepted
- **Date**: 2026-10-12

## Context

The *agent browser* launched Chrome with `--no-sandbox --disable-setuid-sandbox`.
It browses whatever sites a task needs. Without the sandbox, a page that
exploits a renderer bug runs with the user's full rights: their files, keychain,
workspace and provider credentials. No model call or tool approval stands in
the way, and the agent sees nothing.

Plan: `docs/plans/2026-10-12-agent-browser-keeps-chromes-sandbox.md`.

## Decision

The agent browser always runs inside Chrome's sandbox, on every OS and in both
headless and visible modes. The vendored chromiumoxide cannot express anything
else. A host where Chrome cannot sandbox gets no agent browser and an error
that names the host fix.

## Rationale

- **The sandbox costs nothing where we ship.** On macOS Seatbelt needs no setup,
  and the `.app` is not App-Sandboxed, so Chrome's own sandbox starts under it.
  On Linux the Google Chrome package works through user namespaces or its setuid
  helper. The headless tarball's systemd unit blocks neither.
- **Chrome fails closed by itself.** Without `--no-sandbox`, Linux Chrome either
  sandboxes or aborts ("No usable sandbox!", or a refusal to run as root). It
  never downgrades quietly. So never asking for `--no-sandbox` is enough to make
  the unsandboxed state impossible.
- **The code cannot ask.** `BrowserConfigBuilder::no_sandbox()` is gone from the
  vendored crate, so a call fails to compile. Its `launch` refuses every
  sandbox-weakening switch passed through `.arg()`.

## Consequences

- A Linux host running Lucidos as root, or in a container whose seccomp profile
  blocks user namespaces, has no agent browser. The error says how to fix the
  host. It never suggests `--no-sandbox`, though Chrome's own message does, so
  Chrome's stderr goes to the engine log only.
- A sandbox failure is a typed `SandboxUnavailable` launch outcome. The
  stale-profile retry in `ensure_browser` cannot catch it.
- `the_agent_browser_renderers_are_sandboxed` asks the kernel whether each
  renderer is confined. A move back to upstream chromiumoxide, which still has
  `no_sandbox()`, must keep it passing (`docs/temporary-measures.md`).

## Alternatives considered

- **An explicit opt-out for root and container hosts**, as OpenClaw offers
  (`noSandbox`). Rejected: it reopens the exact hole. OpenClaw's container
  browser runs `--no-sandbox` because the container holds only the browser. A
  Lucidos container holds the workspace, Postgres and the provider credentials,
  so it is no boundary for a browser running inside it.
- **A silent fallback to `--no-sandbox`** when a sandboxed launch fails.
  Rejected: a host would lose its protection with nothing to show for it.
- **A runtime probe after launch** that checks the renderers and kills an
  unsandboxed Chrome. Rejected: Chrome enforces its sandbox once we stop asking
  it not to. A probe would guard a state that cannot occur, as a
  detect-and-recover path. The check lives in a test instead.
- **Bundling a browser** (Chrome for Testing) so every Linux host has one that
  sandboxes. Out of scope. Its downloaded binary also lacks the setuid helper,
  so it would not fix the AppArmor case on its own.
