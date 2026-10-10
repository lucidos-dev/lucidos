# 0034: A CI-built artifact is ad-hoc by construction, so the headless front door stays unsigned

- **Status**: Accepted
- **Date**: 2026-08-02
- **Relates to**: [0031: A deploy to a lucidos.dev origin runs on the maintainer's machine](0031-deploys-run-on-the-maintainers-machine.md)

## Context

`.claude/rules/build-release.md` has long recorded that the macOS headless
tarballs on a Release, including the one a Mac's
`curl -fsSL https://lucidos.dev/install.sh | sh` downloads, are the **unsigned CI
ones** from `build-headless.sh`. The
[2026-08-02 macOS update-path audit](../audits/2026-08-02-macos-update-path-audit.md)
(F11) verified it on the shipped artifacts:

```
=== lucidos-0.19.0-aarch64-apple-darwin.tar.gz
  lucidos-engine:  Identifier=lucidos_engine-dff7bfbe5d7dbebc  Signature=adhoc  TeamIdentifier=not set
  lucidos-gateway: Identifier=lucidos_gateway-ebabd94f1c4923ab Signature=adhoc  TeamIdentifier=not set
  lucidos:         Identifier=lucidos-98c1c747a3190669         Signature=adhoc  TeamIdentifier=not set
```

The audit's objection was not that this is wrong. It was that the **stated
reason** was a fact about today rather than a rule:

> there is no signed macOS tarball for CI to clobber

That sentence describes the current wiring (`release.sh` never passes
`--emit-tarball`, so the signed local tarball is a capability attached to
nothing). It gives no guidance about what to do if that wiring changes, and it
invites the reading that wiring `--emit-tarball` in would be a straightforward
improvement. F1 sharpened the question: before F1 the updater payload was ad-hoc
too, so the front door was one unstable identity among several; now that the
updater payload is repacked from the signed app, the front door is the **only**
remaining unstable one, and an asymmetry that used to be background is now the
exception.

This ADR replaces the fact with the principle, and measures the consequence
rather than asserting it.

## Decision

**The Developer ID identity lives only on the release machine and cannot be
handed to CI. Therefore any artifact CI builds is ad-hoc by construction.**

This is the same boundary ADR 0031 drew for deploy credentials, applied to the
signing credential. A Developer ID Application certificate plus its private key
is an identity that can sign anything as this developer, for the lifetime of the
certificate; putting it in a public repository's Actions secrets makes every
workflow, and everything a workflow can be induced to run, a signer. No workflow
in `.github/workflows/` reads an `APPLE_*` secret today, and none should.

The consequence follows directly and is the second half of the rule: **the
headless install path has a code identity that changes on every build.** An
ad-hoc, linker-signed Mach-O has a designated requirement of
`cdhash H"<hash>"`, which moves with every compile, so macOS treats each build as
a different program.

**No change to the release flow.** `--emit-tarball` is deliberately NOT wired
into `release.sh` or `release-to-lucidos.sh`.

## What the headless path actually reaches that macOS gates on code identity

The reason the decision is defensible is not that the cost is unmeasured. It is
that the cost is close to zero, and that is checkable rather than assumed.

TCC gates a specific set of resources. Here is what the headless install runs,
and where it goes:

| what it touches | TCC-gated? |
|---|---|
| `<prefix>/runtime/<stem>/` (the extracted tarball; `~/.lucidos/runtime` by default) | no |
| `<prefix>/<slug>/` (registry, embedded Postgres, fastembed cache, logs, port marker) | no |
| gateway-provisioned workspace dirs, which are RELATIVE and resolve under `<app-data>` (`Workspace::resolve_dir`) | no |
| loopback bind, and the `--bind` listen on other interfaces | no |
| outbound HTTPS (LLM providers, huggingface model fetch, GitHub) | no |

The home directory itself is not gated; only Desktop, Documents, Downloads,
iCloud Drive, removable and network volumes are, and the default layout touches
none of them.

The gated resources it does **not** reach at all, established by absence rather
than by inspection of behaviour:

- **No camera, microphone, screen recording, accessibility, input monitoring,
  contacts, calendars, reminders, photos, or location.** `lucidos-engine`,
  `lucidos-gateway` and the `lucidos` CLI declare **zero** Apple-framework
  dependencies (no `objc2`, no `core-graphics`, no AVFoundation, nothing of the
  kind) in their `Cargo.toml`s. There is no code path to gate.
- **No Apple Events (Automation).** The only `osascript` invocation in the tree
  is in `crates/lucidos-app/src/desktop.rs`, and the headless tarball does not
  contain `lucidos-app` at all: `RESOURCE_NAMES` is `lucidos-engine`,
  `lucidos-gateway`, `lucidos`, `frontend`, `postgres`, `sdk`,
  `system-knowhow`, where `frontend` is the built static assets from
  `crates/lucidos-app/dist` and not a Mach-O. The engine's other `osascript`
  mentions are string literals in `command_guard.rs`'s side-effect classifier,
  which reads command lines rather than running them.
- **No local-network discovery.** macOS 15's Local Network gate covers
  local-network *discovery* (mDNS/Bonjour, multicast, broadcast) and outgoing
  connections to local addresses. The tree contains no mDNS, Bonjour or
  multicast code. A loopback bind and an accepted inbound connection are not
  gated.

**What can be gated, and when.** Two paths reach user-named locations, so a user
who puts them under a gated folder meets the gate:

1. A workspace at an ABSOLUTE path the user chose. `Workspace::dir` takes
   absolute paths verbatim, so `~/Documents/my-workspace` is expressible.
2. Repositories that coding-agent threads work in, and any path chat's bash /
   python tools are pointed at.

For those, an ad-hoc per-build identity means the grant is discarded at every
update. That is a real cost and it is the honest limit of "near zero".

**Not tested, and labelled as such.** The TCC database is SIP-protected and the
audit's brief forbade `tccutil`, so this section is an inventory of what our own
code *reaches*, which is answerable by reading it, plus the documented TCC model.
It is not a measurement of grant loss across an update. A second inference worth
naming separately: a launchd agent running a non-bundled CLI has no natural UI to
attribute a TCC prompt to, so the practical grant mechanism for the headless path
is Full Disk Access granted to the binary in System Settings rather than a
prompt. Both are consistent with the conclusion and neither was measured.

**The path moves anyway.** An update lays the new runtime down at
`<prefix>/runtime/lucidos-<version>-<triple>/` and repoints `current`, so the
binary's path changes per version regardless of its code identity. Stabilising
the identity would remove one of two reasons a grant does not survive an update.

## The three-identity asymmetry

The same engine gets a different code identity depending on how it was
installed. Naming the set is the point, because two of the three moved in the
last week and the third did not:

| install path | identity | stable across builds? |
|---|---|---|
| DMG (`Lucidos.app` dragged to /Applications) | `lucidos-engine`, Developer ID, team `F5D4TE3RG4`, DR anchored to the certificate leaf | **yes**, always was |
| in-app updater (`Lucidos.app.tar.gz`) | same as the DMG, since the payload is repacked from the signed app | **yes**, since F1; ad-hoc with a per-build cdhash on every release up to and including v0.19.0 |
| headless tarball (`curl \| sh`) | `lucidos_engine-<crate metadata hash>`, ad-hoc, DR is a bare cdhash | **no** |

So after F1 the front door is the only unstable identity of the three. That is
the state this ADR accepts, with the measurement above as the reason.

## Consequences

- **The rule is checkable.** "Any artifact CI builds is ad-hoc" can be verified
  by grepping the workflows for an `APPLE_*` secret, which is a stronger property
  than "we happen not to attach the signed tarball".
- **Wiring `--emit-tarball` in is not a small improvement, it is a different
  release topology.** The signed tarball is produced by `build-dmg.sh` on the
  release machine for the HOST triple only; the four published tarballs come from
  a CI matrix over four triples on native runners. Signing them would mean either
  building all four on the release machine (no Linux runner, no Intel Mac) or
  moving the identity into CI (refused above). The honest version of the change
  is "publish a signed macOS-aarch64 tarball alongside the CI ones", which
  introduces two artifacts for one triple and a rule about which the installer
  prefers. Not free, and not worth it for the measured cost.
- **What would reopen this.** Any of: the headless path starting to reach a
  TCC-gated resource by default (a workspace root that defaults under Documents,
  a feature that reads Desktop, anything needing screen recording); Apple
  extending Gatekeeper assessment to `curl`-fetched files, which today carry no
  `com.apple.quarantine` xattr and are therefore never assessed; or a notarized
  signing service that does not require the private key to leave its enclave.
- **The `install.sh` execution smoke stays load-bearing.** `verify_runtime_executes`
  runs the extracted gateway once at install time, so any refusal to launch (a
  wrong-arch tarball, a too-old glibc, and in principle a Gatekeeper refusal) is
  loud and immediate at install rather than an opaque service crash-loop later.
