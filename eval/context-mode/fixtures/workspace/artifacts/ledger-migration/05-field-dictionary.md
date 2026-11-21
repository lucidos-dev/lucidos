# Ledger field dictionary

**Applies to:** `ledger` schema version 0074 | **Updated:** 2026-07-30

Every column in the new ledger, what it means, and where it comes from in
`tally`. This is the reference for anybody writing a query, a report, or a
migration mapping.

Three columns are computed rather than copied. They are listed in section 9
and they are the only place where the new ledger holds a value the old one
never stored.

## 1. How to read this document

Each table gets a section. Each section has a column table and then notes on
the fields that need them.

| Marking | Meaning |
|---|---|
| **PK** | Part of the primary key |
| **NN** | Not null |
| **FK** | Foreign key, target named in the notes |
| **C** | Computed at migration time, see section 9 |
| **D** | Dropped from `tally`, see section 10 |

The `tally` column shows the source. A `-` means the column is new and has no
source. That is either a computed field, or a field the new schema needs and
the old one did not have.

## 2. Conventions

### 2.1 Money

Every monetary value is a `bigint` in the currency's minor unit. There are no
floating point columns anywhere in the ledger and there never will be.

NOK, SEK and EUR all have two decimal places, so a minor unit is one øre, one
öre or one cent. The currency is always stored beside the amount, in the same
row, and no row is meaningful without it.

`tally` stored money as `numeric(18,4)` with four decimal places. The
migration multiplies by 100 and asserts the remainder is zero. Section 10.3
covers the 36 accounts where it was not.

### 2.2 Time

All timestamps are `timestamptz` and all are stored in UTC. The application
never writes a naive timestamp.

Three time columns appear on most tables and they mean different things.

| Column | Meaning |
|---|---|
| `occurred_at` | When the thing happened in the real world |
| `created_at` | When we first wrote the row |
| `updated_at` | When we last changed the row |

`occurred_at` can be earlier than `created_at`, sometimes by days, because a
vendor can replay an old record. `created_at` is never earlier than
`occurred_at` by more than the clock skew allowance of 2 seconds.

### 2.3 Identifiers

Public identifiers are ULIDs with a type prefix, stored as `text`.

| Prefix | Type |
|---|---|
| `acct_` | Account |
| `entry_` | Ledger entry |
| `item_` | Line item |
| `pty_` | Party |
| `rps_` | Vendor replay session |
| `mrc_` | Merchant |

Internal keys are `bigint`. The rule is that anything crossing an API boundary
uses the prefixed ULID, and anything crossing only a join uses the bigint.

### 2.4 Shards

The ledger is 16 shards, numbered 0 to 15. A shard is chosen by hashing the
account id, so every entry for an account is on one shard.

`shard_id` is `smallint` and appears on every table that holds entry data. It
is part of the primary key wherever it appears, because the sequence it pairs
with is per-shard rather than global.

## 3. Table `entries`

The core table. One row per immutable ledger line, 612,981,933 rows at the
migration snapshot.

| Column | Type | Flags | `tally` column | Meaning |
|---|---|---|---|---|
| `shard_id` | smallint | PK NN | - | Placement, section 2.4 |
| `entry_seq` | bigint | PK NN | - C | Position within the shard |
| `entry_id` | text | NN | `entry.public_id` | Public ULID |
| `account_id` | text | NN FK | `entry.account_ref` | Owning account |
| `party_id` | text | NN FK | `entry.counterparty` | The other side |
| `amount_minor` | bigint | NN | `entry.amount` | Signed, in minor units |
| `currency` | char(3) | NN | `entry.ccy` | ISO 4217 |
| `entry_type` | text | NN | `entry.kind` | See section 3.2 |
| `direction` | text | NN | `entry.dr_cr` | `debit` or `credit` |
| `occurred_at` | timestamptz | NN | `entry.value_ts` | Real-world time |
| `created_at` | timestamptz | NN | `entry.created` | Write time |
| `posted_at` | timestamptz | | `entry.posted` | Settlement time, if settled |
| `reversal_of` | text | | `entry.reverses` | Entry this reverses |
| `reversed_by` | text | C | - | Entry that reverses this |
| `batch_id` | text | | `entry.batch` | Vendor batch, if any |
| `vendor_ref` | text | | `entry.ext_ref` | Vendor's own identifier |
| `idempotency_key` | text | | `entry.idem` | Client-supplied, unique per account |
| `metadata` | jsonb | NN | `entry.meta` | Free-form, defaults to `{}` |

### 3.1 On `amount_minor`

Signed. A debit is negative and a credit is positive, from the perspective of
the account named in `account_id`.

`tally` stored the sign in `dr_cr` and kept `amount` unsigned. Both are
carried across: `direction` preserves the original word and `amount_minor`
carries the sign. They are always consistent, and a check constraint enforces
it.

```sql
ALTER TABLE entries ADD CONSTRAINT entries_sign_matches_direction
  CHECK ((direction = 'debit'  AND amount_minor <= 0)
      OR (direction = 'credit' AND amount_minor >= 0));
```

Zero is allowed in both directions. A zero entry is rare and legitimate: it
records that something happened without moving money, such as an
acknowledged reversal of a zero-value correction.

### 3.2 Values of `entry_type`

Nine values. The list is closed and adding to it needs a migration.

| Value | Meaning | Share of rows |
|---|---|---|
| `payment` | A customer payment into the platform | 41.2% |
| `payout` | A settlement out to a seller | 18.7% |
| `fee` | Platform fee taken from a payment | 17.9% |
| `refund` | Money returned to a customer | 8.1% |
| `chargeback` | A forced reversal from the card network | 0.4% |
| `adjustment` | A manual correction, always paired | 1.1% |
| `transfer` | Movement between two platform accounts | 9.8% |
| `fx` | The currency leg of a cross-currency payment | 2.6% |
| `opening` | The migration's opening balance line | 0.2% |

`opening` exists only because of this migration. Each account with a non-zero
balance at the snapshot got one, and no new `opening` entry will ever be
written.

### 3.3 On `reversal_of` and `reversed_by`

`reversal_of` is copied from `tally`. `reversed_by` is computed, and it is one
of the three fields in section 9.

The pair makes the reversal graph walkable in both directions. `tally` could
only walk it forwards, which meant answering "was this reversed" needed a scan
of the whole table.

An entry is reversed at most once. A second reversal of the same entry is
rejected by a partial unique index on `reversal_of`.

### 3.4 On `idempotency_key`

Client-supplied and unique per account, not globally. Two different accounts
may legitimately use the same key.

```sql
CREATE UNIQUE INDEX entries_idem_per_account
  ON entries (account_id, idempotency_key)
  WHERE idempotency_key IS NOT NULL;
```

Roughly 61% of rows carry one. The rest predate the idempotency feature, which
shipped in 2024.

### 3.5 On `metadata`

Free-form JSON, defaulting to `{}` rather than null. Never null, so a query
can always index into it without a guard.

Three keys are conventional and are read by reporting.

| Key | Type | Meaning |
|---|---|---|
| `source` | string | Which system created the entry |
| `trace_id` | string | Distributed trace, for support |
| `legacy` | object | Fields from `tally` with no new home |

The `legacy` object is written by the migration and is the only place a
dropped `tally` column survives. Section 10 lists what goes in it.

## 4. Table `accounts`

One row per account. 41,882,718 rows at the snapshot, of which 41,882,306 are
in scope for parity and 412 are excluded.

| Column | Type | Flags | `tally` column | Meaning |
|---|---|---|---|---|
| `account_id` | text | PK NN | `acct.public_id` | Public ULID |
| `shard_id` | smallint | NN | - | Placement, section 2.4 |
| `merchant_id` | text | NN FK | `acct.merchant` | Owning merchant |
| `account_kind` | text | NN | `acct.type` | See section 4.1 |
| `currency` | char(3) | NN | `acct.ccy` | The account's only currency |
| `status` | text | NN | `acct.state` | See section 4.2 |
| `opened_at` | timestamptz | NN | `acct.opened` | When the account opened |
| `closed_at` | timestamptz | | `acct.closed` | When it closed, if closed |
| `display_name` | text | | `acct.label` | Shown in the console |
| `external_ref` | text | | `acct.partner_ref` | The merchant's own id |
| `balance_minor` | bigint | NN C | - | Cached balance, section 9.2 |
| `balance_as_of` | timestamptz | NN | - | When the cache was computed |
| `metadata` | jsonb | NN | `acct.meta` | Free-form |

### 4.1 Values of `account_kind`

| Value | Meaning | Count |
|---|---|---|
| `merchant_balance` | A seller's spendable balance | 38,441,092 |
| `merchant_reserve` | Held funds, released on a schedule | 3,102,884 |
| `platform_fee` | Where fees accumulate | 16 |
| `platform_settlement` | The bank-facing settlement account | 16 |
| `suspense` | Unattributed money, resolved manually | 188 |
| `clearing` | Transient, nets to zero at settlement | 141 |
| `test` | Created by dry runs and load tests | 47 |

The last three kinds are exactly the parity exclusions. That is not a
coincidence: the exclusion list is generated from `account_kind`, and the
36 closed-with-residue accounts are the only manual entries.

One `platform_fee` and one `platform_settlement` account exist per shard,
which is why both counts are 16.

### 4.2 Values of `status`

| Value | Meaning |
|---|---|
| `active` | Normal |
| `frozen` | No writes, reads normal, set by risk |
| `closing` | Draining to zero, no new credits |
| `closed` | Zero balance, no writes at all |

A closed account keeps its entries forever. Closing is a status change, never
a delete, and nothing in the ledger deletes a row.

## 5. Table `line_items`

Detail rows hanging off an entry. Present on `payment` and `payout` entries,
absent on the rest.

| Column | Type | Flags | `tally` column | Meaning |
|---|---|---|---|---|
| `shard_id` | smallint | PK NN | - | Same shard as the entry |
| `entry_seq` | bigint | PK NN | - | The owning entry |
| `item_index` | smallint | PK NN | `item.pos` | Position, 0 based |
| `item_id` | text | NN | `item.public_id` | Public ULID |
| `sku` | text | | `item.sku` | Merchant's product code |
| `description` | text | | `item.descr` | Free text, up to 200 characters |
| `quantity` | integer | NN | `item.qty` | Always positive |
| `unit_amount_minor` | bigint | NN | `item.unit_price` | Per unit, minor units |
| `tax_rate_bp` | integer | NN | `item.vat_bp` | Basis points, 2500 is 25% |
| `tax_amount_minor` | bigint | NN | `item.vat` | Computed by the merchant |

An entry carries at most 16 line items. The limit comes from the vendor's
record format rather than from anything we need. It is enforced by a trigger
rather than a constraint, because it spans rows.

### 5.1 On `tax_rate_bp`

Basis points, so 2500 means 25%. Integer, because a rate expressed as a
decimal invites a float somewhere downstream.

Norwegian VAT rates in the data are 2500, 1500, 1200 and 0. The 1200 rate
disappears from new data after 2025 and remains in history.

### 5.2 On the sum rule

The line items of an entry sum to the entry's amount, including tax. This is
checked at write time and it was not checked in `tally`.

```sql
SELECT e.shard_id, e.entry_seq
  FROM entries e
  JOIN line_items li USING (shard_id, entry_seq)
 GROUP BY e.shard_id, e.entry_seq, e.amount_minor
HAVING sum(li.quantity * li.unit_amount_minor + li.tax_amount_minor)
       <> abs(e.amount_minor);
```

The migration ran this over the whole source and found 2,204 violations, all
from before 2024. They are carried across unchanged, with
`metadata.legacy.item_sum_mismatch` set to true.

## 6. Table `parties`

The counterparty of an entry. A party is not an account: it is whoever is on
the other side, which is often outside the platform.

| Column | Type | Flags | `tally` column | Meaning |
|---|---|---|---|---|
| `party_id` | text | PK NN | `party.public_id` | Public ULID |
| `party_kind` | text | NN | `party.type` | See below |
| `display_name` | text | NN | `party.name` | For statements |
| `country` | char(2) | | `party.cc` | ISO 3166-1 alpha-2 |
| `created_at` | timestamptz | NN | `party.created` | First seen |

| `party_kind` | Meaning |
|---|---|
| `customer` | An end customer paying a merchant |
| `merchant` | A seller on the platform |
| `platform` | Us, for fees and internal movement |
| `bank` | A settlement bank |
| `network` | A card network, for chargebacks |

`parties` is the smallest table in the ledger at 8.4 million rows, and it is
not sharded. It is replicated to every shard as a read-only copy, because
every entry joins to it.

## 7. Table `batches`

One row per vendor batch. A batch groups records the vendor delivered
together, and it exists so a drain can be replayed idempotently.

| Column | Type | Flags | `tally` column | Meaning |
|---|---|---|---|---|
| `batch_id` | text | PK NN | `batch.public_id` | Vendor's batch id |
| `session_id` | text | NN | - | Replay session that carried it |
| `vendor` | text | NN | `batch.provider` | Always `kestrel` today |
| `record_count` | integer | NN | `batch.n` | Records in the batch |
| `received_at` | timestamptz | NN | `batch.received` | When we got it |
| `acknowledged_at` | timestamptz | | - | When we acknowledged it |
| `applied_at` | timestamptz | | `batch.applied` | When entries were written |

`session_id` and `acknowledged_at` are new. `tally` predates the replay API
and had no concept of a session or an acknowledgement.

A batch is applied exactly once. Re-delivering an applied batch is a no-op,
detected by the primary key, and that is the whole of our idempotency story
for the drain.

## 8. Operational tables

Two tables that hold no ledger data and exist for the migration and its
checks.

### 8.1 `shard_lease`

| Column | Type | Flags | Meaning |
|---|---|---|---|
| `shard_id` | smallint | PK NN | The shard being claimed |
| `owner` | text | NN | Worker id holding the claim |
| `deadline` | timestamptz | NN | When the claim expires |
| `claimed_at` | timestamptz | NN | When the claim was taken |

One row per claimed shard, deleted on release. `shard_lease_history` has the
same columns plus `released_at` and keeps every claim forever.

The history table is small and it is the reason dry run 4 was diagnosed in
eleven minutes rather than a day.

### 8.2 `parity_exclusions`

| Column | Type | Flags | Meaning |
|---|---|---|---|
| `account_id` | text | PK NN | The excluded account |
| `reason` | text | NN | Why, in one line |
| `added_at` | timestamptz | NN | When it was excluded |
| `migration` | text | NN | Which migration file added it |

412 rows, all added by `0060_parity_exclusions.sql` on 2026-06-26. The table is
never written outside a migration, so an addition is always a reviewed change.

## 9. The three computed fields

Three columns hold a value `tally` never stored. Everything else in the new
ledger is either copied from a source column or is operational metadata.

They are listed together because they are the only places where the migration
makes a claim rather than moves a value. A copied column can be wrong only if
the copy is wrong. A computed column can be wrong because the computation is
wrong, which is a different and larger risk.

| Field | Table | Computed from |
|---|---|---|
| `entry_seq` | `entries` | Ordering within the shard |
| `reversed_by` | `entries` | The reverse of `reversal_of` |
| `balance_minor` | `accounts` | Sum of the account's entries |

### 9.1 `entries.entry_seq`

A per-shard monotonic sequence, assigned in the order the entries are written
during the backfill. It is not a global sequence and it is not comparable
across shards.

The ordering is by `(occurred_at, entry_id)` within the shard. `occurred_at`
alone is not unique, and `entry_id` is a ULID, so the pair is both stable and
deterministic. Two runs of the backfill over the same source produce the same
sequence numbers.

That determinism is what makes the migration re-runnable. It is also what
makes duplicate detection possible, because a duplicated range produces the
same sequence numbers twice rather than fresh ones.

```sql
SELECT row_number() OVER (
         PARTITION BY shard_id ORDER BY occurred_at, entry_id
       ) AS entry_seq
  FROM source_entries;
```

New entries written after the migration take their sequence from a per-shard
Postgres sequence, seeded above the migration's highest value. The window
function is used only during the backfill.

### 9.2 `entries.reversed_by`

The inverse edge of `reversal_of`. If entry B has `reversal_of = A`, then
entry A gets `reversed_by = B`.

Computed with a single pass after the backfill:

```sql
UPDATE entries a
   SET reversed_by = b.entry_id
  FROM entries b
 WHERE b.reversal_of = a.entry_id
   AND b.shard_id = a.shard_id;
```

The shard predicate is not redundant. A reversal is always on the same shard
as the entry it reverses, because both belong to the same account. Stating it
explicitly keeps the update inside one shard.

Kept current after the migration by the application, in the same transaction
that writes the reversal. There is no background job.

### 9.3 `accounts.balance_minor`

The sum of the account's entries, cached on the account row. `balance_as_of`
records when the sum was taken.

This is the field the parity check compares, and it is the one worth the most
suspicion. A cached total is wrong the moment anything is missed.

```sql
UPDATE accounts a
   SET balance_minor = coalesce(s.total, 0),
       balance_as_of = now()
  FROM (SELECT account_id, sum(amount_minor) AS total
          FROM entries GROUP BY account_id) s
 WHERE s.account_id = a.account_id;
```

`tally` computed the balance on read, every time, from a full scan of the
account's entries. That was correct and slow, and it is why balance reads on
`tally` degrade with account age.

The new cache is maintained transactionally. Every entry write updates the
account row in the same transaction, so the cache cannot drift from the
entries under normal operation.

The nightly parity run in the first week after cutover exists to prove that
claim, rather than to assume it.

## 10. Dropped `tally` columns

Twenty-three columns exist in `tally` and not in the new ledger. Nineteen are
dropped outright and four are folded into `metadata.legacy`.

### 10.1 Dropped outright

| `tally` column | Why |
|---|---|
| `entry.row_version` | Optimistic locking, replaced by the append-only model |
| `entry.updated` | Entries are immutable, so it never changed |
| `entry.updated_by` | Same |
| `entry.locked` | Row locking, replaced by transactions |
| `entry.import_batch` | An artefact of the 2021 import |
| `entry.legacy_id` | The 2021 import's key, unused since |
| `entry.recon_flag` | Set by a reconciliation job retired in 2023 |
| `entry.recon_at` | Same |
| `acct.balance_dirty` | Cache invalidation flag, no cache to invalidate |
| `acct.last_recalc` | Same |
| `acct.legacy_type` | Superseded by `account_kind` |
| `acct.region` | Always `nordics`, no information in it |
| `party.legacy_id` | The 2021 import's key |
| `party.merged_into` | Party merging removed in 2024 |
| `item.line_hash` | Integrity check for a transport we no longer use |
| `item.source_row` | Import artefact |
| `batch.retry_count` | Retries are now the replay session's concern |
| `batch.error_text` | Same |
| `batch.legacy_provider` | Always the same value |

The test applied to each was simple. Does anything read it, and would anybody
miss it? Nineteen columns failed both.

### 10.2 Folded into `metadata.legacy`

| `tally` column | JSON key | Why kept |
|---|---|---|
| `entry.note` | `note` | Free text an operator wrote, occasionally read |
| `entry.approved_by` | `approved_by` | Audit trail on manual adjustments |
| `entry.approved_at` | `approved_at` | Same |
| `entry.source_system` | `source_system` | Distinguishes six pre-2023 systems |

These four are not queried by anything today. They are kept anyway, because
deleting an audit trail during a migration is hard to reverse and easy to
regret.

They live in `metadata.legacy` rather than in real columns so that nothing new
is built on them.

### 10.3 The rounding residue

`tally` held money as `numeric(18,4)`. The new ledger holds minor units, so
the migration multiplies by 100 and requires a zero remainder.

Thirty-six accounts had a non-zero remainder, all from a rounding change in
2024 that left sub-minor-unit residue on closed accounts. The total residue
across all 36 is under one krone.

They are excluded from parity rather than adjusted. Writing a correcting
entry to a closed account during a migration is worse than carrying a
documented rounding difference.

## 11. Indexes

Eleven indexes across the five data tables. Every one is here because a query
needs it, and the query is named.

| Index | Table | Columns | Serves |
|---|---|---|---|
| `entries_pkey` | `entries` | `(shard_id, entry_seq)` | Primary key |
| `entries_shard_seq_uniq` | `entries` | `(shard_id, entry_seq)` | Duplicate detection |
| `entries_account_time` | `entries` | `(account_id, occurred_at DESC)` | Statement pagination |
| `entries_entry_id` | `entries` | `(entry_id)` | Public id lookup |
| `entries_idem_per_account` | `entries` | `(account_id, idempotency_key)` | Idempotent writes |
| `entries_reversal_of` | `entries` | `(reversal_of)` | Reversal graph |
| `entries_batch` | `entries` | `(batch_id)` | Drain reconciliation |
| `accounts_pkey` | `accounts` | `(account_id)` | Primary key |
| `accounts_merchant` | `accounts` | `(merchant_id, account_kind)` | Console listing |
| `line_items_pkey` | `line_items` | `(shard_id, entry_seq, item_index)` | Primary key |
| `parties_pkey` | `parties` | `(party_id)` | Primary key |

### 11.1 On the two apparently identical indexes

`entries_pkey` and `entries_shard_seq_uniq` cover the same columns and both
exist on purpose.

The primary key is created by the schema and is present from the first row.
The unique index is dropped by `migrate-up` before the bulk load and rebuilt
by `seq-check` afterwards, which is what makes the rebuild a duplicate
detector.

Dropping a primary key during a load would be a much larger change, because
foreign keys depend on it. Dropping a separate unique index costs nothing.

The redundancy is deliberate and it is documented here so that a future
cleanup does not quietly remove one of them.

### 11.2 What has no index

`metadata` has no GIN index. Nothing queries into it today, and adding one
speculatively over 613 million rows is expensive.

`occurred_at` has no standalone index. Every query that filters on time also
filters on an account, so `entries_account_time` covers it.

`vendor_ref` has no index. Vendor lookups go through `batch_id`, which does.

## 12. Constraints

Beyond the primary keys and the unique indexes above.

| Constraint | Table | Rule |
|---|---|---|
| `entries_sign_matches_direction` | `entries` | Section 3.1 |
| `entries_currency_matches_account` | `entries` | Entry currency equals the account's |
| `entries_amount_bounds` | `entries` | Absolute value under 10^15 minor units |
| `entries_occurred_not_future` | `entries` | `occurred_at` at most 2 seconds ahead |
| `accounts_closed_is_zero` | `accounts` | A closed account has a zero balance |
| `line_items_quantity_positive` | `line_items` | Quantity is at least 1 |
| `line_items_tax_rate_bounds` | `line_items` | Between 0 and 10000 basis points |
| `batches_count_positive` | `batches` | A batch has at least one record |

### 12.1 On `entries_currency_matches_account`

An account holds one currency and every entry on it uses that currency. A
cross-currency payment is two entries on two accounts plus an `fx` entry, not
one entry with two currencies.

`tally` did not enforce this and had 118 violations, all from 2022. They were
corrected in the source before the migration rather than carried across.

### 12.2 On `accounts_closed_is_zero`

Enforced as a check constraint, which means an account cannot be closed while
it holds money.

The 36 residue accounts from section 10.3 are the exception. They are closed
and hold a sub-minor-unit residue that rounds to zero in minor units, so the
constraint sees a zero and is satisfied.

## 13. Views

Four views. They exist so that common questions have one right answer rather
than five nearly-right queries.

### 13.1 `account_statement`

An account's entries with the running balance, ordered newest first.

```sql
CREATE VIEW account_statement AS
SELECT e.account_id,
       e.entry_id,
       e.occurred_at,
       e.entry_type,
       e.amount_minor,
       e.currency,
       sum(e.amount_minor) OVER (
         PARTITION BY e.account_id ORDER BY e.occurred_at, e.entry_id
       ) AS running_balance_minor
  FROM entries e;
```

Always filter by `account_id` when selecting from it. Without that filter the
window runs over every account on the shard.

### 13.2 `seq_gap_view`

Missing sequence numbers per shard. This is the view the sequence-gap check
reads.

```sql
CREATE VIEW seq_gap_view AS
SELECT shard_id,
       entry_seq + 1 AS gap_start,
       next_seq - 1  AS gap_end
  FROM (SELECT shard_id, entry_seq,
               lead(entry_seq) OVER (PARTITION BY shard_id
                                     ORDER BY entry_seq) AS next_seq
          FROM entries) t
 WHERE next_seq > entry_seq + 1;
```

Zero rows is the healthy state. A row names a shard and a range, which is
enough to find what the drain missed.

### 13.3 `daily_settlement`

Fees and payouts per merchant per day, in the merchant's currency. Read by the
finance dashboard every morning.

### 13.4 `open_reversals`

Entries with `reversal_of` set and no matching original, which should always
be empty. It is checked nightly and it has never returned a row.

## 14. Common queries

Five queries that people write often, written once correctly.

### 14.1 An account's balance

```sql
SELECT balance_minor, currency, balance_as_of
  FROM accounts WHERE account_id = $1;
```

Use the cached balance. Do not sum the entries. Summing is correct and it is
three orders of magnitude slower on an old account.

### 14.2 The last 50 entries on an account

```sql
SELECT entry_id, occurred_at, entry_type, amount_minor, currency
  FROM entries
 WHERE account_id = $1
 ORDER BY occurred_at DESC, entry_id DESC
 LIMIT 50;
```

The ordering matches `entries_account_time`, so this is an index scan.

### 14.3 Everything in one vendor batch

```sql
SELECT e.entry_id, e.amount_minor, e.currency, e.vendor_ref
  FROM entries e
 WHERE e.batch_id = $1
 ORDER BY e.entry_seq;
```

### 14.4 Was this entry reversed

```sql
SELECT reversed_by FROM entries
 WHERE shard_id = $1 AND entry_seq = $2;
```

One column read. On `tally` the same question needed a scan, which is why
`reversed_by` is computed at all.

### 14.5 A merchant's accounts

```sql
SELECT account_id, account_kind, currency, balance_minor, status
  FROM accounts
 WHERE merchant_id = $1
 ORDER BY account_kind;
```

## 15. What changes for API consumers

The public API keeps its shape. Five things change underneath it and two are
visible.

| Change | Visible to consumers |
|---|---|
| Balance is cached rather than summed | Yes, reads get much faster |
| Money is minor units internally | No, the API already used minor units |
| `entry_seq` exists | No, it is not exposed |
| Dropped columns | No, none were exposed |
| Reversal is walkable both ways | Yes, a new `reversed_by` field |

### 15.1 The new `reversed_by` field

`GET /v1/entries/{id}` gains one field.

```json
{
  "entry_id": "entry_01J9F2K8Q4X7M3N0P5R8T2V6W1",
  "account_id": "acct_01J8QK4M2P0001",
  "amount_minor": -4200,
  "currency": "NOK",
  "entry_type": "refund",
  "reversal_of": null,
  "reversed_by": "entry_01J9F2K8Q4X7M3N0P5R8T2V6W9"
}
```

It is null on almost every entry. It is additive, so no consumer breaks.

### 15.2 Balance read latency

Measured on dry run 6 against the shadow database, over 10,000 sampled
accounts.

| Percentile | `tally` | `ledger` |
|---|---|---|
| p50 | 41 ms | 2 ms |
| p90 | 187 ms | 3 ms |
| p99 | 1,204 ms | 6 ms |
| p99.9 | 4,880 ms | 11 ms |

The tail is the point. An account with two million entries took nearly five
seconds to read a balance on `tally`. That is not an unusual account for a
merchant who has been on the platform since 2019.

## 16. Data volumes

Measured on the production shadow at the 2026-07-28 snapshot. Useful when
sizing a query or deciding whether something belongs in a report.

| Table | Rows | Heap | Indexes | Total |
|---|---|---|---|---|
| `entries` | 612,981,933 | 218 GB | 141 GB | 359 GB |
| `line_items` | 388,204,117 | 74 GB | 31 GB | 105 GB |
| `accounts` | 41,882,718 | 11 GB | 6 GB | 17 GB |
| `parties` | 8,411,092 | 1.4 GB | 0.9 GB | 2.3 GB |
| `batches` | 2,118,447 | 0.3 GB | 0.1 GB | 0.4 GB |
| **Total** | | **305 GB** | **179 GB** | **484 GB** |

Spread across sixteen shards, that is about 30 GB per shard. Each shard is a
separate database on the same cluster, and a shard is small enough to restore
alone.

### 16.1 Growth

The ledger grows by roughly 1.9 million entries per day, and the rate has
risen 34% year on year. At that rate `entries` passes a billion rows in the
first half of 2028.

Nothing in the schema assumes a size, and the sharding exists precisely so
that growth is a capacity question rather than a design one.

### 16.2 What the indexes cost

Indexes are 37% of the total, and `entries_account_time` alone is 68 GB. It is
the largest single object in the ledger and it serves the most common query in
the product.

That is a good trade and it is worth restating whenever somebody proposes a
new index. An index over `entries` costs tens of gigabytes, so it needs a
named query and a measurement.

## Appendix A: reverse mapping

The same mapping read from the `tally` side, for anybody porting a query. Only
columns that exist in both are listed.

| `tally` | `ledger` |
|---|---|
| `entry.public_id` | `entries.entry_id` |
| `entry.account_ref` | `entries.account_id` |
| `entry.counterparty` | `entries.party_id` |
| `entry.amount` + `entry.dr_cr` | `entries.amount_minor` (signed) |
| `entry.ccy` | `entries.currency` |
| `entry.kind` | `entries.entry_type` |
| `entry.dr_cr` | `entries.direction` |
| `entry.value_ts` | `entries.occurred_at` |
| `entry.created` | `entries.created_at` |
| `entry.posted` | `entries.posted_at` |
| `entry.reverses` | `entries.reversal_of` |
| `entry.batch` | `entries.batch_id` |
| `entry.ext_ref` | `entries.vendor_ref` |
| `entry.idem` | `entries.idempotency_key` |
| `entry.meta` | `entries.metadata` |
| `acct.public_id` | `accounts.account_id` |
| `acct.merchant` | `accounts.merchant_id` |
| `acct.type` | `accounts.account_kind` |
| `acct.ccy` | `accounts.currency` |
| `acct.state` | `accounts.status` |
| `acct.opened` | `accounts.opened_at` |
| `acct.closed` | `accounts.closed_at` |
| `acct.label` | `accounts.display_name` |
| `acct.partner_ref` | `accounts.external_ref` |
| `acct.meta` | `accounts.metadata` |
| `item.pos` | `line_items.item_index` |
| `item.public_id` | `line_items.item_id` |
| `item.sku` | `line_items.sku` |
| `item.descr` | `line_items.description` |
| `item.qty` | `line_items.quantity` |
| `item.unit_price` | `line_items.unit_amount_minor` |
| `item.vat_bp` | `line_items.tax_rate_bp` |
| `item.vat` | `line_items.tax_amount_minor` |
| `party.public_id` | `parties.party_id` |
| `party.type` | `parties.party_kind` |
| `party.name` | `parties.display_name` |
| `party.cc` | `parties.country` |
| `party.created` | `parties.created_at` |
| `batch.public_id` | `batches.batch_id` |
| `batch.provider` | `batches.vendor` |
| `batch.n` | `batches.record_count` |
| `batch.received` | `batches.received_at` |
| `batch.applied` | `batches.applied_at` |

Two rows in that table map one source column to two targets. `entry.amount`
and `entry.dr_cr` together become a signed `amount_minor`, and `entry.dr_cr`
also survives as `direction`.

## Appendix B: one payment, end to end

A 420.00 NOK payment from a customer to a merchant, with a 12.60 NOK platform
fee. This is what the migration produces from one `tally` payment.

### B.1 The rows

```
entries
  shard_id            7
  entry_seq           31882044
  entry_id            entry_01J8QK4M2P0001XYZ
  account_id          acct_01J8QK4M2P0001
  party_id            pty_01J7A2B3C4D5E6F7
  amount_minor        42000
  currency            NOK
  entry_type          payment
  direction           credit
  occurred_at         2026-03-14 09:41:22+00
  created_at          2026-03-14 09:41:22+00
  posted_at           2026-03-16 04:00:11+00
  reversal_of         (null)
  reversed_by         (null)
  batch_id            batch_01J7Z9Y8X7W6
  vendor_ref          KC-88213-4471
  idempotency_key     chk_9f2b41
  metadata            {"source": "checkout", "trace_id": "b41f..."}

entries
  shard_id            7
  entry_seq           31882045
  entry_id            entry_01J8QK4M2P0002ABC
  account_id          acct_01J8QK4M2P0001
  party_id            pty_platform_nordics
  amount_minor        -1260
  currency            NOK
  entry_type          fee
  direction           debit
  occurred_at         2026-03-14 09:41:22+00
  ...

line_items (3 rows on the payment)
  (7, 31882044, 0)  sku SKU-1188  qty 2  unit 15000  vat_bp 2500  vat 7500
  (7, 31882044, 1)  sku SKU-2043  qty 1  unit  9000  vat_bp 2500  vat 2250
  (7, 31882044, 2)  sku SHIP-STD  qty 1  unit  2400  vat_bp 2500  vat  600
```

### B.2 The arithmetic

Line items: `2 × 15000 + 7500` is 37500, `9000 + 2250` is 11250, and
`2400 + 600` is 3000. The three sum to 51750.

That does not equal the entry's 42000, and it should not. The line items carry
the gross basket including tax, and the entry carries the net settlement after
the marketplace discount recorded in the batch.

This is the one case where section 5.2's sum rule does not apply. The rule is
therefore checked only on entries with no discount recorded.

The query in 5.2 is written loosely for readability. The real version carries
that predicate.

### B.3 What the merchant sees

A balance movement of 42000 minus 1260, which is 40740 minor units, or 407.40
NOK. Two lines on the statement, both timestamped 09:41:22, with the fee
immediately after the payment.

## Appendix C: questions

**Why is `entry_seq` per shard rather than global?**

A global sequence needs coordination across sixteen shards on every write.
Per-shard costs nothing and the sequence is only ever used within a shard.

**Can I join `entries` across shards?**

Yes, and it is slow. Every cross-shard query in the codebase today is a
report, and reports run against the nightly export rather than the ledger.

**Why keep `direction` when the sign already carries it?**

Because the word is what the accounting team uses, and because a query that
filters on `direction = 'debit'` is clearer than one filtering on a sign.

**Is `metadata` safe to write to from an application?**

Yes, and keep it small. It is not indexed and it is copied into every export.

**What happens to `entry_seq` if we re-run the migration?**

The same values, because the ordering is deterministic. That is by design and
section 9.1 explains it.

**Where did the 2021 import's identifiers go?**

Dropped. `entry.legacy_id` and `party.legacy_id` have not been read since
2023, and the import they came from is documented elsewhere.

## Appendix D: schema history

| Version | Date | Change |
|---|---|---|
| 0001 | 2026-02-11 | Initial schema, four tables |
| 0018 | 2026-03-30 | Sharding, `shard_id` on every table |
| 0031 | 2026-04-22 | `line_items` split out of `entries.metadata` |
| 0044 | 2026-05-19 | `reversed_by` added |
| 0052 | 2026-06-02 | `batches` and the replay session id |
| 0060 | 2026-06-26 | `parity_exclusions` |
| 0067 | 2026-07-16 | `seq_gap_view` |
| 0074 | 2026-07-30 | Constraint tightening, current |

## Appendix E: terms

| Term | Meaning here |
|---|---|
| Basis point | One hundredth of a percent, so 2500 is 25% |
| Entry | One immutable ledger line |
| Line item | Detail row under a payment or payout |
| Minor unit | The smallest unit of a currency, one øre for NOK |
| Party | The other side of an entry, often outside the platform |
| Shard | One of sixteen partitions, chosen by account id |
| ULID | A sortable 26 character identifier |

## Appendix F: the nightly export

Reporting does not query the ledger. It reads a nightly export, and the export
is the reason several columns exist in the shape they do.

### F.1 What it contains

One Parquet file per shard per day, written at 03:00 UTC, holding every entry
whose `created_at` falls in the previous day.

| Column in the export | Source |
|---|---|
| `entry_id` | `entries.entry_id` |
| `account_id` | `entries.account_id` |
| `merchant_id` | `accounts.merchant_id`, joined |
| `amount_minor` | `entries.amount_minor` |
| `currency` | `entries.currency` |
| `entry_type` | `entries.entry_type` |
| `occurred_at` | `entries.occurred_at` |
| `posted_at` | `entries.posted_at` |
| `tax_amount_minor` | Sum over `line_items`, or zero |

Nine columns. Everything else stays in the ledger, and a request to add a
tenth is a conversation about why reporting needs it.

### F.2 Why an export rather than a replica

A replica would let reporting write any query it liked, including one that
scans 613 million rows during the working day.

The export is a boundary. It costs a day of freshness and it buys a ledger
whose latency does not depend on what an analyst is doing.

### F.3 Reconciling the export

The export writes a manifest with a row count and a checksum per shard. A
nightly job compares the count against the ledger and alerts on any
difference.

The comparison has never differed. It runs anyway, because an export nobody
checks is a report nobody should trust.

## Closing note

The dictionary is the boring document and it is the one people actually open.
Keep it accurate: a wrong line here becomes a wrong query, and a wrong query
about money becomes a support ticket.

When a column changes, change this file in the same commit. A dictionary that
lags the schema is worse than no dictionary, because it is trusted.
