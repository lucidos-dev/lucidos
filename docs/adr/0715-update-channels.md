# 0715: The update address becomes a named update channel

- **Status**: Accepted
- **Date**: 2026-11-16
- **Amends**: [0543](0543-update-address-is-a-gateway-setting.md): the single
  address, **Use default**, and the rc channel in a dev workspace.
- **Relates to**: 0024, 0031, 0034, 0036, 0039, 0108, 0462, 0625.
- **Plan**: `docs/plans/2026-11-16-update-channels.md`

## Context

ADR 0543 made the update address one gateway setting: one `latest.json` URL in
`[release_check] update_address`. Using it for a while showed three faults:

- **It holds one value.** Switching between the published release and a test
  feed means pasting a URL each time.
- **Use default throws the URL away.** Going back to the published release
  loses the address, so the next test starts from a blank field.
- **The local rc address moves.** It names the dev engine's API port, and that
  port changes when the engine restarts on another one. The stored address
  then points at nothing, or at a different workspace.

The owner also asked for public pre-release channels, as other desktop apps
offer. One is for release candidates, for people who want a release early. The
other is for builds from `main`, for people who want every change.

## Decision

**The setting names an update channel, not a URL.** A channel resolves to a
`latest.json` address at check time. The built-in channels are:

| Channel | Carries | Feed |
|---|---|---|
| **Stable** | published releases | unchanged: the GitHub `latest.json` for the app, the ADR 0108 route for the notice |
| **Beta** | each release candidate, before it is published | `https://updates.lucidos.dev/beta/latest.json` |
| **Nightly** | a build of `main`, at most once a night | reserved: `https://updates.lucidos.dev/nightly/latest.json`, not yet published |
| **Local** | a candidate staged on this machine | served by this machine's gateway, at its own origin |

**Custom slots remember named URLs.** A slot is a name and a `latest.json`
URL under the ADR 0543 transport rule. Names contain 1 to 64 characters, with no surrounding whitespace, controls,
colon or slashes. Selecting another channel keeps every slot. Removing a slot is its own action.

**The pick persists.** It lives in `~/.lucidos/updates.toml`, as ADR 0543's
address did:

```toml
[release_check]
enabled = true
channel = "beta"            # stable | beta | nightly | local | custom:<name>

[[release_check.custom]]
name = "Release test"
url = "https://example.com/lucidos/latest.json"
```

**A Beta build is the release candidate itself.** Beta carries the bytes that
Phase A staged, at the version they will ship as. When the release is
published, Beta users already run it. A version that reached Beta is never
reused. An abandoned candidate retires its version, and the next candidate
takes the next patch.

**A Beta build is published by hand, after its gates pass.** The owner runs
one command after Phase A. It refuses while any rc gate on that candidate is
red or still running. A fault found on Beta is fixed forward by the next
candidate.

**Nightly is reserved, not yet published.** The channel exists so the setting,
the order and the comparator already cover it. Settings hides it and every
gateway refuses it until a later decision builds and hosts it. That decision
must settle one more question: a nightly of the private `main` would publish
unreleased work, and possibly its source, every day.

**A Nightly build will carry a pre-release version.** It reads
`<base>-nightly.<yyyymmdd>.<n>`. The base is the next minor version above the
newest version that is released or cut. So a nightly built before a cut sorts
below that release, and one built after it sorts above.

**Every feed offers the newest build on its channel or a more stable one.**
Stable is the most stable, then Beta, then Nightly. Publishing Stable therefore
also moves the Beta feed when it is newer than what Beta offers.

**An update never moves to a lower version.** Every check compares the offered
version with the running one under semver, pre-release ordering included. A
lower or equal offer is not an update. Switching to a slower channel keeps the
running build, and Settings says it is ahead of the channel. The build moves
again when the channel passes it.

**One signing key for every channel.** Beta, Nightly, Local and Custom bundles
are signed with the updater key that signs Stable. The app checks every bundle
with its baked `plugins.updater.pubkey`. The ADR 0543 transport rule applies
unchanged to every feed and every bundle URL.

**The Local channel is served by the gateway the app already asks.** The
gateway keeps one staged candidate in its own home, under
`update-channels/local/`. It serves it on two fixed loopback routes. It builds
the address from its own origin at check time, so a port change moves the
address with it.

**Beta carries the packaged macOS app only, and so will Nightly.** A headless
install or a container stays on Stable. Its gateway refuses another channel
with a reason, since nothing there could install the build.

## Rationale

- **A name, not a URL, is what the user picks.** Chrome, Firefox and VS Code
  ship channels, and Sparkle and electron-updater read a feed per channel. A
  remembered name also makes the switch one tap.
- **Beta reuses rc-first instead of adding a build.** ADR 0024 already makes the
  candidate the published artifact. Publishing those same bytes to Beta costs
  one upload, and Beta testers test exactly what Stable users get. A separate
  `-rc.N` build would need a second notarization and would test other bytes.
- **Nightly needs its own version, and semver gives it one.** A nightly has no
  release to share bytes with. A pre-release tag sorts it between releases
  without a second numbering scheme. Basing it on the newest cut keeps the
  order faithful to what each build contains.
- **The order must follow what a build contains, because migrations run one
  way.** An older engine refuses a database a newer one migrated (ADR 0625).
  An offer that sorts higher but carries fewer migrations would strand the
  workspace. The base rule above prevents that.
- **Never downgrading is the only safe answer for the same reason.** The way
  back from a newer build is a fresh install plus a backup restore. That is a
  deliberate act, not an update check.
- **One key keeps one trust root.** The app bakes one public key, and the
  Tauri updater verifies against one key. A key per channel would need
  multi-key verification in every shipped binary. It would also protect
  little, since both keys would sit on the same release Mac.
- **Feeds get their own origin.** A Cloudflare Pages deploy replaces a
  project's whole manifest, which is why the rc front door has its own
  project. A feed inside `lucidos.dev` would republish the landing page on
  every channel publish. A separate `updates.lucidos.dev` project deploys alone.
- **Bundles stay on GitHub.** Pages refuses files above 25 MiB, and an updater
  bundle is about 100 MB. Every installable Lucidos artifact already lives on
  the public mirror's Releases, and a channel bundle joins them.
- **The gateway keeps its no-redirect client.** A GitHub download URL
  redirects, and the gateway's release check follows no redirect (ADR 0108). A
  feed on our own origin keeps that rule. Only the app's updater follows the
  bundle's redirect, and the signature check covers what it fetches.

- **The Beta ledger is a private source artifact.** Its version/hash reservation
  commits before public mutation. Later state commits include only that file,
  so publishing Beta cannot leave Phase B's checkout dirty with bookkeeping.
  Rebuilt candidate feeds come from that ledger and immutable GitHub assets;
  `.lucidos` remains a cache. The ledger never enters the public release tree.

## Consequences

- **Stable behaves exactly as today.** With Stable picked, the gateway sends the
  ADR 0108 request and the app reads its configured endpoint.
- **The existing address migrates.** A stored `update_address` becomes a custom
  slot named "Custom address", and it stays selected. The key is dropped on the
  next write. A gateway older than this ADR reads the new file as Stable.
- **The rc channel is renamed the Local channel.** It moves from the dev
  workspace into the gateway's home. `scripts/publish-rc-channel.sh` becomes
  `scripts/publish-local-channel.sh`.
- **The gateway grows two read-only routes.** They serve one feed and the one
  bundle it names, to loopback peers, behind the local token. No path comes
  from the request.
- **A release version may carry a pre-release tag.** Every Lucidos release
  comparison, in Rust and in TypeScript, moves to semver ordering. Dev build
  IDs keep their four-component CalVer order through a separate comparator.
  Plugin version checks already use semver and stay as they are.
- **Nightly, when built, signs on the release Mac.** Its key and Apple identity
  live only there, so CI cannot build it. That later decision owns the cost.
- **Release publishing gains a channel step.** Phase A can publish its candidate
  to Beta by hand. Phase B moves the Beta feed when Stable passes it. `--abandon` after a Beta publish retires the version.
- **The mirror gains one rolling prerelease**, `beta`. Its one tag moves, so
  the public releases page gains one row, not one per candidate.
- **ADR 0036 still holds.** The `rc-<version>` gate release stays a draft. The
  Beta prerelease is a separate, deliberate public release of the same bytes,
  made only when the owner publishes it.

## Alternatives considered

- **Keep one address and add a history of recent URLs.** Lost: it fixes the
  lost URL but not the moving port, and gives no public channels.
- **Beta as its own `-rc.N` build.** Lost: a second build and notarization per
  candidate, and Beta would test bytes Stable never ships. The chosen rule
  costs a skipped patch number when a Beta candidate is abandoned.
- **A separate signing key per channel.** Lost: see the rationale. The app
  trusts one key, and multi-key support buys nothing while both keys share one
  machine.
- **Allow a downgrade on switching to a slower channel.** Lost: migrations
  run one way, so a downgrade by update can strand the database.
- **Feeds inside the `lucidos.dev` Pages project.** Lost: every feed publish
  would republish the landing page. That coupling is what split the rc front door
  out.
- **Feeds and bundles both on a GitHub rolling prerelease.** Lost: the gateway
  would have to follow GitHub's redirect. Channel control would also sit with
  GitHub rather than on our own origin (ADR 0108).
- **Bundles on Cloudflare R2.** Lost for now: a new store and a new credential,
  where the mirror's Releases already host every installable artifact.
- **Build Nightly in GitHub Actions, when it comes.** Lost: it would put the
  Apple and updater signing credentials in CI. ADR 0031 keeps deploys on the maintainer's
  machine, and ADR 0034 keeps CI artifacts ad-hoc.
- **Keep the local candidate in a dev workspace and resolve its port at check
  time.** Lost: the workspace can run under another gateway. The bundle URL
  inside its `latest.json` would also still name the old port.
