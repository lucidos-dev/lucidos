# 0162: A device credential is matched against the store, not merely found in a cookie

- **Status**: Accepted
- **Date**: 2026-08-29

## Context

ADR 0132 split the credential cookie per gateway, `lucidos_device_<id>`, where
the id digests the gateway's data dir. It kept the pre-split name
`lucidos_device` as a fallback, so nobody paired before the split was locked
out.

The lookup that implemented that fallback took the first name **present** rather
than the first that **matched**. This gateway's own name won whenever it held
any value at all. A cookie is scoped to the host and ignores the port. So a slot
on one hostname can hold a value left by a gateway that is long gone. A device
whose good credential sat in the second name was refused, with its row still in
the store.

The same lookup left a second gap. `device_cookie_name` digests the data-dir
string, so changing that string renames the cookie for every device at once. The
store's own move shipped a seed for exactly that reason. The cookie half shipped
with a one-name fallback that a rename walks straight past, and the doc comment
conceded it: "Moving a data dir renames the cookie and asks its devices to pair
again."

Both were found while diagnosing a paired iPhone that met the pairing screen.
Both are real, and both produce the same symptom the report opened with: the
server lists the device, and the device is refused.

> **Correction, ADR 0171.** This entry went on to name the phone's storage
> container as that incident's cause. It was not. The gateway read only the
> first `cookie` header field, and HTTP/2 clients split a jar across several.
> The decisions below all stand. The cause attribution does not, and 0171
> carries the evidence.

## Decision

**Authorization reads every `lucidos_device*` cookie on the request and takes
the first whose value matches a stored device's digest.** This gateway's own
name is tried first, so the ordinary request still costs one compare.

**Whatever name carried the match, the response re-issues the credential under
this gateway's own name.** That one rule replaces the special-cased pre-split
migration, and covers a data-dir move for free.

**The credential is also renewed on every authorized page load**, not only on
the daily last-seen beat the two used to share. The store write stays on that
beat.

## Rationale

**Presence was never the question.** The store decides who a caller is, and a
cookie name only says which slot a value came out of. Reading one name and
stopping made the slot authoritative. That is what let a dead value in a
contested slot mask a live credential on the same request.

**Reading every name grants nothing.** A candidate still has to match a stored
digest. The set widens from two names to a family, and the answer for every
value in it is unchanged: no row, no access. What changes is that the gateway
stops giving up early.

**One rule beats one exception per rename.** The pre-split name was a special
case with its own flag, its own branch and its own test. A data-dir move needed
a second such case, and the next scheme change a third. Naming the family
retires all of them.

**Renewal belonged with the store write only because that write was expensive.**
Re-sending a header is not. A launch makes a handful of page loads, against one
per fetch. A page load is also where a browser certainly stores what it is
given. Renewing there restarts the window on every launch. The difference is
between a client that must hold a year-long cookie untouched and one that is
topped up whenever it is used.

## Consequences

- A device paired under any earlier release authenticates, and is moved to this
  gateway's own cookie on the same response.
- Moving a gateway's data dir no longer unpairs its devices. The store's seed
  carries the rows and this carries the cookie.
- A stale value in any one slot cannot lock a device out while its credential is
  on the request under another name.
- An authorized page load carries one extra `Set-Cookie`. Nothing else changes:
  the credential, the attributes, and the daily store write are the same.
- The refusal is no longer silent. One rate-limited line names how many
  credential cookies arrived, whether one was ours, and how many devices the
  store holds. Counts and one cookie name, never a value.
- ADR 0132's fallback clause is superseded by this rule. Its store decisions,
  its machine-wide local token and its `lucidos pair` correction all stand.

## Alternatives considered

**Keep the two-name lookup and merely reorder it.** Try the pre-split name
first, or try our own name and fall back on a miss. It fixes the masking case
and leaves the rename case exactly where it was, so the next data-dir move
unpairs everyone again. It also keeps the special case that made a second one
necessary.

**Put the matched credential in `Authorization::Device`.** It would remove the
second return value. Rejected because that value names a device and is matched
on everywhere, and a bearer secret inside it would travel to all of them. The
credential rides in a separate slot that only the re-issue site reads.

**Re-seed the store from the pre-split path on every boot.** Considered while
reading the same incident, because the two stores on the reporting machine had
diverged. Rejected outright: a later re-seed puts back every device the gateway
has revoked since, which is a revocation that does not hold.

**A reusable install token in the icon's launch URL.** Proposed as a way for an
installed home-screen app to re-pair itself with no taps after its container
drops the cookie. Rejected. A token replayed from `start_url` is a bearer secret
the app's own JS can read.

App iframes are served same-origin with `allow-same-origin`, so that is the
shape `HttpOnly` exists to prevent here. It also mints a device row per launch
after a wipe, and three "Safari on iPhone" rows had already accumulated in one
store that way. The recovery path instead names the browser on the same phone,
which is a separate device with its own credential and can mint a code.

**Refuse a ports file with no `PROTO=` line**, in the sibling fix that rides
with this one. It would make a missing protocol loud instead of guessed.
Rejected because every ports file written before the key existed looks like
that. Refusing them breaks working cross-workspace calls to make a rare one
clearer. The guess is recorded instead, and a failed call names it.
