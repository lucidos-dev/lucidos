# macOS update and distribution path: read-only audit

- **Date** 2026-08-02
- **Tree audited** `f83257a56` (main at v0.19.0 plus two mobile-shell commits)
- **Artifacts audited** every published release of `lucidos-dev/lucidos` that carries an updater payload: 19 releases, v0.12.0 through v0.19.0
- **Nature** read-only. Nothing under `scripts/` was modified, no release action was taken against the mirror, `/Applications/Lucidos.app` was inspected but not touched, `tccutil` was not run.

A sibling coding-agent session is concurrently rewriting `scripts/build-dmg.sh`,
`scripts/lib/codesign.sh`, `scripts/lib/release_signing.sh` and the
`scripts/lib/*_test.sh` harnesses. Every line number below refers to this
worktree's snapshot at `f83257a56` and may already have moved.

---

## Executive summary

**Every one of the 19 published releases that carries `Lucidos.app.tar.gz`
ships an ad-hoc, linker-signed updater payload with zero sealed resources.**
Not v0.15.0 onward, and not "since a recent regression": the divergence dates to
`bbc39374a` (2026-06-18), the commit that first made `cargo tauri build` skip
macOS codesigning, and the very next commit `a7f4fba06` restored the updater
`.sig` while leaving the payload unsigned. There has never been a release whose
updater payload carried a Developer ID signature. I verified all 19
individually.

The consequence is worse than "the app is ad-hoc". `codesign --verify` on the
shipped payload **fails outright**, because the bundle has an embedded
linker-signature that claims sealed resources and no `_CodeSignature` directory
to hold them. So in the app that reaches `/Applications` after any update,
`Info.plist`, the engine, the gateway, the `lucidos` CLI, the whole 134-file
Mach-O set including the relocatable Postgres tree, the frontend and the SDK are
all **unsealed**. In the DMG-installed app they are sealed by one
`_CodeSignature/CodeResources`.

The single worst thing the sibling session is **not** fixing is **F3**: the
notarization resume path pairs a checksum-pinned DMG with whatever
`.app.tar.gz` / `.sig` / `.app` happen to be sitting in
`target/release/bundle/macos` at that moment, with no checksum tying them to the
build that produced the DMG. `assert_dmg_is_the_submitted_bytes` will even
*restore* the DMG from its pin after a concurrent rebuild overwrote it, which
guarantees the mismatch it then stages and ships. That is the same shape as the
bug that triggered this audit ("the thing we verified is not the thing we
shipped"), one level up.

Two other things worth reading first. **F2**: nothing anywhere in the repo ever
runs `codesign` against the shipped updater payload, which is precisely why F1
survived 19 releases and why fixing F1 without adding a gate leaves the class
open. **F7**: three separate documents, including ADR 0027's decision table,
state as fact that the updater payload "launch passes on the Developer ID
signature". That sentence is the reason nobody looked.

The good news, all verified rather than assumed: **every DMG is correctly
Developer ID signed, notarized and stapled** (10 of 10 tested), **every updater
Ed25519 signature verifies against its shipped tarball** (10 of 10), and **every
`latest.json` is internally consistent** with the `.sig` asset and the tag it
was uploaded to (10 of 10). Updates are not broken, they are unsigned.

---

## Findings

| id | sev | one line | file(s) | sibling fix covers it? |
|---|---|---|---|---|
| F1 | critical | The updater payload is ad-hoc/linker-signed with unsealed resources on all 19 releases that have one | `scripts/build-dmg.sh:1467`, `:1560-1565`, `:1603` | **yes**, that is the bug it is fixing |
| F2 | high | Nothing in the pipeline or CI ever inspects the shipped updater payload's Apple signature; `dmg-verify` covers only the DMG | `scripts/build-dmg.sh:499-534`, `.github/workflows/install-smoke.yml:353-416` | **unknown**; recommend an explicit gate |
| F3 | high | The notarize resume path pairs a pinned DMG with unpinned updater artifacts found on disk, and can be driven into the mismatch by its own recovery branch | `scripts/build-dmg.sh:893-906`, `:1079-1090`, `:502-505` | **no** |
| F4 | medium | `DMG_PATH` discovery does not exclude `refresh_dmg_payload`'s `.rw.dmg` / `.zlib.dmg` leftovers, and the version-stamp guard cannot catch them | `scripts/build-dmg.sh:1473`, `:1483-1488`, `:1577-1601` | **no** |
| F5 | medium | The `.app` inside every shipped DMG carries no stapled ticket; the copy that gets stapled is never shipped | `scripts/build-dmg.sh:1603`, `:925-934` | **no** |
| F6 | medium | Engine / gateway / CLI in the updater payload carry per-build ad-hoc identifiers and a bare-CDHash designated requirement, so TCC re-prompts on every update | `scripts/build-dmg.sh:1467`, `scripts/lib/codesign.sh` (dev counterpart) | **yes**, as a consequence of F1 |
| F7 | medium | Three documents assert the updater payload "launch passes on the Developer ID signature". It never has | `docs/adr/0027-...:28`, `docs/desktop-app.md:274`, `scripts/build-dmg.sh:96` | **no** |
| F8 | medium | `latest.json` finishes uploading before the tarball it points at; on v0.16.0 that window was 8h06m | `scripts/build-dmg.sh:630-632` | **no** |
| F9 | low | A failed bundle swap destroys the backup and leaves no app at `/Applications`, with a `KeepAlive` launchd job pointed at it | `tauri-plugin-updater` 2.10.1 `updater.rs:1217-1307` (upstream) | **no** |
| F10 | low | `latest.json`'s platform key comes from the *upload host's* `uname -m`, not from the artifact | `scripts/build-dmg.sh:592-597` | **no** |
| F11 | low | The macOS headless tarballs on Releases are ad-hoc-signed, so the `curl \| sh` front door lays down unsigned Mach-O too | `.github/workflows/release-tarballs.yml`, `scripts/build-headless.sh` | **no** (documented as intentional) |
| F12 | low | `updater:default`, including `download_and_install`, is granted to a plain-HTTP loopback origin | `crates/lucidos-app/src/desktop.rs:306-360` | **no** (largely mitigated in place) |

---

## F1 (critical). The updater payload has never been Apple-signed

### What is wrong

`scripts/build-dmg.sh:1467` runs the Tauri build with the Apple identity removed
from the subprocess environment:

```bash
(cd "$APP_DIR" && env -u APPLE_SIGNING_IDENTITY cargo "${TAURI_BUILD_ARGS[@]}")
```

That is deliberate and the reasoning at `:1453-1466` is sound: `--no-sign` would
suppress the updater `.sig` as well (the v0.11.0 failure), so the identity is
withheld instead. But `cargo tauri build` packs `Lucidos.app.tar.gz` **during**
that same invocation, from the app as it exists at that moment, which is
unsigned. `sign_app_bundle` at `:1560-1565` then signs the app, and
`refresh_dmg_payload` at `:1603` re-injects the signed app into the DMG. Nothing
re-packs the tarball. The DMG gets the signed app; the updater gets the unsigned
one.

### Blast radius (verified)

19 releases carry `Lucidos.app.tar.gz`. I downloaded and extracted all 19 and
ran `codesign -dvvv` on each payload. Every single one:

```
Signature=adhoc
TeamIdentifier=not set
CodeDirectory ... flags=0x20002(adhoc,linker-signed)
_CodeSignature dirs: 0
```

| version | DMG signed | DMG notarized | DMG stapled | app in DMG stapled | tarball payload signed | tarball payload CDHash |
|---|---|---|---|---|---|---|
| v0.12.0 | not tested | not tested | not tested | not tested | **no (adhoc)** | `681b06e22f9704d037e861b6d00b2ed8408e9154` |
| v0.12.1 | not tested | not tested | not tested | not tested | **no (adhoc)** | `5dc93338d5a83f6dae5db8dd58ad5165905fb408` |
| v0.12.2 | not tested | not tested | not tested | not tested | **no (adhoc)** | `b562132ae22a75df8cd22f6392d4cbc1c14875d0` |
| v0.12.3 | not tested | not tested | not tested | not tested | **no (adhoc)** | `f98e959b8db50243d987d9066ce49dfa9a2e1497` |
| v0.12.4 | not tested | not tested | not tested | not tested | **no (adhoc)** | `efebefcaeb5be00d6e2371d5640e08dbe8411dd5` |
| v0.12.5 | not tested | not tested | not tested | not tested | **no (adhoc)** | `054b546492d6138860ac9c6cc5c730e2d16b9705` |
| v0.13.0 | not tested | not tested | not tested | not tested | **no (adhoc)** | `eb6591b188fc8512dbb6f60b885df3a9cdabf9e1` |
| v0.13.1 | not tested | not tested | not tested | not tested | **no (adhoc)** | `847a5d3315d2824354a4f26f97b51123f58fdd7c` |
| v0.14.0 | not tested | not tested | not tested | not tested | **no (adhoc)** | `900eacb6b88884531f637189f23b1e2d353222ad` |
| v0.15.0 | yes (F5D4TE3RG4) | yes | yes | **no** | **no (adhoc)** | `68db3c7653151e97928eb3bb67ec4f5151f6de94` |
| v0.16.0 | yes (F5D4TE3RG4) | yes | yes | **no** | **no (adhoc)** | `fe41382ddc1816931e7644b3e81772de455d26a9` |
| v0.17.0 | yes (F5D4TE3RG4) | yes | yes | **no** | **no (adhoc)** | `7e0845cf371e156533e4d0ea6b68e2936da04492` |
| v0.18.0 | yes (F5D4TE3RG4) | yes | yes | **no** | **no (adhoc)** | `2392ea42cc4ff491971e04a086d07e047c7e90fd` |
| v0.18.1 | yes (F5D4TE3RG4) | yes | yes | **no** | **no (adhoc)** | `58ec7bbdba8626985768a72a7ae49c12fae5337b` |
| v0.18.2 | yes (F5D4TE3RG4) | yes | yes | **no** | **no (adhoc)** | `535ab45957102f2447a9843e61dbeb781037c212` |
| v0.18.3 | yes (F5D4TE3RG4) | yes | yes | **no** | **no (adhoc)** | `2a79916396bce9183ab75f8047c2ca505a2a3888` |
| v0.18.4 | yes (F5D4TE3RG4) | yes | yes | **no** | **no (adhoc)** | `21846fa566262ee1d946ebf0f61cde8e12cf641b` |
| v0.18.5 | yes (F5D4TE3RG4) | yes | yes | **no** | **no (adhoc)** | `f12bf11e0970d3e156fee334f728c680f2efce51` |
| v0.19.0 | yes (F5D4TE3RG4) | yes | yes | **no** | **no (adhoc)** | `d3974ae45fa91b7a9df11b9b5e52eb988532a7cb` |

"not tested" in the DMG columns means only that I limited the DMG mount-and-assess
sweep to the v0.15.0 through v0.19.0 range the brief named. The tarball column is
complete: all 19 verified individually.

The DMG side, for the ten tested (representative output, v0.19.0):

```
$ codesign -dvv Lucidos_0.19.0_aarch64.dmg
Identifier=Lucidos_0.19.0_aarch64
Authority=Developer ID Application: ... (F5D4TE3RG4)
TeamIdentifier=F5D4TE3RG4

$ xcrun stapler validate Lucidos_0.19.0_aarch64.dmg
The validate action worked!

$ spctl -a -t open --context context:primary-signature -vv Lucidos_0.19.0_aarch64.dmg
accepted
source=Notarized Developer ID
```

The app inside that DMG, for contrast with the tarball:

```
Identifier=com.lucidos.app
CDHash=38458e70e5f0ecafe9d42b0a8fa4f090084e1caf
TeamIdentifier=F5D4TE3RG4
CodeDirectory v=20500 ... flags=0x10000(runtime)
designated => identifier "com.lucidos.app" and anchor apple generic
              and certificate leaf[subject.OU] = F5D4TE3RG4
_CodeSignature dirs: 1
```

And the same version's tarball payload:

```
Identifier=lucidos_app-e0ed54f1c3141357
CDHash=d3974ae45fa91b7a9df11b9b5e52eb988532a7cb
Signature=adhoc
TeamIdentifier=not set
CodeDirectory v=20400 ... flags=0x20002(adhoc,linker-signed)
designated => cdhash H"d3974ae45fa91b7a9df11b9b5e52eb988532a7cb"
_CodeSignature dirs: 0
```

The installed app on this machine, read-only:

```
$ codesign -dvvv /Applications/Lucidos.app
Identifier=lucidos_app-e0ed54f1c3141357
CDHash=d3974ae45fa91b7a9df11b9b5e52eb988532a7cb
Signature=adhoc
--- _CodeSignature dirs: 0
--- version: 0.19.0
--- DR: # designated => cdhash H"d3974ae45fa91b7a9df11b9b5e52eb988532a7cb"
```

Byte-identical code identity to the v0.19.0 tarball payload, which is proof (not
inference) that the running install arrived through the updater and not through
the DMG.

### The identifier discrepancy, explained

`com.lucidos.app` versus `lucidos_app-e0ed54f1c3141357` is not two different
bundles. The `.app`'s main executable is `Contents/MacOS/lucidos-app`, the
`lucidos-app` crate's binary. rustc emits it **linker-signed** with an ad-hoc
identifier derived from the crate name plus its metadata hash, which is what
`lucidos_app-<hash>` is. When `sign_app_bundle` signs the outer bundle at
`:1534`, codesign takes the identifier from `CFBundleIdentifier` instead, giving
`com.lucidos.app`. So the identifier tells you directly which path an app came
from: `com.lucidos.app` means DMG, `lucidos_app-<hash>` means updater. The hash
also moves with the crate's metadata: I observed `lucidos_app-26fcd0a280087598`
(v0.12.0), `-80498cd11bd56530` (v0.12.3), `-9c58685bdff9693a` (v0.14.0),
`-37c4048005011e1e` (v0.16.0 to v0.18.5), `-e0ed54f1c3141357` (v0.19.0).

### Beyond "unsigned": the seal is broken

```
$ codesign --verify --verbose=2 Lucidos.app       # extracted from the tarball
Lucidos.app: code has no resources but signature indicates they must be present

$ spctl -a -vvv -t exec Lucidos.app
Lucidos.app: code has no resources but signature indicates they must be present
```

The bundle does not merely lack a Developer ID; its signature does not validate
at all. Nothing seals `Contents/Info.plist` or anything under
`Contents/Resources/`, which is where the engine, the gateway, the `lucidos`
CLI, the frontend, the SDK and the relocatable Postgres tree live. The
equivalent DMG bundle has 134 Mach-O files under one `_CodeSignature`.

### User-visible consequence

Verified:

1. Every user who has ever taken an in-app update is running an ad-hoc bundle
   whose designated requirement is a bare CDHash that changes on every release.
2. Hardened runtime is lost. The DMG app carries `flags=0x10000(runtime)`; the
   updater payload carries `flags=0x20002(adhoc,linker-signed)` and no runtime
   bit. So every update silently downgrades the process's security posture
   (library validation, unsigned-executable-memory restrictions, and the rest of
   the hardened-runtime set are all off).
3. Gatekeeper would reject the bundle if it were ever assessed (`spctl` output
   above).

Inferred, and I did not test these:

4. macOS TCC keys grants to the responsible process's code identity. A bare-CDHash
   designated requirement changes on every build, so I expect every permission
   grant (screen recording, accessibility, files-and-folders, and so on) to be
   discarded and re-prompted at each update. This is exactly the failure mode
   `scripts/lib/codesign.sh` was written to eliminate for the *dev* engine, and
   its header describes it precisely. I did not read the TCC database (it is
   SIP-protected) and the brief forbade `tccutil`, so this is inference from the
   documented TCC model plus the observed DR, not an observation.
5. The bundle is only launchable at all because the updater writes it without a
   `com.apple.quarantine` xattr, so Gatekeeper never assesses it. Anything that
   *does* set quarantine on it later (a zip round-trip, some backup or
   endpoint-security tooling) would turn it into an app the user cannot open.

### Recommended fix

Re-pack `Lucidos.app.tar.gz` from `$APP_PATH` **after** `sign_app_bundle`
returns and **before** `stage_release_artifacts`, then re-sign it with the Tauri
updater key so the `.sig` matches the new bytes. The re-sign is not optional:
the current `.sig` is over the unsigned tarball, and V1 in the "verified
correct" section shows those signatures do currently verify, so a re-pack
without a re-sign would turn a silent problem into a loud one (every update
would fail signature verification).

The signing key is already loaded into `TAURI_SIGNING_PRIVATE_KEY` by
`resolve_tauri_signing_private_key` at `:1346`, and
`cargo tauri signer sign --private-key-path ...` is already exercised by
`_release_signing_test_sign` in `scripts/lib/release_signing.sh:55-91`, so the
mechanism exists. Do not hand-roll minisign.

I would additionally consider notarizing and stapling the `.app` in its own
submission before the DMG is built, which fixes F1 and F5 together at the cost of
a second notary round trip. That is Apple's documented ordering. It is a bigger
change than the sibling's scope and I would not couple them.

---

## F2 (high). Nothing ever checks the shipped payload's signature

### What is wrong

I searched the whole repo for any place that runs `codesign` against the updater
tarball or its contents:

```
$ grep -rn "codesign" scripts/ .github/ | grep -i "tar.gz"
(no output)
```

The pipeline's post-build assertions are:

- `:1483-1488` version-stamp guard: checks the DMG **filename** carries
  `EFFECTIVE_VERSION`. Says nothing about signatures.
- `:1540-1551` `sign_app_bundle`'s own verify: runs `codesign --verify --deep
  --strict` on `$APP_PATH`, the standalone app under
  `target/release/bundle/macos`. That app is **not** shipped. It is the source
  for `refresh_dmg_payload` and for `--emit-tarball`, and it is discarded.
- `:502-505` `stage_release_artifacts`: asserts the `.app.tar.gz` and `.sig`
  **exist**. Nothing about their contents.
- `release_staging_verify`: sha256 of each staged artifact against the manifest
  the same run wrote. Self-consistency only, so it cannot detect a
  consistently-wrong artifact.
- `.github/workflows/install-smoke.yml:353-416` `dmg-verify`: the only
  clean-machine Gatekeeper gate. It downloads `--pattern '*.dmg'` and asserts
  spctl, stapler and `codesign --verify --deep --strict` on the mounted app. It
  never fetches `Lucidos.app.tar.gz`.

So the artifact that reaches almost every user is the one artifact with no
verification at any stage.

### User-visible consequence

Nineteen releases. The bug was structurally undetectable by the pipeline, so
fixing F1's mechanism without adding a check leaves the whole class open: the
next refactor of the build ordering can silently reintroduce it.

### Recommended fix

Add a post-stage assertion in `stage_release_artifacts` that extracts the staged
tarball to a temp dir and refuses on anything other than a Developer ID
signature with the expected team, at minimum:

```
codesign --verify --deep --strict "$tmp/Lucidos.app"
codesign -dvv "$tmp/Lucidos.app" 2>&1 | grep -q "TeamIdentifier=$APPLE_TEAM_ID"
```

Two properties matter. It must run in **release-build** mode, before staging
completes, so a bad payload never reaches a Release. And it must **fail closed**:
an extraction that produces no `.app`, or a `codesign` that cannot run, refuses
rather than passing. `build_dmg_test.sh` can drive it with a fixture tarball.

Separately, extend `dmg-verify` to download `Lucidos.app.tar.gz`, extract it, and
run the same three checks it already runs on the DMG's app. That gives the
updater payload a clean-machine gate on the rc prerelease, which is where the
DMG already has one.

---

## F3 (high). The resume path can ship a DMG and an updater payload from different builds

### What is wrong

`run_notarize_resume` at `:1057-1129` reconstructs the release from a handle. It
is careful about the DMG and careless about everything else:

```bash
DMG_PATH="$(release_notarize_field "$NOTARIZE_STATE_FILE" dmg_path)"      # :1079
BUNDLE_DIR="$REPO_ROOT/target/release/bundle"
case "$DMG_PATH" in
    "$BUNDLE_DIR/dmg/"*) ;;
    *) die "the resume handle records a DMG outside this tree's bundle dir ..." ;;
esac                                                                       # :1086-1089
APP_PATH="$(/usr/bin/find "$BUNDLE_DIR/macos" -name '*.app' ... | head -1)" # :1090
```

The comment at `:1082-1085` states the intent exactly: *"Staging pairs the
recorded DMG with the `.app.tar.gz` + `.sig` found under BUNDLE_DIR. Those must
be the same build's artifacts, so refuse a handle whose DMG lives somewhere
else."*

A path test does not establish that. `stage_release_artifacts` at `:502-503`
then does:

```bash
app_tarball="$(/usr/bin/find "$BUNDLE_DIR/macos" -name '*.app.tar.gz' ... | head -1)"
app_sig="$(/usr/bin/find "$BUNDLE_DIR/macos" -name '*.app.tar.gz.sig' ... | head -1)"
```

These are whatever is on disk **now**. There is no checksum, no build id, no
timestamp comparison against the DMG.

The path is not hypothetical, because the recovery branch of
`assert_dmg_is_the_submitted_bytes` at `:893-906` actively creates it:

```bash
if [ ! -f "$DMG_PATH" ] || [ "$(release_staging_sha256 "$DMG_PATH")" != "$expected" ]; then
    pinned="${NOTARIZE_PINNED_DMG:-}"
    ...
    cp -f "$pinned" "$DMG_PATH"    # restore build N's DMG over build N+1's
fi
```

So: build N submits and pins. A concurrent or subsequent build N+1 overwrites
`target/release/bundle/dmg/Lucidos_<ver>_aarch64.dmg` **and**
`target/release/bundle/macos/Lucidos.app.tar.gz`. The resume for N notices the
DMG changed, restores N's DMG from the pin, and then stages it alongside N+1's
tarball and `.sig`. The manifest records both, `release_staging_verify` finds
them self-consistent, and the Release ships a DMG from one build and an updater
payload from another.

The 2026-07-28 incident that motivated the pin was exactly three concurrent
pollers on one tree, so the precondition is a state the repo has already been in.

### User-visible consequence

A published release whose DMG install and whose in-app update land on different
code. Version numbers agree (both were built from the same `RELEASE`), the
manifest verifies, `dmg-verify` passes because it only sees the DMG, and nothing
in the pipeline can tell. The divergence is silent and unbounded: the two builds
could straddle any source change made between them, because the resume gate only
pins `source_commit` for the DMG, and a rebuild at the same commit with different
`target/` state produces different bytes.

### Recommended fix

Extend the notarize resume handle to record the sha256 of the `.app.tar.gz` and
the `.sig` at submit time, alongside `dmg_sha256`, and gate the resume on all
three. `release_notarize_write_state` in `scripts/lib/release_notarize.sh`
already carries the shape; adding two fields is mechanical, and
`release_notarize_test.sh` already tests round-tripping and checksum refusal for
the DMG field.

Pin them the same way, too. `notarize_pin_submitted_dmg` at `:960-980` copies the
DMG into `.lucidos/notarize-submissions/<version>/<sha12>/`; the updater trio
belongs in the same directory, for the same reason and with the same
`cp -c`-then-`cp` fallback. Then the recovery branch can restore a **consistent
set** instead of half of one.

Failing that, the minimum honest fix is to make the recovery branch refuse rather
than recover when the updater artifacts have changed since submit. Recovering
half a build is worse than rebuilding.

---

## F4 (medium). `refresh_dmg_payload`'s intermediates can be picked up as the release DMG

### What is wrong

`refresh_dmg_payload` at `:1577-1601` writes two intermediates next to the real
DMG, both matching `*.dmg`:

```bash
local rw="${dmg%.dmg}.rw.dmg"     # :1580   uncompressed read-write image
...
local out="${dmg%.dmg}.zlib.dmg"  # :1596   recompressed output, before the mv
```

A run killed between `hdiutil convert ... -o "$rw"` and the trailing
`rm -f "$rw"` leaves `Lucidos_<ver>_aarch64.rw.dmg` behind permanently. A kill
during the UDZO convert leaves `Lucidos_<ver>_aarch64.zlib.dmg`. Both survive
until the next successful refresh.

Discovery on the next build is:

```bash
DMG_PATH="$(/usr/bin/find "$BUNDLE_DIR/dmg" -name '*.dmg' 2>/dev/null | head -1 || true)"   # :1473
```

No `-maxdepth`, no exclusion, and `find | head -1` returns directory order, not
newest or sorted. The version-stamp guard at `:1483-1488` cannot help: it matches
`*"_${EFFECTIVE_VERSION}_"*`, and `Lucidos_0.19.0_aarch64.rw.dmg` contains
`_0.19.0_`.

The codebase already knows this. `notarize_adopt_submission` at `:1033-1034`
excludes them explicitly and says why:

```bash
# Exclude refresh_dmg_payload's intermediates: a run killed mid-refresh can
# leave a .rw.dmg / .zlib.dmg behind, and adopting one of those would record a
# checksum for bytes Apple never saw.
dmg="$(/usr/bin/find "$dmg_dir" -maxdepth 1 -name '*.dmg' \
    ! -name '*.rw.dmg' ! -name '*.zlib.dmg' ...)"
```

and `build_dmg_test.sh:364-385` tests exactly that, for the adopt path only. The
main discovery path has neither the exclusion nor the "exactly one candidate"
refusal that accompanies it.

### User-visible consequence

A release could ship `Lucidos_0.19.0_aarch64.rw.dmg` (an uncompressed UDRW image,
roughly 3x the size, named wrongly) as the browser download, having signed,
notarized and stapled it. The landing page's Download-for-Mac link would point at
it. It would probably still mount and install, which is what makes it a quiet
failure rather than a loud one. `--emit-tarball` and the notarize pin would all
key off the same wrong artifact.

Not observed in any published release: all ten DMGs I downloaded are correctly
named and UDZO-compressed. This is a latent hazard, not an active one.

### Recommended fix

Give `:1473` the same exclusion and the same arity check the adopt path has, and
add the fixture case to `build_dmg_test.sh` next to the existing one. While
there, `refresh_dmg_payload` should clean its own intermediates on failure with a
trap rather than only on the success path.

---

## F5 (medium). The `.app` inside every DMG has no stapled ticket

### What is wrong

Ordering in `build-dmg.sh`:

1. `:1563` `sign_app_bundle "$APP_PATH"`
2. `:1603` `refresh_dmg_payload "$DMG_PATH" "$APP_PATH"` injects the signed app
   into the DMG
3. `:1614` `sign_dmg "$DMG_PATH"`
4. `:1630-1633` notarize the DMG, then `staple_notarized_artifacts`

`staple_notarized_artifacts` at `:925-934` staples `$DMG_PATH` and then
`$APP_PATH`. But `$APP_PATH` is the standalone app under
`target/release/bundle/macos`, and the copy inside the DMG was injected at step 2,
before any ticket existed. The stapled standalone app is never shipped: staging at
`:521-523` copies only the DMG, the tarball and the `.sig`.

The comment at `:932` says "(no .app on disk to staple, the DMG carries the ticket
that matters)". That is true for the DMG as a download, and not true for the app
the user ends up running.

### Evidence

All ten DMGs tested, identical result:

```
$ hdiutil attach Lucidos_0.19.0_aarch64.dmg -readonly ...
$ xcrun stapler validate "$MNT/Lucidos.app"
Lucidos.app does not have a ticket stapled to it.

$ spctl -a -vvv -t exec "$MNT/Lucidos.app"
accepted
source=Notarized Developer ID
```

The `accepted` verdict came from an **online** notarization lookup: the app's
CDHash is covered by the DMG's notary submission, so Apple's service vouches for
it even without a local ticket.

### User-visible consequence

A user who mounts the DMG and drags `Lucidos.app` to `/Applications` gets an app
with no stapled ticket. Its quarantine xattr is set, so first launch triggers a
Gatekeeper assessment that must reach Apple.

**Inference, not tested:** on a machine that is offline (or behind a firewall
that blocks the notarization CDN) at that first launch, I expect that assessment
to fail or to degrade. I did not test the offline first-launch behaviour, and
current macOS behaviour on an unreachable ticket service is something I am not
confident enough about to assert. What is verified is only the absence of the
ticket, which is the whole reason stapling exists.

### Recommended fix

The clean fix is the reordering already mentioned under F1: notarize and staple
the `.app` in its own submission, then build the DMG around the stapled app, then
sign, notarize and staple the DMG. Two notary round trips, and it fixes F1 and F5
in one shape.

The cheap partial fix is not available: you cannot staple the app inside a DMG
that has already been notarized, because re-writing the DMG changes its own
CDHash and invalidates its ticket.

---

## F6 (medium). The engine's signing identity is per-build in the updater payload

### What is wrong

`scripts/lib/codesign.sh` exists for exactly one reason, stated in its header:

> a `cargo build` binary is `adhoc, linker-signed`. Its CDHash changes on every
> rebuild, and macOS TCC keys permission grants by the responsible process's
> code identity, so after each rebuild TCC discards the prior grant and
> re-prompts.

Its answer is `--identifier lucidos-engine` plus a stable certificate leaf, which
yields a rebuild-stable designated requirement. The **release** path achieves the
same thing by a different route: `sign_app_bundle`'s loop at `:1526-1530` signs
each Mach-O with no `--identifier`, so codesign derives the identifier from the
file's basename. Verified across all ten DMGs:

```
lucidos-engine   Identifier=lucidos-engine    TeamIdentifier=F5D4TE3RG4
lucidos-gateway  Identifier=lucidos-gateway
lucidos          Identifier=lucidos

designated => identifier "lucidos-engine" and anchor apple generic
              and certificate leaf[subject.OU] = F5D4TE3RG4
```

Stable across every release, no CDHash in the requirement. Correct.

The updater payload has none of it:

| release | engine identifier in the tarball |
|---|---|
| v0.15.0 to v0.18.0 | `lucidos_engine-ea9f4656ea7c4fdf` |
| v0.18.1 to v0.18.5 | `lucidos_engine-0ee83148e4bbd898` |
| v0.19.0 | `lucidos_engine-dff7bfbe5d7dbebc` |

all with `Signature=adhoc`, `TeamIdentifier=not set`, and a per-build CDHash.
The same applies to `lucidos-gateway`, the `lucidos` CLI, and the bundled
Postgres binaries (`postgres-555549444873e653c27538b0b32964ea72213353`, whose
identifier is at least stable across releases because those binaries are fetched
pre-built rather than compiled).

### User-visible consequence

Two identities exist for the same logical engine process. A DMG install runs
`lucidos-engine` under team F5D4TE3RG4 with a stable DR; an updated install runs
`lucidos_engine-<hash>` ad-hoc with a CDHash DR. **Inferred:** a user who
installs from the DMG, grants a TCC permission, then takes any update, loses the
grant and is re-prompted, and is re-prompted again on every subsequent update
because the CDHash moves each time. The `curl | sh` install produces a third
identity (see F11), also ad-hoc, also per-build.

The launchd job at `gui/<uid>/com.lucidos.engine` is **not** affected by this in
itself. Its label and its `ProgramArguments` path
(`crates/lucidos-app/src/desktop.rs:642-678`) are stable, `restart_service`
(`:778-782`) is a bare `launchctl kickstart -k` on that label, and the plist is
only rewritten when its text changes. That part of the update path is sound. What
changes across an update is only the code identity of the binary launchd starts.

### Recommended fix

Covered by F1: a tarball re-packed from the signed app inherits the stable
identifiers automatically, because they are a property of `sign_app_bundle`'s
output. No separate change needed.

Worth a comment at `sign_app_bundle`'s loop noting that the basename-derived
identifier is load-bearing for TCC and must not be replaced by an explicit
`--identifier` unless that identifier matches the basename. Right now the
stability is a happy accident of codesign's default, and nothing records that it
matters.

---

## F7 (medium). Three documents state the opposite of the truth

`docs/adr/0027-a-release-does-not-wait-on-apple.md:28`, in the decision table
that justifies deferred notarization:

> | `.app.tar.gz` + `.sig` + `latest.json` | Tauri in-app updater | **no**, the
> updater's own download isn't quarantined; integrity is our minisign key;
> **launch passes on the Developer ID signature** |

`docs/desktop-app.md:274` carries the same row verbatim. `scripts/build-dmg.sh:96`
carries the same claim in the file's own header:

> the updater's integrity comes from our own minisign key and the bundle launches
> on its Developer ID signature.

There is no Developer ID signature on that bundle and there never has been. The
launch succeeds for a different reason: the updater writes the bundle without a
quarantine xattr, so Gatekeeper is never invoked at all.

### User-visible consequence

None directly. But this is the mechanism by which the bug stayed invisible for 19
releases: the one written statement about the updater payload's signing state
asserted it was fine, in an ADR, next to a decision that depended on it.

The ADR's **decision** survives: deferring notarization really does only affect
the browser-download population, because the updater payload is genuinely never
quarantined. Only the stated reason is wrong, and it was wrong in a way that made
it look like somebody had checked.

### Recommended fix

Correct the third column in both tables to name the real reason (no quarantine
xattr, therefore no Gatekeeper assessment) and drop the Developer ID claim, in
the same change that fixes F1. Once F1 is fixed the claim becomes true, but it
should still not be the stated reason, because it is not what makes the launch
work.

---

## F8 (medium). `latest.json` is published before the payload it points at

### What is wrong

`upload_staged_assets` at `:630-632` attaches four assets in one `gh` invocation:

```bash
gh release upload "$UPLOAD_TAG" --repo "$REPO_SLUG" --clobber \
    "$dmg" "$app_tarball" "$app_sig" "$latest_json"
```

`gh release upload` uploads concurrently, so the small files finish first. The
updater endpoint is `https://github.com/lucidos-dev/lucidos/releases/latest/download/latest.json`
(`crates/lucidos-app/tauri.conf.json`), and GitHub marks a release as "latest" the
moment it is published, which `release-to-lucidos.sh` does **before** this upload
runs. So there is a window in which `latest.json` is fully readable and advertises
a `Lucidos.app.tar.gz` that GitHub still answers with a 404.

### Evidence

Per-asset `created_at` (upload started) and `updated_at` (upload finished), from
the GitHub API:

```
=== v0.19.0 (release published: 2026-08-02T11:16:03Z)
latest.json          created=2026-08-02T11:16:04Z   updated=2026-08-02T11:16:05Z
Lucidos.app.tar.gz   created=2026-08-02T11:16:04Z   updated=2026-08-02T11:16:15Z
Lucidos.app.tar.gz.sig created=2026-08-02T11:16:04Z updated=2026-08-02T11:16:05Z
Lucidos_0.19.0_aarch64.dmg created=2026-08-02T11:16:04Z updated=2026-08-02T11:16:15Z
```

A 10-second window on v0.19.0. 65 seconds on v0.15.0. And one much larger one:

```
=== v0.16.0 (release published: 2026-07-29T12:24:13Z)
latest.json          created=2026-07-29T20:30:09Z   updated=2026-07-29T20:30:10Z
Lucidos.app.tar.gz   created=2026-07-29T20:30:09Z   updated=2026-07-29T20:30:21Z
Lucidos_0.16.0_aarch64.dmg created=2026-07-29T20:30:09Z updated=2026-07-29T20:30:20Z
lucidos-0.16.0-aarch64-apple-darwin.tar.gz created=2026-07-29T12:39:07Z ...
```

The `created_at` values show the whole DMG trio was **first** uploaded at
20:30:09, eight hours and six minutes after the Release was published. For that
entire window v0.16.0 was GitHub's latest release with no `latest.json` asset at
all, so `/releases/latest/download/latest.json` 404'd and every packaged client's
update check failed. The headless tarballs went up on schedule at 12:39, so this
was a late `--release-attach`, not a re-upload.

### User-visible consequence

An update check landing in the window fails. Tauri's `check()` surfaces that as
an error string through `check_app_update`; the app-update store treats a failed
check as "no update", so the user simply sees nothing and retries on the next
poll. Not damaging, but it means a release's update reachability is not a
property anything asserts, and the v0.16.0 case shows the window can be hours,
not seconds.

`--clobber` is also worth naming here: a re-upload replaces assets in place with
the same non-atomicity, so a corrective re-attach reopens the window.

### Recommended fix

Upload `latest.json` **last**, in its own `gh release upload` call after the
first has returned successfully. That makes the ordering an explicit property of
the script instead of an accident of which file is smallest, and it shrinks the
window to zero for the common case. It costs one extra API call.

The eight-hour case needs the ordering to be respected across *phases*, not just
within one call, so `--attach-notarized` and any manual re-attach should follow
the same rule.

---

## F9 (low). A failed bundle swap leaves no app and destroys the backup

### What is wrong

`tauri-plugin-updater` 2.10.1, `src/updater.rs:1217-1307`, the macOS
`install_inner`:

```rust
let tmp_backup_dir = tempfile::Builder::new().prefix("tauri_current_app").tempdir()?;
...
let move_result = std::fs::rename(&self.extract_path, tmp_backup_dir.path().join("current_app"));
...
} else {
    if self.extract_path.exists() { std::fs::remove_dir_all(&self.extract_path)?; }
    std::fs::rename(tmp_extract_dir.path(), &self.extract_path)?;   // <- no rollback on Err
}
```

The current app is moved into a `TempDir`. If the final rename onto
`/Applications/Lucidos.app` fails, the function returns `Err`, the `TempDir` is
dropped, and the backup is deleted with it. There is no restore branch. The name
`tmp_backup_dir` implies a rollback the code never performs.

This is upstream, not Lucidos code, but Lucidos ships it and the blast radius is
larger here than for a typical Tauri app: the launchd job
`gui/<uid>/com.lucidos.engine` has `KeepAlive=true` and points at
`/Applications/Lucidos.app/Contents/MacOS/lucidos-app`
(`crates/lucidos-app/src/desktop.rs:642-678`). With the bundle gone, that job
crash-loops on a 10-second `ThrottleInterval`, so the gateway, every workspace
engine and the embedded Postgres are down, not just the GUI.

`install_app_update_and_restart` handles the error correctly on its own terms
(`crates/lucidos-app/src/updater.rs:494-497` emits `failed` and returns), and the
already-running service keeps its deleted inode alive until the next restart. So
the user sees a failed update now and a dead stack later.

### Recommended fix

Not fixable in this repo without forking the plugin. What **is** available here
is detection: after `update.install(bytes)` returns `Ok`, before
`restart_service()`, assert that `/Applications/Lucidos.app` exists and that its
`Contents/MacOS/lucidos-app` is executable, and surface a distinct failure
message if not. That turns a silent later-boot failure into an immediate one the
user can act on. It is three lines in `updater.rs` around `:494`.

Upstream, the fix is a restore branch on the final rename. Worth filing.

---

## F10 (low). The `latest.json` platform key comes from the upload host

`:592-597`:

```bash
case "$(uname -m)" in
    arm64|aarch64) platform_key="darwin-aarch64" ;;
    x86_64)        platform_key="darwin-x86_64" ;;
    *) die "unsupported arch for latest.json: $(uname -m)" ;;
esac
```

The key describes the machine running the upload, not the artifact. All ten
`latest.json` files I checked carry exactly one key, `darwin-aarch64`, which
happens to be correct because the DMG is Apple-Silicon-only. But an
`--release-attach` run on a different host, or a future universal or Intel build,
would mislabel the payload with no guard to catch it, and the mislabelling is
silent: an updater whose target key is absent from `platforms` reports no update
rather than an error.

### Recommended fix

Derive the key from the artifact. The staged `.app` is right there; `lipo -archs
"$app/Contents/MacOS/lucidos-app"` or `file` on the same binary gives the real
answer. Since `--release-attach` deliberately has no `.app` on disk, the honest
alternative is to record the platform key **in the staging manifest** at build
time, next to `version` and `source_commit`, and have `upload_staged_assets` read
it from there. That also makes it verifiable by `release_staging_verify`.

---

## F11 (low). The `curl | sh` front door also lays down ad-hoc Mach-O

The brief asks whether the front door can produce the same outcome. It can, in
the sense that matters for code identity, and it cannot collide with the DMG
install.

**What it installs.** `install.sh` downloads
`lucidos-<version>-<triple>.tar.gz` into `$LUCIDOS_PREFIX/runtime/<stem>/`
(default `$HOME/.lucidos/runtime/`) and registers the bundled **gateway** as a
launchd agent `com.lucidos.gateway.<slug>`. It never writes to `/Applications`
and never installs an `.app`, so there is no filesystem collision with the DMG
install. The only shared resource is the port: the default 5252 is the packaged
app's gateway port, and a bare run on a new instance auto-picks the first free
port upward, so the two coexist. A pinned `--port 5252` against a running app
fails closed.

**Is it signed.** No. Verified:

```
=== lucidos-0.19.0-aarch64-apple-darwin.tar.gz
  lucidos-engine:  Identifier=lucidos_engine-dff7bfbe5d7dbebc  Signature=adhoc  TeamIdentifier=not set
  lucidos-gateway: Identifier=lucidos_gateway-ebabd94f1c4923ab Signature=adhoc  TeamIdentifier=not set
  lucidos:         Identifier=lucidos-98c1c747a3190669         Signature=adhoc  TeamIdentifier=not set

=== lucidos-0.15.0-aarch64-apple-darwin.tar.gz
  lucidos-engine:  Identifier=lucidos_engine-f5e0c63020854de5  Signature=adhoc
  lucidos-gateway: Identifier=lucidos_gateway-16bd8c61cc2cc7c6 Signature=adhoc
  lucidos:         Identifier=lucidos-79066e61b36e07ac         Signature=adhoc
```

This is **documented and intentional**. `.claude/rules/build-release.md` states
that the macOS headless tarballs on a Release are the unsigned CI ones from
`build-headless.sh`, that `release.sh` never passes `--emit-tarball`, and that
the signed local tarball is therefore a capability that is never attached to
anything. I confirmed the second half: `grep -n "emit-tarball" scripts/release.sh
scripts/release-to-lucidos.sh` returns nothing.

The checksum sidecar is correct:

```
$ shasum -a 256 -c lucidos-0.19.0-aarch64-apple-darwin.tar.gz.sha256
lucidos-0.19.0-aarch64-apple-darwin.tar.gz: OK
```

**Assessment.** The stated justification (a `curl`-fetched file carries no
quarantine xattr, so Gatekeeper never assesses it) is sound and is the same
reasoning as F1's "why does it launch at all". What the documentation does not
say is that this gives the same engine **three** distinct TCC identities
depending on install path, all of them different, two of them per-build. If F1's
fix makes the updater path stable, the front door becomes the only remaining
unstable one, and the asymmetry is then worth an explicit decision rather than a
consequence.

### Recommended fix

No change required for correctness. Once F1 lands, revisit whether wiring
`--emit-tarball` into the release flow for the macOS triples is now worth it, and
record the decision, because the reason for not doing it ("there is no signed
macOS tarball for CI to clobber") is a statement about the current state rather
than a principle.

---

## F12 (low). The updater command is reachable from a plain-HTTP loopback origin

`crates/lucidos-app/src/desktop.rs:306-360` grants `updater:default`, which
includes `plugin:updater|download_and_install`, to `http://localhost:<port>`, the
gateway origin the packaged window is navigated to.

The code already reasons about the obvious hazard and closes it: the URL pattern
carries the **resolved** port rather than `localhost:*` (the comment at `:342-344`
says a wildcard "would hand IPC to any other local HTTP server the window could
be navigated to"), the capability is scoped to `webviews` rather than `windows` so
the `url-preview-*` webviews showing third-party sites do not inherit it, and
`local(false)` keeps it off the bundled-asset origin.

The residual is narrow: the origin is plain HTTP on loopback with no
authentication, so a local process that binds the port **before** the gateway
does would receive the window's IPC. `launch` navigates only after the gateway
reports healthy, and the gateway holds the port for the life of the service, so
the window is very unlikely to reach a squatter. I am recording it because "a
local process can reach `download_and_install`" is a meaningfully different
statement from "the app can update itself", and neither the code comment nor the
docs say it.

### Recommended fix

None proposed. Worth one sentence in the `gateway_capability` doc comment noting
that `updater:default` is in the grant set and what that implies, so the next
person widening `GATEWAY_PERMISSIONS` knows what is already there.

---

## Verified correct

Negative results, recorded so the doc says what was checked and found sound.

**V1. Every updater Ed25519 signature verifies against its shipped tarball.**
This was the loud-failure hypothesis and it is not happening. I decoded the
base64-wrapped minisign `.sig` for all ten releases in the v0.15.0 to v0.19.0
range and verified each against the tarball bytes with the public key baked into
`crates/lucidos-app/tauri.conf.json`, using openssl for the Ed25519 check:

```
=== v0.19.0
  pubkey alg=b'Ed' keyid=5e4b5719bdb941f4
  sig    alg=b'ED' keyid=5e4b5719bdb941f4
  trusted comment: timestamp:1785668280	file:Lucidos.app.tar.gz
  mode: prehashed blake2b-512 over 70032831 bytes
  RESULT: SIGNATURE VALID  (Signature Verified Successfully)
  global (trusted-comment) sig: VALID
```

Ten for ten, including the global (trusted-comment) signature. Key id
`5e4b5719bdb941f4` matches the configured pubkey in every case, so no release was
signed with a different or rotated key.

**V2. Every DMG is Developer ID signed, notarized and stapled.** Ten for ten:
`codesign -dvv` shows team F5D4TE3RG4 with a secure timestamp, `xcrun stapler
validate` reports "The validate action worked!", and `spctl -a -t open --context
context:primary-signature` reports `source=Notarized Developer ID`.

**V3. Every `latest.json` is internally consistent.** For all ten: the `version`
field equals the release version, the `platforms["darwin-aarch64"].signature`
field is byte-identical to the `Lucidos.app.tar.gz.sig` asset on the same
release, and the `url` points at the same tag the assets are attached to. No
release advertises another release's payload or another payload's signature.

**V4. Downgrade and replay are refused client-side.** `tauri-plugin-updater`
2.10.1 `src/updater.rs:530-532` uses `release.version > self.current_version`
when no custom comparator is set, and `crates/lucidos-app/src/lib.rs:1267`
registers the plugin with a bare `Builder::new().build()`, so no comparator is
set. A replayed older `latest.json` yields no update. A same-version replay also
yields no update.

**V5. The rc prereleases cannot poison the updater endpoint.**
`releases/latest/download/` resolves only to non-prerelease releases, and I
confirmed the current resolution:

```
$ curl -sI .../releases/latest/download/latest.json
302 https://github.com/lucidos-dev/lucidos/releases/download/v0.19.0/latest.json
```

`refresh_release_candidate_prerelease` (`scripts/release.sh:1165-1192`) attaches
only the DMG and the `.sig` to the `rc-<version>` prerelease. No `latest.json`,
no `.app.tar.gz`. There are no stale prereleases on the repo today.

**V6. The version stamped into the updater payload is correct.** The tarball
app's `Info.plist` carries `CFBundleShortVersionString = 0.19.0` and
`CFBundleIdentifier = com.lucidos.app`, matching the DMG app. The committed
`tauri.conf.json` pins `"version": "0.1.0"`, and `tauri_build_config_json`
(`:382-390`) overrides it from the `RELEASE` file for both artifacts in the same
build, so they cannot disagree. No release ships an app that would fail to
recognise itself as older than the next one.

**V7. The launchd service survives an update.** `LAUNCH_AGENT_LABEL` is the
constant `com.lucidos.engine`, the plist is written only when its text changes
(`install_or_update_plist`, `:682-694`), its `ProgramArguments` path is
`/Applications/Lucidos.app/Contents/MacOS/lucidos-app` which the updater replaces
in place, and `restart_service` (`:778-782`) is a bare `launchctl kickstart -k`
against the label. Nothing about the update path invalidates the job or the
plist. The ordering in `install_app_update_and_restart`
(`crates/lucidos-app/src/updater.rs:508-514`) is install, then service restart,
then `app.restart()`, which is correct: the service picks up the new binary
before the client re-execs.

**V8. The in-app updater's cancellation state machine is sound.** `AppUpdateRun`
(`crates/lucidos-app/src/updater.rs:211-319`) makes "a cancel must abort the
download and must never touch a started install" a state question rather than a
timing one, including the `Starting` window and the buffered-result race, and its
nine unit tests cover both. Nothing to add.

**V9. The build's own signing verification is thorough where it runs.**
`sign_app_bundle` (`:1493-1552`) discovers every Mach-O by `file` rather than
trusting `--deep`, signs inside-out via `sort -rz`, and then verifies the bundle,
each `BUNDLED_EXECUTABLES` entry, and two Postgres binaries. I counted 134 Mach-O
files in the v0.19.0 DMG app and one `_CodeSignature` directory sealing them.
The problem is not this function, it is that its output is not what ships to
updaters.

**V10. `assert_dmg_is_the_submitted_bytes` does what it claims for the DMG.** The
sha comparison at `:909-918` and the pin at `:960-980` (with the `cp -c`
clonefile rationale at `:946-954`) are correct, and
`release_staple_guard_test.sh` exercises them. F3 is not a criticism of this
guard; it is that the guard's coverage stops at the DMG.

**V11. Deferred notarization cannot silently publish an unstapled DMG.** The
`notarized` field lives in the staging manifest rather than in a flag,
`run_release_attach` (`:667-677`) refuses a non-notarized staging unless the
caller passes `--allow-pending-notarization`, and the refusal is on the manifest
rather than on the command line precisely so a separate later invocation cannot
route around it. The reasoning in the comment is correct.

**V12. The `curl | sh` front door does not collide with the DMG install.**
Different prefix (`$HOME/.lucidos` versus `/Applications`), different launchd
label (`com.lucidos.gateway.<slug>` versus `com.lucidos.engine`), and port
auto-selection that steps around a running app. See F11.

**V13. The updater tarball contains no path-traversal entries.** The plugin
strips the first path component of every entry and joins the rest onto a temp
dir, which would not defend against `..` components; the shipped tarballs contain
none (`tar tzf | grep -c '\.\.'` returns 0 for v0.19.0). This only matters behind
a compromised signing key, so it is noted rather than raised.

---

## Not checked, and why

1. **Offline first-launch Gatekeeper behaviour for an app copied from a stapled
   DMG (F5).** Testing it means disconnecting the network and launching an
   unstapled copy on a machine that has not already cached the assessment. I
   could not do that without either modifying `/Applications/Lucidos.app` or
   installing something, both of which the brief forbade. The absence of the
   ticket is verified; the consequence is inferred and labelled as such.

2. **Whether TCC grants are actually discarded across an update (F1, F6).** The
   TCC database is SIP-protected and `tccutil` was forbidden. The claim rests on
   the documented TCC model plus the observed change in designated requirement,
   and is labelled inference throughout. `scripts/lib/codesign.sh`'s header
   describes the same mechanism from first-hand experience with the dev engine,
   which is corroboration but not a measurement of the packaged app.

3. **DMG signature and staple state for v0.12.0 through v0.14.0.** The brief
   scoped the DMG sweep to v0.15.0 through v0.19.0 and I kept to it. The tarball
   sweep does cover all 19, which is where the finding is.

4. **Whether a Release has ever actually shipped a mismatched DMG and updater
   payload via the resume path (F3).** Establishing that would require the
   original build's `.app.tar.gz` bytes, which exist only in a local `target/`
   directory that has been overwritten many times since. F3 is a code-path
   finding, demonstrated from the source, not an observed incident. The
   preconditions (concurrent pollers on one tree) are known to have occurred on
   2026-07-28.

5. **The behaviour of `gh release upload` under a genuine partial failure
   (F8).** I did not attempt to induce one against the live repo, since the brief
   is read-only against GitHub. The window is established from asset timestamps;
   the partial-failure claim is a reading of the code (`|| die` after a
   four-argument upload) rather than an observation.

6. **Anything about the sibling session's in-progress fix.** I read this
   worktree's snapshot at `f83257a56` only. The "sibling fix covers it?" column
   reflects what the brief said that session is fixing, not an inspection of its
   work. Every "no" in that column should be re-checked against the actual diff
   before it is treated as an open item.

7. **Whether the site publisher's Download-for-Mac link ever pointed at a
   pending DMG.** That runs on the maintainer's machine off a workspace trigger
   chain and is outside this repo. `release_staging_is_notarized` gates it
   correctly from the manifest side, which is the half that lives here.
