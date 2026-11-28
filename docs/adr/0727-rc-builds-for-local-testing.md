# 0727: Local rc builds use distinct pre-release versions before the final candidate

- **Status**: Accepted
- **Date**: 2026-11-17
- **Amends**: [0715](0715-update-channels.md): Local staging and the deferred bundle-version question.
- **Relates to**: 0024, 0036, 0039, 0639.
- **Plan**: `docs/plans/2026-11-17-rc-builds-for-local-testing.md`

## Context

Successive local Phase A builds currently share a final version. The updater
correctly refuses an equal version, so a tester cannot install the next build
through Local. Hand-editing a feed cannot give those bytes an honest version.

## Decision

**An rc build has version `X.Y.Z-rc.N`.** The release command
`release.sh --verify-build --rc X.Y.Z` allocates N automatically before
building. Failed and abandoned allocations consume their numbers. An append-only
local allocation log, leftover release worktrees and the Local feed establish
the highest allocated number. An existing final candidate or published target
refuses another rc build for that target.

**rc builds stay on this machine.** They build, sign, notarize, staple and stage,
then publish to Local. They do not push a mirror candidate, deploy an rc front
door, create a draft or dispatch public gates. A release candidate remains the
final-version object gated and promoted under ADR 0024. Beta still carries only
that final candidate's bytes after its gates pass, as ADR 0715 requires.

**Every staging Phase A publishes to Local best-effort.** This includes final
candidates and completed notarization resumes. Failure prints the exact manual
retry as `STILL OWED`. Local refuses lower or equal versions with different
bytes; republishing identical bytes is a no-op.

**Both plist version keys carry the full pre-release version.** Tauri 2.11.2
writes its config version verbatim to `CFBundleShortVersionString` and, absent
an override, `CFBundleVersion`. Developer ID distribution accepts non-numeric
versions; the three-integer restriction is an App Store requirement. The first
live rc build verifies notarization, stapling, Gatekeeper and an updater hop.
The fallback on a notary refusal is a numeric-core Tauri version with the DMG
named from `RELEASE`; runtime comparison still uses the compiled full version.

**The final rebuilds the last rc's source cut.** Without an explicit cut, Mode 1
uses the last recorded rc cut. A differing explicit cut lists untested commits
and needs the usual confirmation. The command explains that the app, DMG,
updater bundle and tarballs must be rebuilt because their compiled version
changes. Phase B refuses pre-release versions and, after publishing the final,
cleans that target's rc worktrees, branches and staging, retaining allocations.

## Rationale

Numeric semver identifiers already sort `rc.10` above `rc.2`, with the final
above every rc. Distinct versions let the existing updater offer every build
without a new comparator. Local testing needs no public writes; final candidates
retain every existing release gate and are promoted verbatim.

## Consequences

- Local testers of the packaged macOS app can install successive rc builds on
  the release machine. Other machines and headless installs remain outside Local.
- Stable and Beta users receive no rc builds from this flow.
- Each build has an honest runtime, changelog and bundle version.
- rc release commits never land on main. Only the final bump does.
- The final needs its own build and full gate; an rc cannot certify those bytes.
- Nightly's build and public rc distribution remain separate work.

## Alternatives considered

- **Fake the next patch version in the feed.** It misrepresents the installed
  bytes and collides with the next real patch.
- **Reuse rc bytes as the final.** The compiled version remains `-rc.N`, so
  About reports an rc and Stable repeatedly offers the final.
- **Use `+rc.N` build metadata.** Semver ignores it for precedence, so successive
  builds are equal and the updater offers no hop.
- **Use a numeric short bundle version now.** It makes Finder disagree with the
  runtime without evidence that Developer ID notarization requires it.
- **Put rc builds on Beta now.** It adds public publishing and gate runs per rc.
  Beta keeps the exact-final-bytes contract; expanding its population can be a
  later decision.
