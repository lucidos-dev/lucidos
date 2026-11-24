# 0722: Keep ordinary API reads out of the service worker

- **Status**: Accepted
- **Date**: 2026-11-16

## Context

An iPhone PWA repeatedly lost its page process after resume.
WebKit can end a shared page and worker process when a worker does not stop.
The cause remains unproven.

## Decision

Ordinary API GETs bypass the worker.
The page and SDK retry a transport failure once.
Immutable blobs and the marked worker health probe stay in the worker.

## Rationale

A worker-owned read can reach WebKit's fetch timeout and worker termination path.
Moving the retry preserves recovery from a stale connection without that worker dependency.
This applies to all browsers, workspaces and app frames.

## Consequences

Caller cancellation and restore generation guards still block a retry.
HTTP verdicts, body errors, mutations, streams and plain health probes keep their existing behavior.
Resume checks update the worker only after a served build mismatch.
Worker diagnostics and phone evidence must test this experiment.

## Alternatives considered

Keeping blanket interception preserves the suspected trigger.
Removing retries drops stale connection recovery.
Removing the worker also removes push and shell caching.
