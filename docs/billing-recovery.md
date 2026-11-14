# Coding-agent billing recovery

`scripts/recover-billing.py` creates an attribution overlay from audited evidence.
It runs on macOS/Linux with Python 3 and has no engine, database or network access.
It never updates original events, counts usage again or calculates prices.

Run a dry-run summary first:

```sh
python3 scripts/recover-billing.py --captures captures.json --evidence evidence.json
python3 scripts/recover-billing.py --captures captures.json --evidence evidence.json --output recovery.json
python3 scripts/recover-billing_test.py
```

The output path must not exist. Repeat runs with identical inputs produce identical
JSON. Review the evidence before passing the overlay to an app.

## Input contract

The capture snapshot is an object with a `captures` array. Each row has `id`,
`thread_id`, `sequence`, `request_event_id` (nullable), `original_event_sha256`,
`producer`, `provider` (null or `unknown`), and optional `usage` token counts.
Additional snapshot metadata is included in the canonical snapshot digest.
Only `claude_code` and `codex` producers are accepted.

Compute the original event digest from the full event returned by the event store,
using Python `json.dumps(event, sort_keys=True, separators=(',', ':'))` encoded as
UTF-8, then SHA256. Keep raw transcripts and auth files out of these inputs.
Event query `since` and `until` are both exclusive time bounds.

The evidence document has `schema_version: 1` and an `observations` array.
An empty array is valid and proves no provider. Each observation contains:

- `capture`: all five identity fields listed above, matching exactly.
- `producer`: matching the capture.
- `scope`: `exact-capture`.
- `source`: `runtime-observation`, `session-metadata` or `call-metadata`.
- `source_sha256`: digest of the contemporaneous evidence retained by its owner.
- `status`: `known`, `transport-only` or `unresolved`.
- For `known`, `provider` and `billing_mode` from the table below.

| Provider | Billing mode |
|---|---|
| claude-gateway | unknown |
| vertex, bedrock, foundry, anthropic, openai | token-metered |
| anthropic-subscription, chatgpt-plan | plan |

Codex accepts only `openai` and `chatgpt-plan`; Claude accepts the other providers.
These are reviewed evidence assertions, not a transcript discovery mechanism.
The utility validates binding and consistency; it cannot establish that an
operator's assertion is true. The evidence owner must verify effective backend,
endpoint and auth mode for that exact call before asserting `known`.
A source digest alone does not prove the assertion. Do not feed current settings,
model names, configuration directory names or calendar guesses as evidence.

Session-wide assignments are deliberately unsupported. An audited session source
must first bind each capture to an exact call with unchanged auth/routing evidence.
Transport-only metadata without an exact identity match cannot enter this sidecar.

## Output contract

Schema version 1 contains `proven`, `partial`, `unresolved`, `coverage` and `parity`.
Every entry keeps the original capture identity and digest. Proven entries add
provider, mode and deduplicated allowlisted provenance. Other entries carry a
reason. Arbitrary evidence fields, endpoint strings and auth values are omitted.
The snapshot and evidence have canonical SHA256 digests; the CLI also records the
snapshot file digest for comparison with external pricing reports.

Different known assertions for one capture conflict. A known assertion combined
with an unresolved or transport-only observation also conflicts. Auth changes
between distinct captures remain separate. Duplicate capture identities, unknown
identities, digest mismatches and invalid known provider/mode pairs fail closed.

Consumers must verify identity and original digest before overlaying a proven
entry, reject conflicting overlays, and retain the original token/cost rollup.
Never append a second usage row. Plan costs are list-price equivalents, not extra
charges. Gateway provider evidence does not establish a billing contract.
An absent override leaves historical attribution unresolved.

Coverage counts include every input capture exactly once. Token parity sums the
unchanged input usage fields; it does not certify app deduplication or pricing.
Price the immutable inventory through the app's existing dated pricing functions
and bind that report to `snapshot_file_sha256`. If the proven set is empty, all
raw inventory cost remains unresolved. The app owner must reconcile thread/hour
rollups and list-price labels after deployment before claiming completion.
