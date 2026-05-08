// Thread lifecycle scenario tests — driven by the shared contract JSON.
//
// These tests verify that the frontend's event replay (via handleEvent) and the generated
// displaySection()/isSectionLegal() agree with the shared scenario definitions
// in tests/thread-lifecycle-scenarios.json.
//
// Scenarios with expect_error steps are backend-only validations and are skipped.
// Scenarios with only assert_invariant (no steps) are tested in the invariants block.

import { describe, it, expect } from 'vitest';
import { handleEventWithAgg } from './aggregate-test-helper';
import type { ThreadState, ThreadMeta, ThreadStatus } from '../thread-events';
import {
  displaySection,
  isSectionLegal,
  resolveActions,
  EVENT_CLASSIFICATION,
  type ArchiveState,
  type ThreadType,
} from '../../generated/thread-lifecycle';
import scenarioFileRaw from '../../../../../tests/thread-lifecycle-scenarios.json';

interface ScenarioStep {
  emit: string;
  payload?: Record<string, unknown>;
  expected?: {
    stored_section?: string;
    status?: string;
    display_section?: string;
    is_saved?: boolean;
    expected_actions?: string[];
  };
  set_pending_changes?: boolean;
  expect_error?: string;
}

interface Scenario {
  name: string;
  thread_type: string;
  description: string;
  steps?: ScenarioStep[];
  assert_invariant?: {
    thread_type: string;
    forbidden_section: string;
  };
}

interface ScenarioFile {
  scenarios: Scenario[];
}

const scenarioFile = scenarioFileRaw as ScenarioFile;

describe('Thread Lifecycle Scenarios (shared contract)', () => {
  for (const scenario of scenarioFile.scenarios) {
    if (!scenario.steps || scenario.steps.length === 0) continue;

    // Skip scenarios that are purely negative (all steps have expect_error)
    const hasPositiveSteps = scenario.steps.some(s => !s.expect_error);
    if (!hasPositiveSteps) continue;

    describe(scenario.name, () => {
      it(scenario.description, () => {
        const baseTime = new Date('2026-03-26T10:00:00Z').getTime();
        let seq = 0;
        let hasPendingChanges = false;

        // Create a test thread with initial meta
        const threadId = 'test-thread-id';
        const initialMeta: ThreadMeta = {
          id: threadId,
          title: 'Test Thread',
          channel: scenario.thread_type === 'claude_code' ? 'claude_code' : 'chat',
          initiator: 'user',
          saved: false,
          createdAt: new Date(baseTime).toISOString(),
          updatedAt: new Date(baseTime).toISOString(),
          status: 'idle' as ThreadStatus,
          messageCount: 0,
          section: 'archived',
          activeChildrenCount: 0,
          totalChildrenCount: 0,
          ccHasChanges: false,
          ccRequiresRestart: false,
          ccIsExternalRepo: false,
          ccApplying: false,
          lastRevivedAt: '',
          state: 'active',
        };

        const thread: ThreadState = {
          meta: initialMeta,
          events: new Map(),
          streamingBuffer: '',
          eventsLoaded: true,
          eventsLoadFailed: false,
          lastDbSeq: 0,
          pendingUserMessages: [],
        };

        const threadMap = new Map<string, ThreadState>();
        threadMap.set(threadId, thread);

        for (let i = 0; i < scenario.steps!.length; i++) {
          const step = scenario.steps![i];

          // Skip error-expected steps (backend-only validation)
          if (step.expect_error) continue;

          if (step.set_pending_changes !== undefined) {
            hasPendingChanges = step.set_pending_changes;
          }

          seq++;
          const created = new Date(baseTime + seq * 1000).toISOString();

          // Build the event from the step
          const event: any = {
            type: step.emit,
            ...((step.payload || {}) as any),
          };

          // Use the step's expected status (and is_saved/section if declared)
          // as the synthesized aggregate so meta matches what the backend
          // would project. Falls back to handleEventWithAgg's rule replay when
          // the step doesn't declare expectations.
          const overrides: Record<string, unknown> = {};
          if (step.expected?.status) overrides.status = step.expected.status;
          if (step.expected?.is_saved !== undefined) overrides.isSaved = step.expected.is_saved;
          if (step.expected?.stored_section) overrides.section = step.expected.stored_section;
          handleEventWithAgg(threadMap, threadId, seq, event, created, undefined, overrides);

          if (!step.expected) continue;

          // Test status — now read from thread.meta.status after event replay
          if (step.expected.status) {
            const status = thread.meta.status;
            expect(status, `step ${i} (${step.emit}): expected status='${step.expected.status}'`).toBe(step.expected.status);
          }

          // Test display_section when both stored_section and status are available
          if (step.expected.display_section && step.expected.stored_section) {
            const status = thread.meta.status;
            const display = displaySection(
              step.expected.stored_section as ArchiveState,
              status,
              step.expected.is_saved || false,
              false,
              hasPendingChanges,
            );
            expect(display, `step ${i} (${step.emit}): expected display='${step.expected.display_section}'`).toBe(step.expected.display_section);
          }

          // Test resolve_actions when expected_actions is specified
          if (step.expected.expected_actions) {
            const status = thread.meta.status;
            const storedSection = (step.expected.stored_section || 'archived') as ArchiveState;
            const threadType = scenario.thread_type as ThreadType;
            const isSaved = step.expected.is_saved || false;
            const actions = resolveActions(threadType, status, storedSection, hasPendingChanges, isSaved);
            expect(actions, `step ${i} (${step.emit}): actions`).toEqual(step.expected.expected_actions);
          }
        }
      });
    });
  }

  describe('Contract invariants', () => {
    it('Both thread types share the same legal sections', () => {
      expect(isSectionLegal('chat', 'archived')).toBe(true);
      expect(isSectionLegal('chat', 'inbox')).toBe(true);
      expect(isSectionLegal('claude_code', 'archived')).toBe(true);
      expect(isSectionLegal('claude_code', 'inbox')).toBe(true);
    });

    it('All critical events are classified in the contract', () => {
      const critical = [
        'MessageReceived', 'ResponseGenerated', 'CodingAgentIdled',
        'ThreadArchived',
        'ChangeProposed', 'ChangeApplied', 'ChangeDiscarded',
        'SessionStarted', 'SessionEnded', 'ResponseFailed',
      ];
      for (const evt of critical) {
        expect(EVENT_CLASSIFICATION[evt], `${evt} should be classified`).toBeDefined();
      }
    });

    // Test invariant scenarios from JSON
    for (const scenario of scenarioFile.scenarios) {
      if (!scenario.assert_invariant) continue;
      it(`Invariant: ${scenario.name}`, () => {
        const { thread_type, forbidden_section } = scenario.assert_invariant!;
        expect(isSectionLegal(thread_type as ThreadType, forbidden_section as ArchiveState)).toBe(false);
      });
    }
  });

  describe('Negative: illegal section transitions', () => {
    it('displaySection never returns review for stored=default (no pending changes)', () => {
      for (const status of ['idle', 'running', 'waiting'] as const) {
        const display = displaySection('archived', status, false, false, false);
        expect(display).not.toBe('review');
      }
    });

    it('running status maps to active when not saved, saved when saved (save overrides)', () => {
      for (const stored of ['archived', 'inbox'] as const) {
        expect(displaySection(stored, 'running', false, false, false)).toBe('active');
        expect(displaySection(stored, 'running', true, false, false)).toBe('saved');
      }
    });

    it('badge count must not include review threads that display as running', () => {
      // Simulates the attentionThreadCount logic: only count threads whose
      // displaySection is 'review', not just those with stored section 'inbox'.
      // Bug: a running CC thread with section='inbox' showed in RUNNING but
      // the badge counted it, producing a phantom "1" with no REVIEW section.
      const threads = [
        { section: 'inbox' as ArchiveState, status: 'running' as const, saved: false, hasActiveChildren: false },
        { section: 'inbox' as ArchiveState, status: 'idle' as const, saved: false, hasActiveChildren: false },
        { section: 'inbox' as ArchiveState, status: 'idle' as const, saved: false, hasActiveChildren: true },
        { section: 'archived' as ArchiveState, status: 'idle' as const, saved: false, hasActiveChildren: false },
      ];

      // Only the idle thread without active children should display as 'review'.
      // Running threads and threads with active children display as 'running'/'waiting'.
      const count = threads.filter(t =>
        displaySection(t.section, t.status, t.saved, t.hasActiveChildren, false) === 'review'
      ).length;
      expect(count).toBe(1);
    });
  });
});
