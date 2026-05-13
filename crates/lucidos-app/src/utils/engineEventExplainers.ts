import type { EngineReason } from '../store/thread-events';

/** Why the engine acted, for the route popover. Returns null for `scheduler`
 *  because that variant has its own richer renderer (links to the trigger).
 *  The popover heading ("Why the engine acted") is owned by the renderer. */
export function describeEngineReason(reason: EngineReason): string | null {
  switch (reason.kind) {
    case 'continuation_started':
    case 'session_recovered':
      return 'CC sessions running when the engine stops are auto-resumed when it restarts. This event marks the resume.';
    case 'orphan_recovery':
      return 'After a restart, the engine resumes orphaned threads where work was in flight.';
    case 'harden_retrigger':
      return 'The engine re-triggers `/harden` when the hardening marker is missing or stale, so changes aren’t applied unhardened.';
    case 'stale_session':
      return 'The engine cleans up CC sessions that became stale (process gone, marker missing). This event marks the cleanup.';
    case 'merge_conflict':
      return 'The engine detected a conflict when merging changes from main into your branch. We need to resolve it before applying.';
    case 'missing_hardening':
      return 'Hardening (`/harden`) must run before changes are applied. The engine queues it automatically when the marker is missing.';
    case 'scheduler':
      return null;
  }
}
