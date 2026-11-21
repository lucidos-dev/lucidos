# 0744: Store attachment bytes once per device database

- **Status**: Accepted
- **Date**: 2026-11-19

## Context

A landed upload can stay attached across many page loads. Each adopted owner
created another full copy in pending-upload history. Restore admission loaded
the history on every stream connection. The restore journal also embedded its
own copy of each revision in the observation it read on every hello. An iPhone
system log shows a WebContent process killed at its 2 GB limit. The growth
path exists across browsers and desktop webviews.

## Decision

Give pending-upload history and the restore journal separate IndexedDB byte
stores. Keep all owner, generation and landed-hash revisions as metadata.
Keys include the original local ID, thread, name, MIME, size and creation time
plus a payload variant. Equal bytes within one identity share a key.

A hello lists upload metadata and fetches pending bytes when it first retains a
source in the journal. Reconstruction and explicit review copies read the
journal bytes for the selected draft. Preview uses the journal's metadata.

## Rationale

Revisions track ownership and intent. Physical copies add no information when
their bytes agree. Reusing a key requires byte equality. An archived landed
hash alone cannot establish that equality. Local keys let review recognize
shared attachment content without reading it.

## Consequences

A reused identity with new conflicting bytes aborts the write. Upgrades
preserve distinct legacy payloads under variant keys. Pending history upgrades
by cursor. The old journal has one packed observation, so its upgrade reads
that record once and processes its attachments in sequence.

Each database writes its byte store and metadata in one transaction. Pending
deletion removes the history for one local ID and the byte keys it owns.
Journal copies remain available after pending deletion. The upgrade keeps
generation markers, drafts and recovery destinations.

## Alternatives considered

- Keep history payloads embedded and limit the history listing. That leaves
  disk growth and eventually makes some revisions unreachable.
- Share a byte store across both databases. Pending cleanup could invalidate a
  journal reference, since IndexedDB transactions cover one database.
- Pick one payload for conflicting legacy identities. That would discard
  retained bytes. Variant keys preserve both originals.
