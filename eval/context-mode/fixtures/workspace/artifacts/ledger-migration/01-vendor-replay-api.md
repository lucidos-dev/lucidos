# Kestrel Clearing Replay API

**Specification v3.2.4** | Published 2026-06-02 | Supersedes v3.1.9

Kestrel Clearing Systems, Integrations Group. Distributed to settlement
partners under the terms of the integration agreement.

---

## 1. Scope

This document specifies the Replay API, version 3. The Replay API lets a
settlement partner re-read transaction records that Kestrel has already
delivered over the primary settlement feed.

Replay is a recovery channel. It is not a substitute for the primary feed, and
it is not a query interface over your settlement history. Records older than
the replay window are unreachable through this API. Recovering those requires
a settlement file request through the partner portal, which is out of scope
here.

### 1.1 Audience

You are expected to have a working primary feed consumer, a partner
certificate, and a Kestrel partner id. Section 3 assumes you already hold
credentials.

### 1.2 What changed since v3.1

The batch envelope grew a `window` object, the cursor format became opaque,
and three error codes were retired. Section 20 lists every change with its
migration note. Partners still on v2 should read section 19 first.

### 1.3 Conventions

| Convention | Meaning |
|---|---|
| **MUST**, **MUST NOT** | A hard requirement, enforced by the server |
| **SHOULD** | A strong recommendation, not enforced |
| **MAY** | Optional behaviour |
| `monospace` | A literal field name, value, or path |
| *italic* | A term defined in section 5 |

All timestamps are RFC 3339 with a mandatory offset. Kestrel always emits
`Z`. All monetary amounts are integer minor units with a separate currency
code. There are no floating point amounts anywhere in this API.

---

## 2. Versioning and deprecation

The major version is part of the path. Version 3 lives under `/v3`. A major
version is supported for at least eighteen months after its successor ships.

Minor versions are additive only. Kestrel may add a field to a response, add
an optional request parameter, or add a value to an enumeration. Your client
MUST tolerate unknown fields and unknown enumeration values. A client that
rejects an unknown field will break on any minor release.

Kestrel MUST NOT remove a field, rename a field, or change a field's type
within a major version. A field being retired is marked deprecated in the
changelog, keeps working, and disappears only at the next major version.

### 2.1 Deprecation signals

A deprecated endpoint returns a `Deprecation` header carrying the date the
endpoint entered deprecation, and a `Sunset` header carrying the date it stops
answering. Both are HTTP-date format, per RFC 8594.

```
Deprecation: Wed, 04 Mar 2026 00:00:00 GMT
Sunset: Sat, 05 Sep 2026 00:00:00 GMT
Link: <https://docs.kestrel-clearing.example/v3/migrating-cursors>; rel="deprecation"
```

Your integration SHOULD alert on the presence of a `Sunset` header. Kestrel
sends a partner notice at the same time, but the header is the authoritative
signal and it reaches your code rather than your inbox.

---

## 3. Authentication

Every request carries two credentials. Both are required. There is no
API-key-only mode and no bearer-only mode.

1. **A client certificate**, presented during the TLS handshake. Kestrel pins
   the certificate to your partner id.
2. **A bearer token**, in the `Authorization` header, obtained from the token
   endpoint below.

### 3.1 Obtaining a token

```
POST /v3/oauth/token
Content-Type: application/x-www-form-urlencoded

grant_type=client_credentials&scope=replay.read+replay.write
```

```json
{
  "access_token": "kst_at_9f4c2e7a1b8d40559e3a6c0f2d7b8e14",
  "token_type": "Bearer",
  "expires_in": 3540,
  "scope": "replay.read replay.write"
}
```

Tokens live for 59 minutes. Kestrel deliberately does not issue a refresh
token: the client credentials grant is cheap, and a refresh token is one more
secret to store. Request a new token when the old one is within five minutes
of expiry.

### 3.2 Scopes

| Scope | Grants |
|---|---|
| `replay.read` | Read session state, fetch batches |
| `replay.write` | Create a session, acknowledge a batch, cancel a session |
| `replay.admin` | Read another partner id's sessions, granted only to service bureaus |

A token missing the scope an endpoint needs returns `403` with error code
`scope_insufficient`. The error names the scope that was missing.

### 3.3 Certificate rotation

Kestrel accepts two active client certificates per partner id. Rotation is
therefore a four-step sequence with no downtime.

1. Upload the new certificate through the partner portal.
2. Wait for the portal to show both certificates as active. This takes up to
   fifteen minutes to propagate across regions.
3. Switch your clients to the new certificate.
4. Revoke the old certificate through the portal.

Skipping step 2 is the most common cause of intermittent handshake failures
during a rotation. The propagation is eventually consistent, so a client can
succeed in one region and fail in another for several minutes.

---

## 4. Environments

| Environment | Host | Data | Rate limit |
|---|---|---|---|
| Sandbox | `api.sandbox.kestrel-clearing.example` | Synthetic, reset nightly | 4 req/s |
| Certification | `api.cert.kestrel-clearing.example` | Replayed real shapes, scrubbed | 8 req/s |
| Production | `api.kestrel-clearing.example` | Live | 12 req/s |

Sandbox and certification issue their own credentials. A production token is
rejected by sandbox and the reverse is also true. The sandbox reset runs at
03:15 UTC and destroys every session, including sessions still draining.

---

## 5. Concepts

### 5.1 Settlement day

A *settlement day* runs from 00:00:00 UTC to 23:59:59.999 UTC. Every record
belongs to exactly one settlement day, named by its date in `YYYY-MM-DD`
form. The day a record belongs to is fixed when Kestrel finalises it, and it
never moves afterwards.

A settlement day *closes* when Kestrel has finalised every record for it.
Closing is not instantaneous at midnight. Finalisation runs behind the clock,
and the lag is described in section 6.

### 5.2 Record

A *record* is one settled transaction as Kestrel sees it. A record carries a
header, a set of line items, and a set of references back to the originating
authorisation. Section 12 gives the full schema.

A record is immutable once finalised. A correction is a new record with a
`corrects` reference to the original, never an edit of the original. Your
consumer MUST handle a correction arriving after you have already booked the
record it corrects.

### 5.3 Replay session

A *replay session* is a server-side cursor over a range of records. You create
one, drain it batch by batch, and either exhaust it or cancel it. A session
holds its position, so a client that crashes mid-drain resumes where it
stopped rather than starting over.

Sessions are single-consumer. Two clients draining one session will each see
a subset of the batches, and neither sees the whole range. Section 8.4
explains why the server cannot detect this for you.

### 5.4 Batch

A *batch* is one page of records from a session. Batches are delivered in
ascending finalisation order and are never re-ordered. A batch is delivered
at least once: see section 16 on idempotency.

### 5.5 Window

The *window* is the span of time a session may reach back over. It is the
single most important constraint in this API, and section 6 is devoted to it.

---

## 6. The replay window

### 6.1 The rule

**A replay session may reach back at most 26 hours from the moment the session
is created.** The `from` timestamp of a session request MUST NOT be earlier
than 26 hours before the request arrives at Kestrel. A request that violates
this is rejected with `window_exceeded`, and the error body names the earliest
timestamp that would have been accepted.

The window is evaluated once, at session creation. A long-running session that
outlives its own window keeps working. Records that were reachable when the
session was created stay reachable until the session ends.

### 6.2 Why 26 and not a round number

The window covers a full day plus the finalisation lag. Kestrel finalises a
settlement day up to two hours after that day's boundary. A window of exactly
one day would fail at the worst moment. The earliest records of a day would
become unreachable just as the day closed. That is when a partner most often
discovers a gap.

The extra two hours exist to cover that lag. Size your recovery tooling
against the documented figure, not against a round day. Partners who assume a
round day lose the last two hours of recoverable history. This is the most
frequently reported non-bug in the Replay API.

### 6.3 Interaction with session lifetime

Session lifetime and the replay window are different limits and they are often
confused.

| Limit | Value | Measured from | On breach |
|---|---|---|---|
| Replay window | 26 hours | Session creation, looking backwards | `window_exceeded` at creation |
| Session lifetime | 6 hours | Session creation, looking forwards | `session_expired` on next fetch |
| Batch acknowledgement deadline | 15 minutes | Batch delivery | Batch redelivered |

A session that has not been touched for 6 hours is reaped. Draining a large
range therefore needs a client that keeps moving, not one that fetches a batch
per scheduled run.

### 6.4 What the window does not cover

The window bounds `from`. It does not bound `to`, and it does not bound how
much data a session may carry. A session created with a `from` at the far edge
of the window may legitimately return several million records.

---

## 7. Session lifecycle

```
                 create
                   |
                   v
              [ pending ] --- validation fails ---> [ failed ]
                   |
              server prepares
                   |
                   v
              [ ready ] <----------------+
                   |                     |
              fetch batch                | more batches remain
                   |                     |
                   v                     |
              [ draining ] --------------+
                   |
              last batch acknowledged
                   |
                   v
              [ exhausted ]

  Any state except exhausted may transition to [ cancelled ] or [ expired ].
```

A session in `pending` is not yet fetchable. Preparation is usually under two
seconds and is bounded at 90 seconds. Poll section 9's status endpoint rather
than fetching against a pending session, which returns `session_not_ready`.

---

## 8. Create a replay session

```
POST /v3/replay/sessions
Authorization: Bearer <token>
Content-Type: application/json
Idempotency-Key: <uuid>
```

### 8.1 Request parameters

| Field | Type | Required | Description |
|---|---|---|---|
| `from` | timestamp | yes | Inclusive lower bound on finalisation time |
| `to` | timestamp | no | Exclusive upper bound, defaults to now |
| `partner_id` | string | no | Only for `replay.admin`, defaults to the token's partner |
| `filters` | object | no | See 8.2 |
| `batch_size` | integer | no | Records per batch, see section 10 |
| `include` | string[] | no | Optional expansions, see 8.3 |
| `callback_url` | string | no | See section 17 |

### 8.2 Filters

| Filter | Type | Description |
|---|---|---|
| `currencies` | string[] | ISO 4217 codes, at most 12 |
| `record_types` | string[] | See the enumeration in 12.2 |
| `merchant_ids` | string[] | At most 200 |
| `corrections_only` | boolean | Only records carrying a `corrects` reference |
| `exclude_reversals` | boolean | Drop records of type `reversal` |

Filters are applied server-side before batching. A heavily filtered session
still walks the whole range internally, so preparation time tracks the range
rather than the result count.

### 8.3 Expansions

| Value | Adds |
|---|---|
| `line_items` | The full line item array, see section 13 |
| `authorisation` | The originating authorisation snapshot |
| `fees` | The fee breakdown, per line item |
| `raw_network` | The card network message, base64 encoded |

Expansions are the main driver of response size. `raw_network` in particular
roughly doubles a record. Section 10.3 explains how expansions interact with
the batch cap.

### 8.4 Example request

```json
{
  "from": "2026-06-01T04:00:00Z",
  "to": "2026-06-01T16:00:00Z",
  "batch_size": 480,
  "include": ["line_items", "fees"],
  "filters": {
    "currencies": ["EUR", "NOK", "SEK"],
    "exclude_reversals": true
  }
}
```

### 8.5 Example response

```json
{
  "session_id": "rs_01J8ZQ4M7YT3XK2P9B6D0N5FVC",
  "state": "pending",
  "created_at": "2026-06-02T09:14:22Z",
  "expires_at": "2026-06-02T15:14:22Z",
  "window": {
    "earliest_permitted_from": "2026-06-01T07:14:22Z",
    "requested_from": "2026-06-01T04:00:00Z",
    "hours": 26
  },
  "estimated_records": null,
  "batch_size": 480,
  "links": {
    "self": "/v3/replay/sessions/rs_01J8ZQ4M7YT3XK2P9B6D0N5FVC",
    "batches": "/v3/replay/sessions/rs_01J8ZQ4M7YT3XK2P9B6D0N5FVC/batches"
  }
}
```

Note that `estimated_records` is `null` while the session is pending. It is
populated once preparation finishes, and it is an estimate: the exact count is
known only when the session is exhausted.

---

## 9. Read session status

```
GET /v3/replay/sessions/{session_id}
Authorization: Bearer <token>
```

```json
{
  "session_id": "rs_01J8ZQ4M7YT3XK2P9B6D0N5FVC",
  "state": "draining",
  "created_at": "2026-06-02T09:14:22Z",
  "expires_at": "2026-06-02T15:14:22Z",
  "estimated_records": 1284900,
  "delivered_records": 411840,
  "acknowledged_records": 411360,
  "batches_delivered": 858,
  "batches_acknowledged": 857,
  "batch_size": 480,
  "oldest_unacknowledged_batch": {
    "batch_id": "rb_00000859",
    "delivered_at": "2026-06-02T09:41:07Z",
    "redelivery_after": "2026-06-02T09:56:07Z"
  }
}
```

`delivered_records` counts every record the server has handed out, including
records in batches you have not acknowledged. `acknowledged_records` is the
figure to use for progress reporting: it is the only one that never goes
backwards.

### 9.1 Progress is not monotonic in delivered_records

A redelivered batch increments `delivered_records` a second time. A client
that computes a percentage from `delivered_records` will see the bar move past
100 on a session with redeliveries. Use `acknowledged_records` instead.

---

## 10. Batches and the batch cap

### 10.1 The cap

**`batch_size` accepts an integer from 1 to 480 inclusive.** The default is
100. A request naming a larger value is rejected at session creation with
`batch_too_large`, and the error body names the cap.

The cap is a property of the session, not of the fetch. You cannot vary it
per batch. Changing it means cancelling the session and creating a new one,
which restarts the drain from `from`.

### 10.2 Why the cap is 480

A record may carry up to 16 line items, and the response envelope is capped
at 8 MiB. A line item serialises to roughly 1 KiB with the `fees` expansion
on. So 480 records of 16 line items each fill the envelope with a margin of
about four percent.

The cap is not a round number for exactly that reason. It is derived from the
envelope, and it is the largest value that cannot overflow it. Partners who
guess a round cap get `batch_too_large` on their first production call, which
is a cheap failure but an avoidable one.

### 10.3 When you will not receive a full batch

A batch may be shorter than `batch_size` for four reasons.

| Reason | How to tell |
|---|---|
| The session is exhausted | `has_more` is `false` |
| The envelope filled before the count did | `truncated_by` is `"envelope"` |
| A filter removed records inside the page | Normal, no signal |
| The server is shedding load | `truncated_by` is `"load"` |

Only the last is transient. A client MUST NOT treat a short batch as the end
of a session. `has_more` is the single authoritative end-of-session signal.

### 10.4 Choosing a batch size

Larger is not always faster. The envelope, not the count, usually binds when
expansions are on.

| Expansions | Practical size | Typical batch bytes |
|---|---|---|
| None | 480 | 340 KiB |
| `line_items` | 480 | 2.1 MiB |
| `line_items`, `fees` | 480 | 6.8 MiB |
| `line_items`, `fees`, `raw_network` | 180 | 7.4 MiB |
| `raw_network` alone | 240 | 6.9 MiB |

With `raw_network` on, the cap is unreachable in practice: the envelope always
binds first. Setting 480 there is harmless but misleading, because the
observed page size will settle near 180.

### 10.5 Fetch a batch

```
GET /v3/replay/sessions/{session_id}/batches?cursor=<opaque>
Authorization: Bearer <token>
Accept: application/json
```

```json
{
  "batch_id": "rb_00000859",
  "session_id": "rs_01J8ZQ4M7YT3XK2P9B6D0N5FVC",
  "sequence": 859,
  "record_count": 480,
  "has_more": true,
  "truncated_by": null,
  "cursor": "eyJzIjoicnNfMDFKOFpRNE03WVQzWEsyUDlCNkQwTjVGVkMiLCJvIjo0MTE4NDB9",
  "delivered_at": "2026-06-02T09:41:07Z",
  "acknowledge_by": "2026-06-02T09:56:07Z",
  "records": []
}
```

Omit `cursor` on the first fetch. Every later fetch MUST carry the cursor from
the previous batch. A cursor is opaque: it is a base64 string whose contents
are not part of this contract and which changed shape in v3.0.

### 10.6 Cursor rules

A cursor is valid only for the session that issued it. A cursor from another
session returns `cursor_mismatch`. A cursor from a cancelled or expired
session returns `session_expired`.

Cursors are not durable across a major version. Do not persist a cursor for
longer than the session that issued it, which is at most 6 hours.

---

## 11. Acknowledge and cancel

### 11.1 Acknowledge a batch

```
POST /v3/replay/sessions/{session_id}/ack
Content-Type: application/json

{"batch_id": "rb_00000859"}
```

Acknowledgement is what releases the batch server-side. An unacknowledged
batch is redelivered after 15 minutes, at the same `sequence` and with the
same `batch_id`.

Acknowledge after you have durably stored the records, not when you receive
them. The deadline is generous precisely so that the acknowledgement can
follow a database commit rather than precede it.

### 11.2 Acknowledgement is cumulative

Acknowledging batch *n* implicitly acknowledges every batch before it. A
client that loses track of one acknowledgement can recover by acknowledging
the highest batch it has stored.

This is the one place in the API where an operation affects records it does
not name. It is deliberate: it makes recovery after a client crash a single
call rather than a replay of the whole acknowledgement history.

### 11.3 Cancel a session

```
DELETE /v3/replay/sessions/{session_id}
```

Cancellation is immediate and irreversible. In-flight fetches against a
cancelled session fail with `session_expired`. There is no way to resume a
cancelled session, and its cursors are dead.

Cancel sessions you have abandoned. A partner may hold at most 8 concurrent
sessions, and an abandoned session occupies a slot until it is reaped 6 hours
later.

---

## 12. Record schema

### 12.1 Header fields

| Field | Type | Null | Description |
|---|---|---|---|
| `record_id` | string | no | Kestrel's identifier, ULID form, globally unique |
| `record_type` | enum | no | See 12.2 |
| `settlement_day` | date | no | The day this record belongs to |
| `finalised_at` | timestamp | no | When Kestrel finalised it, the replay ordering key |
| `merchant_id` | string | no | Your merchant identifier as registered with Kestrel |
| `terminal_id` | string | yes | Present for card-present records only |
| `currency` | string | no | ISO 4217 alphabetic code |
| `gross_minor` | integer | no | Gross amount in minor units, always positive |
| `net_minor` | integer | no | Gross less fees, may be negative on a reversal |
| `fee_minor` | integer | no | Total fees, always positive |
| `direction` | enum | no | `credit` or `debit`, from the merchant's perspective |
| `corrects` | string | yes | The `record_id` this record corrects |
| `corrected_by` | string | yes | Populated on the original once a correction exists |
| `scheme` | enum | yes | Card scheme, absent for account-to-account records |
| `auth_code` | string | yes | Six characters, present for card records |
| `arn` | string | yes | Acquirer reference number, 23 digits |
| `batch_reference` | string | yes | The acquirer batch this record settled in |
| `line_items` | array | yes | Present only with the `line_items` expansion |
| `fees` | array | yes | Present only with the `fees` expansion |
| `authorisation` | object | yes | Present only with the `authorisation` expansion |
| `raw_network` | string | yes | Base64, present only with the `raw_network` expansion |
| `metadata` | object | yes | Merchant-supplied key values, at most 20 keys |

### 12.2 Record types

| Value | Meaning |
|---|---|
| `sale` | An ordinary purchase |
| `refund` | A merchant-initiated return of funds |
| `reversal` | An authorisation reversed before capture |
| `chargeback` | A cardholder dispute debited from the merchant |
| `representment` | A chargeback contested and re-presented |
| `fee_adjustment` | A fee correction with no underlying transaction |
| `funding` | A payout to the merchant's bank account |
| `funding_return` | A payout that the receiving bank rejected |

`funding` and `funding_return` carry no `line_items` and no `scheme`. A
consumer that assumes every record has a scheme will fail on the first payout
of the day. For most partners that lands around 04:00 UTC.

### 12.3 The direction field and sign

`gross_minor` is always positive. Direction, not sign, tells you which way the
money moved. This trips up nearly every new integration.

A `refund` is a `debit` with a positive `gross_minor`. A `chargeback` is also
a `debit`. A `representment` is a `credit` that reverses a prior `chargeback`
and references it through `corrects`.

### 12.4 Example record

```json
{
  "record_id": "rec_01J8ZR2K4C7M9X0V5T3H8N6PQD",
  "record_type": "sale",
  "settlement_day": "2026-06-01",
  "finalised_at": "2026-06-01T04:12:44.318Z",
  "merchant_id": "mrc_44819",
  "terminal_id": "trm_0071",
  "currency": "NOK",
  "gross_minor": 128900,
  "net_minor": 126318,
  "fee_minor": 2582,
  "direction": "credit",
  "corrects": null,
  "corrected_by": null,
  "scheme": "visa",
  "auth_code": "4TG91K",
  "arn": "74100266153000098412773",
  "batch_reference": "acq_20260601_0043",
  "metadata": {"order_ref": "SO-2026-118342", "channel": "in_store"}
}
```

---

## 13. Line item schema

Line items are present only when the `line_items` expansion is requested. A
record carries between 0 and 16 of them. The count is capped because the
envelope arithmetic in section 10.2 depends on it.

| Field | Type | Null | Description |
|---|---|---|---|
| `line_id` | string | no | Unique within the record, not globally |
| `position` | integer | no | 1-based, dense, never re-used within a record |
| `sku` | string | yes | Merchant-supplied, at most 64 characters |
| `description` | string | yes | Merchant-supplied, at most 256 characters |
| `quantity_milli` | integer | no | Quantity in thousandths, so 1.5 units is 1500 |
| `unit_price_minor` | integer | no | Price per unit, minor units, excluding tax |
| `line_gross_minor` | integer | no | Quantity times unit price plus tax |
| `tax_minor` | integer | no | Tax on this line |
| `tax_rate_bp` | integer | no | Tax rate in basis points, so 25% is 2500 |
| `tax_code` | string | yes | Jurisdiction-specific code, unvalidated |
| `discount_minor` | integer | no | Discount applied to this line, positive |
| `fees` | array | yes | Per-line fee split, with the `fees` expansion |

### 13.1 Line totals do not have to sum to the record total

This surprises people, so it is stated plainly. The sum of `line_gross_minor`
across a record's line items MAY differ from the record's `gross_minor`.

Three causes account for nearly all of it. A merchant may submit partial line
detail. A rounding difference of up to one minor unit per line is permitted.
An order-level discount is not attributed to any line.

Reconcile at the record level. Use line items for analysis and for tax
reporting, never as the source of truth for the amount settled.

### 13.2 Quantity precision

`quantity_milli` is an integer in thousandths. A quantity of 0.001 is the
smallest representable, and there is no way to express a smaller one. Weighed
goods that need finer precision are conventionally submitted as a quantity of
1 with the whole amount in `unit_price_minor`.

---

## 14. Errors

### 14.1 Error envelope

Every error, at every status code, has the same body.

```json
{
  "error": {
    "code": "window_exceeded",
    "message": "from is earlier than the replay window permits",
    "detail": {
      "requested_from": "2026-05-31T18:00:00Z",
      "earliest_permitted_from": "2026-06-01T07:14:22Z"
    },
    "request_id": "req_01J8ZS0P5V2N7C4A8E1K6M9RTX",
    "retryable": false
  }
}
```

`code` is stable and machine-readable. `message` is for humans and may change
without notice. Never branch on `message`.

`request_id` is the only thing Kestrel support will ask for. Log it on every
error, including errors your client handles silently.

### 14.2 Catalogue

| HTTP | Code | Retryable | Meaning |
|---|---|---|---|
| 400 | `malformed_request` | no | The body is not valid JSON |
| 400 | `missing_field` | no | A required field is absent, `detail.field` names it |
| 400 | `invalid_timestamp` | no | Not RFC 3339, or no offset |
| 400 | `range_inverted` | no | `to` is not after `from` |
| 400 | `batch_too_large` | no | `batch_size` above the cap, `detail.cap` names it |
| 400 | `too_many_filters` | no | A filter array exceeded its limit |
| 401 | `token_expired` | yes | Fetch a new token and retry once |
| 401 | `token_invalid` | no | The token is malformed or revoked |
| 403 | `scope_insufficient` | no | `detail.required_scope` names the missing scope |
| 403 | `certificate_mismatch` | no | The client certificate is not pinned to this partner |
| 403 | `partner_suspended` | no | Contact your Kestrel account manager |
| 404 | `session_not_found` | no | Unknown session id, or another partner's session |
| 409 | `session_not_ready` | yes | Still preparing, poll status |
| 409 | `cursor_mismatch` | no | The cursor belongs to a different session |
| 409 | `idempotency_conflict` | no | Key reused with a different body |
| 410 | `session_expired` | no | Expired, cancelled, or reaped |
| 413 | `payload_too_large` | no | The request body exceeded 256 KiB |
| 422 | `window_exceeded` | no | `from` is outside the replay window |
| 422 | `no_primary_feed` | no | Replay needs a configured primary feed |
| 429 | `rate_limited` | yes | Honour `Retry-After` |
| 429 | `session_quota_exceeded` | yes | Cancel an abandoned session |
| 500 | `internal_error` | yes | Retry with backoff, then contact support |
| 502 | `upstream_unavailable` | yes | Kestrel's settlement store is unreachable |
| 503 | `draining_for_maintenance` | yes | Honour `Retry-After`, usually under two minutes |
| 504 | `preparation_timeout` | yes | Narrow the range and create a new session |

### 14.3 Retry policy

Retry only where `retryable` is `true`. Retrying a non-retryable error wastes
your rate limit and will not succeed.

Use exponential backoff with full jitter, starting at 250 ms and capping at
30 s. Give up after eight attempts and alert. A `429` overrides your backoff:
honour `Retry-After` exactly.

### 14.4 The three errors partners actually hit

Support volume is heavily concentrated. Three codes account for most of it.

`window_exceeded` is first, and section 6.2 explains why. `session_expired`
is second, almost always from a client that drains one batch per scheduled
run. `batch_too_large` is third, and it is always a guessed cap.

---

## 15. Rate limits and quotas

| Limit | Production value | Scope | On breach |
|---|---|---|---|
| Requests per second | 12 | Partner id | `rate_limited` |
| Burst allowance | 40 | Partner id | Absorbed, then shaped |
| New sessions per hour | 90 | Partner id | `rate_limited` |
| Concurrent sessions | 8 | Partner id | `session_quota_exceeded` |
| Request body | 256 KiB | Request | `payload_too_large` |
| Response envelope | 8 MiB | Response | Batch truncated |

Limits are enforced with a token bucket refilled continuously, not with a
fixed window. A client that paces itself at the documented rate will never see
a `429`, and one that bursts and sleeps will see them intermittently.

### 15.1 Headers

```
X-RateLimit-Limit: 12
X-RateLimit-Remaining: 7
X-RateLimit-Reset: 1780216882
Retry-After: 2
```

`X-RateLimit-Reset` is a Unix timestamp in seconds. `Retry-After` is present
only on a `429` and is in whole seconds.

### 15.2 Raising a limit

Limits are per partner id and are raised on request. A raise needs a written
estimate of steady-state and peak volume, and it takes two business days.
Kestrel does not raise limits during an active incident on either side.

---

## 16. Idempotency and delivery semantics

### 16.1 Session creation is idempotent

`POST /v3/replay/sessions` accepts an `Idempotency-Key` header carrying a UUID.
Kestrel stores the key with the response for 26 hours, matching the replay
window.

Replaying the same key with the same body returns the original response, with
`Idempotency-Replayed: true`. Replaying the same key with a different body
returns `idempotency_conflict`.

Session creation is the only endpoint that takes the header. Fetches are
naturally idempotent through the cursor, and acknowledgement is idempotent
because it is cumulative.

### 16.2 Batches are at-least-once

A batch may be delivered more than once. This happens after an acknowledgement
deadline passes, and it happens after certain server-side failovers even
within the deadline.

Your consumer MUST be idempotent on `record_id`. A `record_id` is globally
unique and immutable, so an upsert keyed on it is sufficient. Do not key on
`batch_id`, which is stable per delivery but tells you nothing about whether
you have already stored the contents.

### 16.3 Ordering guarantees

Records within a session are ordered by `finalised_at`, ascending. Ties are
broken by `record_id`, ascending, which is stable because ULIDs are.

There is no ordering guarantee *across* sessions. Two sessions covering
overlapping ranges may interleave arbitrarily.

---

## 17. Callbacks

A session created with a `callback_url` receives a POST when it becomes ready,
and another when it is exhausted, cancelled, or expired.

```json
{
  "event": "session.ready",
  "session_id": "rs_01J8ZQ4M7YT3XK2P9B6D0N5FVC",
  "occurred_at": "2026-06-02T09:14:24Z",
  "estimated_records": 1284900
}
```

Callbacks are signed with an HMAC over the raw body, using the shared secret
from the partner portal. The signature is in `Kestrel-Signature`, with a
timestamp to prevent replay.

```
Kestrel-Signature: t=1780216464,v1=8b1f5b0d0c9e4a2f7d3c6a5b8e0f1234567890abcdef1234567890abcdef1234
```

Verify the signature before parsing the body. Reject a callback whose
timestamp is more than five minutes from your own clock.

### 17.1 Callbacks are a hint, not a channel

A callback tells you a session changed state. It never carries records, and a
missed callback loses nothing.

Kestrel retries a failed callback four times over ten minutes and then stops.
Your drain loop MUST NOT depend on receiving one. Poll the status endpoint as
a fallback.

---

## 18. Sandbox

The sandbox serves synthetic records with realistic shapes and unrealistic
amounts. Every merchant id in the sandbox begins `mrc_test_`.

### 18.1 Deterministic triggers

The sandbox reacts to magic values so you can exercise error paths without
waiting for a real failure.

| Trigger | Effect |
|---|---|
| `batch_size` of 13 | Every fetch returns `truncated_by: "load"` |
| `from` exactly at the window edge | Session creation returns `window_exceeded` |
| A merchant filter of `mrc_test_slow` | Preparation takes 60 seconds |
| A merchant filter of `mrc_test_flap` | One batch in four is redelivered |
| A `callback_url` on port 9 | Callback delivery always fails |

### 18.2 What the sandbox does not model

The sandbox does not model the finalisation lag, so the practical difference
between a round day and the real window is invisible there. Test that path
against certification, which does model it.

Nor does the sandbox model rate limiting faithfully. Its limit is lower, and
its bucket is a fixed window rather than a continuous refill.

---

## 19. Migrating from v2

Version 2 sunsets on 2027-01-31. The main differences are listed here, and the
full migration guide is on the documentation site.

| Area | v2 | v3 |
|---|---|---|
| Pagination | Offset and limit | Opaque cursor |
| Window | Named per plan, typically shorter | 26 hours for every partner |
| Batch cap | 200 | 480 |
| Acknowledgement | Per record | Per batch, cumulative |
| Amounts | Decimal strings | Integer minor units |
| Errors | Bare HTTP status | Structured envelope, section 14 |
| Corrections | In-place mutation | New record with `corrects` |

The correction change is the one that breaks consumers silently. Under v2 a
record could change after you stored it. Under v3 it cannot, and a consumer
still polling for mutations will simply never see any.

---

## 20. Changelog

**v3.2.4, 2026-06-02.** Documented the envelope arithmetic behind the batch
cap. Added `truncated_by` to the batch envelope. Clarified that
`acknowledged_records` is the only monotonic progress counter.

**v3.2.0, 2026-04-18.** Added the `fees` expansion. Added
`session_quota_exceeded`. Raised the concurrent session limit from 4 to 8.

**v3.1.9, 2026-02-27.** Fixed a defect where `corrected_by` was not populated
on the original record until the next settlement day.

**v3.1.0, 2025-11-05.** Added callbacks. Added the `corrections_only` filter.
Retired `partial_batch`, `cursor_expired`, and `filter_unsupported`.

**v3.0.0, 2025-08-14.** First release of version 3. Cursors became opaque,
amounts became integer minor units, and acknowledgement became cumulative.

---

## Appendix A: a complete drain

The sequence below drains one settlement day of records with line items and
fees. It is written as pseudocode and omits error handling for clarity.

```
token   = fetch_token(scope = "replay.read replay.write")
session = POST /v3/replay/sessions {
            from:       day_start,
            to:         day_start + 1 day,
            batch_size: 480,
            include:    ["line_items", "fees"]
          }

wait until GET /v3/replay/sessions/{session.id}.state == "ready"

cursor = null
loop:
    batch = GET /v3/replay/sessions/{session.id}/batches?cursor={cursor}
    store_records(batch.records)          # upsert on record_id
    commit()
    POST /v3/replay/sessions/{session.id}/ack {batch_id: batch.batch_id}
    cursor = batch.cursor
    if not batch.has_more: break

assert GET /v3/replay/sessions/{session.id}.state == "exhausted"
```

Three details in that loop matter. The commit precedes the acknowledgement, so
a crash between them costs a redelivery rather than a lost batch. The cursor
comes from the batch rather than being computed. The loop ends on `has_more`
rather than on a short batch.

## Appendix B: quick reference

| You want to | Call |
|---|---|
| Recover a gap under one day old | `POST /v3/replay/sessions` |
| Recover a gap older than the window | Partner portal, file request |
| Check drain progress | `GET /v3/replay/sessions/{id}` |
| Free a session slot | `DELETE /v3/replay/sessions/{id}` |
| Re-read a batch you failed to store | Do nothing, let it redeliver |
| Change the batch size mid-drain | Cancel and create a new session |

## Appendix C: support

Raise an integration ticket through the partner portal. Include the
`request_id` from the error envelope, the session id, and the UTC time of the
call. Kestrel retains request logs for eight days.

For a production incident affecting settlement, use the partner incident line
listed in your integration agreement. Do not use it for sandbox problems.

## Appendix D: window arithmetic, worked

The earliest permitted `from` is always the request time less the window. The
table works that through for one settlement day.

| Session created at | Earliest permitted `from` | Is 2026-06-01 fully reachable |
|---|---|---|
| 2026-06-02T00:00:00Z | 2026-05-31T22:00:00Z | yes |
| 2026-06-02T02:00:00Z | 2026-06-01T00:00:00Z | yes, exactly |
| 2026-06-02T09:14:22Z | 2026-06-01T07:14:22Z | no, first 7h unreachable |
| 2026-06-02T23:00:00Z | 2026-06-01T21:00:00Z | no, only the last 3h |
| 2026-06-03T01:59:59Z | 2026-06-01T23:59:59Z | no, the final second only |
| 2026-06-03T02:00:01Z | 2026-06-02T00:00:01Z | no, the day has fallen out |

Read the second row carefully. A settlement day is fully reachable until
02:00 UTC on the day after next, and not a minute longer. That is the practical
form of the window, and it is the form worth writing into your runbook.

A partner sizing against a round day would compute 00:00 UTC there instead of
02:00. The two hours between those figures are the hours in which a
finalisation-lag gap is usually discovered.

## Appendix E: certification checklist

Kestrel certifies a partner integration before enabling production replay.
Certification is self-service against the certification environment, and the
portal records the result of each case.

| # | Case | Pass condition |
|---|---|---|
| C1 | Obtain a token | 200, token used successfully |
| C2 | Token expiry handling | Client refreshes without operator action |
| C3 | Create a session | 201, state pending |
| C4 | Poll to ready | Ready observed without fetching early |
| C5 | Drain to exhausted | Every batch acknowledged |
| C6 | Cursor discipline | No fetch without the previous cursor |
| C7 | Short batch | Drain continues, `has_more` respected |
| C8 | Redelivery | Duplicate `record_id` upserted, not double-booked |
| C9 | Cumulative ack | Recovery from a lost acknowledgement |
| C10 | Window breach | `window_exceeded` handled, not retried |
| C11 | Batch cap breach | `batch_too_large` handled, cap read from `detail` |
| C12 | Rate limit | `Retry-After` honoured exactly |
| C13 | Session expiry | `session_expired` triggers a new session, not a retry |
| C14 | Cancel | Slot freed, verified through status |
| C15 | Correction after booking | `corrects` applied to an already-stored record |
| C16 | Payout record | `funding` stored despite absent `scheme` |
| C17 | Line total mismatch | Record total preferred over the line sum |
| C18 | Unknown enumeration value | Tolerated, not rejected |
| C19 | Unknown response field | Tolerated, not rejected |
| C20 | Callback signature | Invalid signature rejected |

C8, C15 and C18 are the three cases partners most often fail on the first
attempt. All three are tolerance cases: the integration works until the
unusual input arrives, and then it stops.

## Appendix F: glossary

| Term | Definition |
|---|---|
| Acknowledgement | Confirmation that a batch is durably stored |
| ARN | Acquirer reference number, the scheme's transaction handle |
| Batch | One page of records from a session |
| Correction | A new record superseding an earlier one |
| Cursor | An opaque position within a session |
| Envelope | The serialised response body, capped at 8 MiB |
| Finalisation | The point at which Kestrel fixes a record |
| Minor units | The smallest unit of a currency, as an integer |
| Primary feed | The push channel replay exists to recover |
| Record | One settled transaction |
| Replay window | How far back a new session may reach |
| Session | A server-side cursor over a range of records |
| Settlement day | A UTC calendar day of finalised records |

