# 0243: The plugin catalog is a cached projection, served stale by default

- **Status**: Accepted
- **Date**: 2026-09-22

## Context

Opening Plugins or Settings → Marketplaces took seconds. `GET /api/v1/plugins/catalog`
ran a full marketplace scan per request: a shallow git clone of every registered
repo into a temp directory, a walk for `manifest.toml` files, then the clone
deleted. Nothing cached the result anywhere.

Three things made it worse than the cost implies.

- The scheduler already ran the identical `scan_catalog` at startup, every five
  minutes, and after a registration. It read the update candidates out and
  dropped the rest.
- Settings → Marketplaces lists names and URLs, which come from one small
  registry file. It waited on the clones only because it shared a loadable with
  the Store.
- Opening Apps paid the clone too, to label a row's provenance.

The earlier plan `2026-09-15-marketplace-add-must-not-wait-on-the-catalog-scan.md`
deferred this as "the deeper fix for scan cost".

## Decision

The catalog is a **cached projection**. The scheduler's scan writes
`.lucidos/plugin-catalog.json`, and the HTTP route serves that file with no git
work at all. The route never blocks on a scan: a stale or absent cache starts one
in the background and the request answers anyway.

The response carries `scanned_at`, `scanning` and `scan_error` beside the rows.
The panel shows that age rather than pretending the list is live, and renders a
skeleton only when it holds nothing.

The **marketplace list is not cached**. It is read live from the registry on
every request.

## Rationale

The cache belongs in the engine rather than the client, for three reasons. The
background scan already produces it. Every client benefits, including a freshly
evicted iOS PWA and a second device. And a client cache would repeat the merge
and staleness logic in each surface, then drift between devices.

Leaving the marketplace list live is what makes a stale cache safe. A rename
shows at once with no scan behind it. Cached plugins are filtered to the ids the
registry still holds, so a removed marketplace can never offer installable ones.
That also removes the coupling that made Settings wait on clones.

Both scan events are **transient**. A scan fires at least every five minutes, so
persisting one would add a heartbeat row to the events table and tell an audit
nothing. What a marketplace *is* stays audited by `PluginMarketplaceRegistered`
and `PluginMarketplaceRemoved`, which are persisted.

## Consequences

- The Plugins panel and Settings paint immediately, from a list up to five
  minutes old, with its age on screen.
- `.lucidos/plugin-catalog.json` is rebuildable by definition. A missing,
  truncated or invalid file reads as empty, costing one scan.
- A scan is queued synchronously (`note_scan_queued`) before the task that runs
  it is spawned. Without that, a refetch racing the spawn reports "not scanning"
  for a scan it just queued, which reads as "No plugins found".
- **Install state is re-stamped live on every response**
  (`apply_installed_state_to_catalog`). It is the one part of a row a cache
  cannot hold: a plugin installed since the last scan would otherwise keep
  offering its Install button for the rest of the TTL. Every install-derived
  field is set in `apply_installed_state` and nowhere else, so a scan and a
  re-served cache cannot disagree.
- **A registry change that lands mid-scan queues a trailing pass**
  (`ScanCause::RegistryChanged`). The running scan may have read the registry
  before the write. Joining it would republish pre-change contents under a fresh
  `scanned_at`. That suppresses the next page-open rescan for the whole TTL, so
  a newly registered marketplace would stay invisible for five minutes. Mirrors
  `refreshPluginCatalogAfterMutation` on the frontend.
- `shallow_clone` still has no timeout, so a wedged clone would hold the
  single-flight slot for good. Two bounds answer it, both keyed on
  `SCAN_STALL_CUTOFF_SECS`. The slot is **taken over** past the cutoff, so
  scanning resumes rather than stopping for the life of the engine. And the cue
  stops counting, so the panel falls back to its real age rather than an endless
  "Updating…". The wedged task still holds a blocking thread until its own
  socket gives up, which is the part only a clone timeout can fix.
- **Cache transitions are serialized by one process-wide lock.** The atomic
  rename makes a write all-or-nothing and says nothing about two writers. A
  page open stamps `scan_started_at` from an HTTP thread while a scan task is
  finishing. Without the lock that read-modify-write reverts the finished
  scan's rows and timestamp.
- One scan is no cheaper. This moves it off the request path; it does not make
  cloning faster.

## Alternatives considered

**A persistent clone mirror** (fetch into a kept checkout instead of cloning
fresh). It is what the earlier plan imagined, and it would make each scan cheaper
rather than merely off-path. Rejected as far larger: it needs a mirror per
marketplace plus its own staleness and corruption handling. It also still leaves
a scan on the request path unless a cache is added anyway. Caching the RESULT is
a few hundred bytes of JSON and needs none of that.

**A client-side cache** (localStorage or IndexedDB). Cheaper to build, and it
would survive a reload. Rejected because the merge and staleness rules would live
in the frontend, repeated per surface, with each device holding its own answer.
Nothing in the app persists fetched data today, and starting with the most
derived value was the wrong place.

**Caching the marketplace list too.** It would make the response one file read
instead of two. Rejected because the registry read costs microseconds and the
list is the source of truth: caching it would put a rename minutes behind, on the
one surface whose whole job is showing it.

**Persisting the scan events.** It would let a trigger subscribe to a scan
landing. Rejected for the heartbeat cost above. Should a use for subscribing
appear, the right move is a persisted event for the OUTCOME: an update became
available. Never one per tick of a timer.

**Blocking the first request on a cold workspace**, so the very first open shows
plugins rather than an empty list with a cue. Rejected because it reintroduces
the slow path at the exact moment the user is least patient, and the scan
announces itself moments later anyway.
