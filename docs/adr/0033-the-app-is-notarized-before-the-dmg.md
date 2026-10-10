# 0033: The .app is notarized before the DMG, so a release waits on Apple once even when deferring

- **Status**: Accepted
- **Date**: 2026-08-02
- **Amends**: [0027: A release does not wait on Apple](0027-a-release-does-not-wait-on-apple.md)

## Context

ADR 0027 established that a release does not block on Apple's verdict, because
notarization gates exactly **one** artifact: the `.dmg` a browser downloads.
`--defer-notarization` submits, publishes the signed-but-unstapled DMG behind a
banner, and staples it later. That reasoning is intact and this ADR does not
touch it.

What it did not cover is the `.app` **inside** that DMG. The
[2026-08-02 macOS update-path audit](../audits/2026-08-02-macos-update-path-audit.md)
(F5) found that no shipped DMG has ever contained a stapled app. Verified on all
ten releases it tested:

```
$ hdiutil attach Lucidos_0.19.0_aarch64.dmg -readonly …
$ xcrun stapler validate "$MNT/Lucidos.app"
Lucidos.app does not have a ticket stapled to it.

$ spctl -a -vvv -t exec "$MNT/Lucidos.app"
accepted
source=Notarized Developer ID
```

The `accepted` came from an **online** lookup: the app's cdhash is covered by the
DMG's notary submission, so Apple's service vouches for it with no local ticket.
That is also why `staple_notarized_artifacts` could staple the standalone
`$APP_PATH` at all. But that copy is never shipped. Staging copies the DMG, the
updater tarball and its `.sig`, and the app inside the DMG was injected by
`refresh_dmg_payload` **before** any ticket existed.

So a user who mounts the DMG, drags the app to `/Applications` and launches it
gets a Gatekeeper assessment that has to reach Apple. Apple's own stated reason
for stapling is to make exactly that unnecessary.

**The cheap fix does not exist.** `stapler staple` writes the ticket into the
bundle. Putting it in the copy inside the DMG means rewriting the image, which
changes the image's own cdhash and voids both its signature and its ticket. Two
notary submissions are required, and they cannot overlap, because the DMG has to
be built from the already-stapled app.

## Decision

**Adopt Apple's documented ordering.** A release makes two submissions in
sequence:

1. archive the signed `.app` (`ditto -c -k --sequesterRsrc --keepParent`),
   submit it, staple the bundle, and prove both the ticket and the signature;
2. build the DMG around the stapled app, sign it, submit it, staple it.

**`--defer-notarization` defers the second verdict only.** The first is in the
critical path of every release, because there is nothing to build a DMG from
until it lands.

The alternative was to record the residual and keep one submission: the failure
needs a user who downloads online and then launches offline, and it has not been
observed. It was rejected because "correct only while the network is up" is not
a property to ship deliberately once the fix is understood, and because the
audit's `spctl` evidence shows the assessment really is reaching Apple on every
first launch today.

## Consequences

**The cost, stated plainly.** A release now waits for two Apple verdicts instead
of one, sequentially. Apple's verdicts run 1 to 20 hours; on v0.16.0 the observed
window was 8h06m. `--defer-notarization` goes from "publish without waiting" to
"wait once, then publish", which is a real weakening of ADR 0027's headline
promise and the reason this is an ADR rather than a comment.

**What ADR 0027 keeps.** Its decision table is unchanged: the headless tarball
and the updater trio are still never quarantined, so Gatekeeper still never
assesses them, and deferring the DMG still leaves existing users and
`curl … | sh` installs unaffected. What changed is only how much of the release
can happen before Apple answers.

**Resumability covers both halves.** One handle per version carries a `stage`
field naming which submission is outstanding, so a run killed during either wait
costs a poll rather than a rebuild. A resumed `app` stage runs on into the DMG
half through the same `run_dmg_notarize_stage` a fresh build uses, so the two
cannot drift. `--adopt-app-submission` is the sibling of `--adopt-submission` for
the window between notarytool returning an id and the handle reaching disk.

**The app's identity is asserted by cdhash, not by file hash.** A ticket is
issued for a cdhash, so that is the exact correctness condition, and it is the
only workable one: `ditto -c -k` is not byte-reproducible, so re-archiving and
comparing checksums would report false mismatches on an untouched bundle.

**Stapling does not break the seal, and the build proves it rather than assuming
it.** The ticket is written to `Contents/CodeResources`, outside the sealed
resource set (`Contents/_CodeSignature/CodeResources`), so a stapled bundle still
passes `codesign --verify --deep --strict`. That was confirmed against a stapled
third-party app before the change was designed, and the build re-checks it after
every staple: a future macOS that changed it would fail the release rather than
ship a bundle that will not launch.

**The updater payload is still packed pre-staple**, so an auto-updating user
still receives a Developer ID signed but unstapled bundle. That was ADR 0027's
first accepted cost, and it survives here only because it was left deliberately
out of scope. The reason it was pre-staple has now gone: the app stage is never
deferred, so repacking after the staple would no longer make the payload's
contents depend on whether the release was deferred. Revisiting it is a separate
decision, recorded as a non-goal in
`docs/plans/2026-08-02-paired-notarize-set-and-stapled-app.md`.

**Verification is manual at the next release**, and deliberately so: producing a
real ticket needs Apple and a full build. Mount the shipped DMG and run
`xcrun stapler validate "$MNT/Lucidos.app"`, which must report that the validate
action worked rather than "does not have a ticket stapled to it". The build's own
`stapler validate` plus `codesign --verify` after stapling perform the same check
automatically, so the manual step confirms rather than guards.
