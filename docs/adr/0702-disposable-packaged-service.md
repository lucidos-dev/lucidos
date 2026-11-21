# 0702: Disposable packaged services own fresh storage and a bounded process tree

- **Status**: Accepted
- **Date**: 2026-11-15

## Context

Coding-agent threads must run their actual signed package through Lucidos.
The persistent gateway refuses worktree binaries because it survives the
thread and would keep serving a frozen build (ADR 0021).
An isolated HOME alone does not change that lifecycle: persistent engines
survive gateway death, and embedded Postgres daemonizes.

## Decision

Add an explicit disposable app-service role with a fresh storage scope and
an owned gateway process group. Keep persistent launch guards unchanged.

The disposable app launches its bundled gateway with an explicit argument,
a cleared environment, loopback listeners, and fresh gateway, HOME and private
storage. The gateway claims that scope once, before opening its registry.
The cleared environment preserves a canonical `LUCIDOS_AGENT_SANDBOXED=1`
only when supplied by the parent. macOS seatbelts cannot nest, so dropping
this fact would break the engine's existing inherited-sandbox mode. No
launch stage invents the marker or changes the sandbox preference.
Every child selects the existing file key store under the scope's private
directory, including workspace secrets and backup keys. A signed bundle must
not resolve those keys through the machine's Keychain in this role. The
gateway validates this selection and never binds the shared OAuth callback port.
It cannot adopt engines, reload itself, pair gateways or register external
workspace directories. Its control surface permits fresh workspace creation
and listing, read-only user-directory discovery, and the registered engine's
boot-phase/boot-failure reports. All retain normal authentication. Workspace
API requests also pass through normal authentication.

An app bundle is a source-discovery boundary even when built inside a checkout.
The engine and gateway must not walk above `.app` to discover platform source.
A packaged engine's recovery fallback is its own workspace, never its embedded
compile-time source path. Otherwise a disposable package built in a coding-agent
worktree can scan and operate on the live source checkout.

The app starts a separate supervisor with a pipe as its lifetime channel.
The supervisor owns the gateway group, watches controller loss and gateway
exit, and enforces a fifteen-minute limit. It signals the group before reaping
its leader, preventing PID reuse. After stopping the group, it stops the
separately daemonized Postgres cluster.
Requested stop and controller loss succeed after cleanup; lifetime expiry and
unexpected gateway failure remain failures. The supervisor asks the gateway
leader to stop with its supported SIGUSR1 before escalating the owned group.

## Rationale

A typed launch scope carries storage and lifetime requirements together.
An inherited environment flag cannot select it. The supervisor survives abrupt controller loss and owns every cleanup step.
The persistent gateway keeps its recovery behavior and worktree refusal.

## Consequences

This role supports disposable package tests, independently of workspace names
and machine paths. It does not install services or replace a running stack.
The harness must use a new scope on each run and retain failure diagnostics.
The signed macOS package remains the authority for proxy and Python results.

## Alternatives considered

- A general worktree opt-out leaves persistent adoption and detached engines active.
- Copying binaries outside a worktree evades the guard without fixing lifecycle ownership.
- Testing an installed package does not verify the thread's changes.
- Asking for a terminal invocation prevents the owner from operating entirely through Lucidos.
