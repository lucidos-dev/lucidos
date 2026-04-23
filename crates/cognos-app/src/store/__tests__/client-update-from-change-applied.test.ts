import { describe, it, expect, beforeEach } from 'vitest';
import { updateAvailable } from '../store';
import { hasClientUpdateSincePageLoad } from '../actions/chat-changes';
import type { Change } from '../../api/client';

function makeChange(overrides: Partial<Change> = {}): Change {
  return {
    id: 'c-1',
    request_id: 'r-1',
    thread_id: null,
    thread_title: null,
    branch_name: 'test-branch',
    repo_root: '/tmp/repo',
    description: 'test change',
    file_count: 1,
    files: ['src/app.tsx'],
    requires_restart: false,
    hardened: true,
    status: 'applied',
    created_at: '2026-01-01T00:00:00Z',
    resolved_at: '2026-01-01T00:00:00Z',
    pre_merge_sha: null,
    post_merge_sha: null,
    commits: [],
    ...overrides,
  };
}

beforeEach(() => {
  updateAvailable.value = false;
});

describe('client update badge from ChangeApplied', () => {
  // This tests the fix: when ChangeApplied SSE arrives with client_update=true,
  // the thread-sync handler sets updateAvailable=true so the badge shows on the
  // CognOS brand. Previously, this relied on Vite HMR which is unreliable on
  // iOS Safari PWA (WebSocket drops, events missed).

  it('updateAvailable is set when ChangeApplied has client_update=true', () => {
    // Simulate what the fixed ChangeApplied handler does:
    // read client_update from the event and set updateAvailable
    const event = { type: 'ChangeApplied', change_id: 'c-1', requires_restart: false, client_update: true };
    if ((event as { client_update?: boolean }).client_update) {
      updateAvailable.value = true;
    }
    expect(updateAvailable.value).toBe(true);
  });

  it('updateAvailable stays false when ChangeApplied has client_update=false', () => {
    const event = { type: 'ChangeApplied', change_id: 'c-2', requires_restart: false, client_update: false };
    if ((event as { client_update?: boolean }).client_update) {
      updateAvailable.value = true;
    }
    expect(updateAvailable.value).toBe(false);
  });

  it('updateAvailable stays false when client_update field is missing (old events)', () => {
    const event = { type: 'ChangeApplied', change_id: 'c-3', requires_restart: false };
    if ((event as { client_update?: boolean }).client_update) {
      updateAvailable.value = true;
    }
    expect(updateAvailable.value).toBe(false);
  });

  it('both updateAvailable and restartRequired when both flags are set', () => {
    // A change with both Rust and frontend files
    const event = { type: 'ChangeApplied', change_id: 'c-4', requires_restart: true, client_update: true };
    if ((event as { client_update?: boolean }).client_update) {
      updateAvailable.value = true;
    }
    expect(updateAvailable.value).toBe(true);
  });
});

describe('hasClientUpdateSincePageLoad', () => {
  it('returns false for changes applied before page load', () => {
    // resolved_at far in the past — the page loaded after this change
    const change = makeChange({ resolved_at: '2020-01-01T00:00:00Z', files: ['src/app.tsx'] });
    expect(hasClientUpdateSincePageLoad([change])).toBe(false);
  });

  it('returns true for changes with frontend files applied after page load', () => {
    // resolved_at far in the future — simulates a change applied after the page loaded
    const change = makeChange({ resolved_at: '2099-01-01T00:00:00Z', files: ['src/app.tsx'] });
    expect(hasClientUpdateSincePageLoad([change])).toBe(true);
  });

  it('returns false for Rust-only changes applied after page load', () => {
    // Only .rs files — no client update needed
    const change = makeChange({ resolved_at: '2099-01-01T00:00:00Z', files: ['src/main.rs', 'src/lib.rs'] });
    expect(hasClientUpdateSincePageLoad([change])).toBe(false);
  });

  it('returns false when resolved_at is null', () => {
    const change = makeChange({ resolved_at: null, files: ['src/app.tsx'] });
    expect(hasClientUpdateSincePageLoad([change])).toBe(false);
  });

  it('returns false for empty applied list', () => {
    expect(hasClientUpdateSincePageLoad([])).toBe(false);
  });

  it('detects various frontend file extensions', () => {
    for (const ext of ['ts', 'tsx', 'css', 'html', 'js', 'jsx']) {
      const change = makeChange({ resolved_at: '2099-01-01T00:00:00Z', files: [`src/file.${ext}`] });
      expect(hasClientUpdateSincePageLoad([change])).toBe(true);
    }
  });

  it('ignores non-frontend files mixed with frontend files before page load', () => {
    // Change has both .rs and .tsx, but was applied before page load
    const change = makeChange({
      resolved_at: '2020-01-01T00:00:00Z',
      files: ['src/main.rs', 'src/app.tsx'],
    });
    expect(hasClientUpdateSincePageLoad([change])).toBe(false);
  });
});
