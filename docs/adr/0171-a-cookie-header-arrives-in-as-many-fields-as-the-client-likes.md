# 0171: A cookie header arrives in as many fields as the client likes

- **Status**: Accepted
- **Date**: 2026-08-31

## Context

A paired iPhone met the pairing screen for the third time. ADR 0162 was written
after the second, and it records the cause as the phone's own storage container
dropping a cookie. **That attribution is wrong, and this ADR corrects it.**

HTTP/2 lets a client split one cookie jar across several `cookie` header fields,
because a field per cookie compresses better under HPACK (RFC 9113 §8.2.3). A
receiver is required to read them as one header. WebKit splits, hyper hands each
field to the application separately, and the gateway's parser read
`headers.get(COOKIE)`, which returns the first field alone.

So every cookie after the first was invisible to authorization. HPACK decides
the split, so the same jar arrives whole on one request and cut in two on the
next. That is the intermittence the reports describe: the phone is let in, then
turned away, with its row untouched in the store.

Measured against the running gateway before the fix. The same two cookies read
as 2 over HTTP/1.1 and 1 over HTTP/2. Going through `tailscale serve` does not
join them either, so no address avoided it.

**It was invisible from both ends, and that is why it took three rounds.** ADR
0162 added a refusal log to answer "the store lists my phone, so why the pairing
screen?". It fires only when at least one credential cookie arrives. A
credential in a dropped field is one the gateway never counted. So the log read
zero and stayed silent, treating a paired phone as a browser that had never
paired. The one instrument aimed at this bug was blind to exactly it.

## Decision

**Every `cookie` field on the request is read, and the fields are one header.**
`cookie_pairs` iterates `get_all` rather than `get`. This is what ADR 0162's
"reads every `lucidos_device*` cookie on the request" already said, now true of
the request rather than of its first field.

**The refusal log turns on carrying no cookie AT ALL, not on carrying no
credential.** A browser the host already knows, arriving without the one cookie
that lets it in, gets a line. Its line also reports how many fields arrived.

**ADR 0162's cause attribution is superseded.** Its decisions all stand: match
rather than merely find, re-issue under our own name, renew on every page load.
Only the sentence naming the container as the cause is withdrawn.

## Rationale

**A header is not a field.** Cookies are the one request header a client may
legally spread over several fields. A parser that reads one is therefore right
up to the day a client uses that allowance. Nothing about the old code looked
wrong, which is why three reviews walked past it.

**Reading every field grants nothing.** A candidate still has to match a stored
digest, exactly as ADR 0162 argued for reading every name. The set widens from
one field to all of them, and the answer for a value in none of the rows is
unchanged. Header size is capped upstream, so the compare loop stays bounded.

**Silence must key on the thing the reader cannot mistake.** Counting
credentials was the natural choice while a credential was assumed readable. It
turned the one failure worth logging into the one case that logged nothing.
Counting cookies is coarser and cannot be fooled the same way: an arriving
cookie is an observation, where an arriving credential was already a judgment.

**The correction is worth more than the fix.** ADR 0162 sent the next reader
into WebKit, and the next reader was us. A decision log that keeps a disproven
cause costs more than one that never named a cause.

## Consequences

- A device whose jar the client splits stays paired, on any HTTP version.
- The refusal line gains two counts, cookies and fields, and reports credentials
  separately. Still counts and one cookie name, never a value, still one line a
  minute.
- An unpaired browser holding any unrelated cookie for the host now produces a
  line where it produced none. The rate limit is unchanged, so the budget is.
- `presented_credential_summary` returns a `CookieAudit` rather than a pair. The
  refusal path pays one extra pass over a header list of a handful of entries.
- Nothing on the authorized path changed. `enforce` still scans the `Cookie`
  header once per request, and only a refusal builds the audit.

## Alternatives considered

**Join the fields into one string before parsing.** Allocates on a path whose
doc promises it does not, and buys nothing: the parser already splits on `;`, so
iterating the fields is the same loop without the copy.

**Reach for a cookie crate.** It would have prevented this. Rejected on the
standing ground that the gateway is the only network-facing process, so its
dependency list is kept short. The fix is one call, and the doc comment at the
parser now carries the reason a hand-rolled reader has to know.

**Turn off HTTP/2 on the gateway socket.** It makes the symptom go away by
denying clients a feature they are entitled to. It leaves the parser wrong for
the next transport that splits a header, and costs every client h2's
multiplexing for a parsing bug.

**Log every refusal, including one carrying nothing.** Considered while widening
the trigger. Rejected: a browser that has never paired is the ordinary first
run, and it makes a refusal per asset. That is the noise the rate limit and the
original silence were both protecting.

**Leave ADR 0162 alone and record only the fix.** Rejected. For anyone debugging
this next, the false cause is the load-bearing part of that entry. An ADR nobody
corrects is a decision log that teaches the wrong lesson twice.
