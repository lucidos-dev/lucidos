# 0027 — A release does not wait on Apple: the DMG is deferred, labelled, and swapped in place

- **Status** — Accepted
- **Date** — 2026-07-29

## Context

Every Lucidos release blocked on Apple's notary service. Phase A
(`release.sh --verify-build`) only *stages* artifacts after
`notarize_await_verdict` returns `Accepted`, and Phase B
(`release.sh --publish-verified`) refuses without a verified staging dir. So a
release that was finished in every other respect — tree scanned, RC gate green,
changelog approved, binaries built and signed — could not go out until Apple
answered.

That wait is not a few minutes. Across the 0.16.0 cycle it ran from ~10 minutes
to **14+ hours**, including a head-of-line queue wedge on 2026-07-27/28 where
five submissions all returned within one ~10-minute window after being stuck
overnight. ADR 0024 made a release *reproducible*; it did not make it
*shippable on our own schedule*.

The unlock came from asking which artifacts notarization actually gates. The
answer is **one**:

| artifact | consumed by | Gatekeeper assessment? |
|---|---|---|
| headless tarball + `.sha256` | `curl … \| sh` | **no** — `curl` sets no `com.apple.quarantine`, so nothing is assessed |
| `.app.tar.gz` + `.sig` + `latest.json` | Tauri in-app updater | **no**: the updater writes the bundle itself and sets no `com.apple.quarantine`, so Gatekeeper performs no assessment on launch; integrity is our minisign (Ed25519) key, which *is* checked |
| **`.dmg`** | browser download | **yes** |

Note (2026-08-02): the `.app.tar.gz` payload was ad-hoc signed rather than
Developer ID signed for every release through v0.19.0 (F1 in
`docs/audits/2026-08-02-macos-update-path-audit.md`), and is repacked from the
signed bundle as of that day. That was worth fixing on its own merits, but it is
not what the middle row turns on: the absence of a quarantine xattr is. So a
signature on the payload is not the launch mechanism here and must not be cited
as one, now that it exists any more than when it did not.

Gatekeeper's notarization check is driven by the quarantine extended attribute,
which the *downloading application* opts into — browsers, Mail, AirDrop. Neither
`curl` nor the Tauri updater does. So the population actually affected by a
missing ticket is "someone who downloads the DMG from a browser during the
notary window", not "our users".

The cost to that population is real and got worse: macOS Sequoia (15) removed
the Control-click → Open override, so the workaround is now a five-step trip
through System Settings → Privacy & Security with two administrator-password
prompts.

## Decision

**Publish on our schedule; let the DMG catch up.** A release may be published
while its notarization verdict is outstanding, provided the unnotarized DMG is
labelled and the advertised download path never points at it.

1. **`build-dmg.sh --defer-notarization`** submits, persists the resume handle,
   and stages the **unstapled** DMG with `notarized: false` in the staging
   manifest. Explicitly opt-in — no path falls back to it — and refused on any
   mode that would upload in the same process, because that is where no banner
   can be composed.

2. **The state travels in the staging manifest, not in a flag.** Every
   public-facing consumer derives its behaviour from `manifest.notarized`: the
   release body, the cleanup decision, the site link. A pending DMG therefore
   cannot reach a Release page without its warning, and the warning cannot
   appear on a release that doesn't need it. An **absent** key means notarized —
   the only writer that omits it predates this mode and staged solely after an
   `Accepted` verdict.

3. **The banner is on the GitHub Release body only.** `$NOTES_FILE` — the plain
   changelog section — still feeds `latest.json`, which is what the in-app
   updater displays, and updater users are unaffected. Telling them to visit
   System Settings would be a lie.

4. **`release.sh --attach-notarized <version>`** finishes it: poll, staple,
   re-stage, `--clobber` the asset in place, rewrite the body without the
   banner, dispatch the clean-machine DMG gate against the published tag, emit
   `ReleaseDmgNotarized`, and only then run the cleanup Phase B deferred.
   Ordered so every step leaves the previous state intact — in particular the
   banner comes down *after* the stapled asset is up.

5. **A deferred publish keeps what the attach step needs** — worktree, staging,
   `verify-build-<v>.env`, the notarize handle, the submitted-bytes pin. On the
   normal path all five are still cleaned up as before.

6. **The site tells the truth while it is behind.** `lucidos.dev` keeps serving
   the last notarized DMG and gains a "newer version available" notice; both
   flip when `ReleaseDmgNotarized` lands.

7. **Verification follows the artifact.** `dmg-verify` gains a `dmg_tag`
   dispatch input so it can gate a *published* tag. A deferred release never
   creates the `rc-<ver>` prerelease that normally fires it — an unstapled DMG
   cannot pass a stapled-ticket assertion, and arming a gate that must fail is
   worse than not arming it.

## Consequences

- A release goes out when *we* are ready. Existing desktop users get it
  immediately via the updater; terminal users via the tarball. Only a
  first-time Mac visitor during the window is affected, and the site steers them
  to the last notarized build rather than the pending one.
- **Early auto-updaters run an unstapled bundle permanently.** The updater
  tarball is built pre-staple, and re-issuing a stapled one afterwards cannot
  reach anyone already on that version. Invisible in practice (nothing assesses
  a non-quarantined bundle); it would only surface if that user AirDropped the
  `.app` to another Mac, which re-quarantines it. Accepted, not designed around.
- **An `Invalid`/`Rejected` verdict now lands on an already-public asset.** That
  DMG can never be notarized, so the recovery is to pull it with
  `gh release delete-asset`, leave the banner up, and fix in a patch release.
  The attach step prints exactly this. Deliberately not automated — retracting a
  published artifact is a decision, not a script.
- The Release Cockpit shows a deferred release as complete while its `notarize`
  step is still outstanding; that step succeeds later, at attach time. Cosmetic,
  and not worth a second completion model.

## Alternatives rejected

- **Keep waiting.** The status quo. Rejected: the wait is unbounded, externally
  controlled, and blocks work that is otherwise finished.
- **Publish with no DMG at all.** Strictest posture, but it leaves the Release
  page with no Mac download for hours and 404s the updater endpoint
  (`releases/latest/download/latest.json`) for every existing desktop user —
  strictly worse than shipping a labelled one.
- **Point the site at the unnotarized DMG with instructions.** Considered and
  rejected by the maintainer: a first-time visitor meeting a "could not verify
  … malware" dialog is the worst possible first impression for a product whose
  pitch is a one-click install, and it teaches exactly the click-through habit
  Apple removed the shortcut to stop.
- **Keep `rc/<version>` and its prerelease alive past GA so `dmg-verify` can
  fire the usual way.** Rejected: a prerelease sitting above the GA release on
  the public releases page is more confusing than a dispatch input, and it
  muddies `release_promote_preflight`'s "the rc is the thing being promoted"
  semantics. (ADR 0036 later removed the premise: the rc release is a DRAFT now,
  so it is never publicly listed, and since a draft fires no event the dispatch
  input above is what arms the rc gate too.)
- **Re-issue a stapled updater tarball at attach time.** Rejected: it would
  invalidate the staged `.sig` that `latest.json` already advertises, and it
  cannot reach the users it would be for (see Consequences).
