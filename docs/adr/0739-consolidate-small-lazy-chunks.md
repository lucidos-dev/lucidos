# 0739: Consolidate audited small lazy chunks in App shell with a 1950 kB first-paint budget

- **Status**: Accepted
- **Date**: 2026-11-19

## Context

The production graph contained 103 deferred JavaScript chunks below 20,000 bytes.
Each added a separate browser import boundary and another possible fetch failure.
The approved consolidation trades larger initial downloads for fewer small imports.

## Decision

Group the audited small modules with App using explicit manual chunk assignments.
Raise the first-paint budget to 1950 kB, superseding ADR 0436's 1400 kB limit.
Keep the 600 kB entry limit and ADR 0454's growth check and explanation requirement.

Keep large libraries deferred behind loading facades, including markdown parsing,
sanitization, syntax highlighting, QR decoding and Mermaid.
Production builds reject small deferred chunks and unexpected heavy-library promotion.
Development build-watch reports graph violations without withholding refreshed output.

## Rationale

The measured graph fell from 130 JavaScript chunks to 27, with eight deferred chunks
and none below 20,000 bytes. First-paint size grew by approximately 439 kB raw
and 141 kB gzip. Explicit assignments preserve control over large dependencies.

## Consequences

All browser workspaces receive this policy, including packaged and source installations.
The picker and App remain separate render roots, but grouped pairing modules share
the shell download and evaluation cost. Pairing therefore also pays that cost.
Camera access and QR decoding still wait for the scanner mount.

Source dynamic imports preserve mount gates, not module evaluation boundaries
inside a shared output chunk. Module initializers must remain safe during shell load.
This reduces small import boundaries; it does not cure network failures or make
uncached large resources available offline.

## Alternatives considered

Keeping every small chunk preserves smaller initial downloads but retains the
import boundaries targeted by the approved change.
Merging every dependency would also promote heavy libraries into first paint.
Separate picker grouping would depart from the approved inventory's App assignment.
