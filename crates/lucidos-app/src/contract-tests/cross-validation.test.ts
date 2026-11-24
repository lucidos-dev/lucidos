
import { describe, it, expect } from 'vitest';
import { actionBlocker, availableThreadActions, changeWork, displaySection, ownBlocker, OWN_BLOCKER_PRIORITY, THREAD_STATUSES } from '../generated/thread-lifecycle';
import type { ThreadType, ThreadStatus, ArchiveState, ThreadAction, ChangeStateKind, ChangeWork, DisplaySection, OwnBlocker } from '../generated/thread-lifecycle';
import fixture from '../generated/cross-validation-fixture.json';

interface AvailableThreadActionsCase {
  fn: 'availableThreadActions';
  args: [string, string, string, boolean, boolean, boolean, boolean, boolean];
  expected: string[];
}

interface DisplaySectionCase {
  fn: 'displaySection';
  args: [string, string, boolean, boolean, boolean, boolean, boolean];
  expected: string;
}

interface ChangeWorkCase {
  fn: 'changeWork';
  args: [ChangeStateKind, boolean];
  expected: string;
}

interface OwnBlockerCase {
  fn: 'ownBlocker';
  args: [string, string, string, string, boolean];
  expected: string | null;
}

interface ActionBlockerCase {
  fn: 'actionBlocker';
  args: [string | null, boolean, string | null];
  expected: string;
}

type TestCase = AvailableThreadActionsCase | DisplaySectionCase | ChangeWorkCase | OwnBlockerCase | ActionBlockerCase;

const FUNCTIONS_REQUIRING_CROSS_VALIDATION = new Set(['availableThreadActions', 'displaySection', 'changeWork', 'ownBlocker', 'actionBlocker']);

const LEGAL_RETENTION_PAIRS = 3;

describe('Cross-validation: generated TS matches Rust', () => {
  const cases = fixture.cases as TestCase[];

  describe('availableThreadActions', () => {
    const availableThreadActionsCases = cases.filter((c): c is AvailableThreadActionsCase => c.fn === 'availableThreadActions');

    it(`has exhaustive coverage (${availableThreadActionsCases.length} cases)`, () => {
      expect(availableThreadActionsCases.length).toBe(2 * THREAD_STATUSES.length * LEGAL_RETENTION_PAIRS * 2 ** 4);
    });

    for (const tc of availableThreadActionsCases) {
      const [threadType, status, section, pending, descendantsBlockArchive, waits, hasUnsentDraft, isSaved] = tc.args;
      const label = `(${threadType}, ${status}, ${section}, pending=${pending}, dba=${descendantsBlockArchive}, waits=${waits}, draft=${hasUnsentDraft}, saved=${isSaved})`;

      it(`${label} → [${tc.expected.join(', ')}]`, () => {
        const result = availableThreadActions(
          threadType as ThreadType,
          status as ThreadStatus,
          section as ArchiveState,
          pending,
          descendantsBlockArchive,
          waits,
          hasUnsentDraft,
          isSaved,
        );
        expect(result).toEqual(tc.expected as ThreadAction[]);
      });
    }
  });

  describe('displaySection', () => {
    const displaySectionCases = cases.filter((c): c is DisplaySectionCase => c.fn === 'displaySection');

    it(`has exhaustive coverage (${displaySectionCases.length} cases)`, () => {
      expect(displaySectionCases.length).toBe(LEGAL_RETENTION_PAIRS * THREAD_STATUSES.length * 2 ** 4);
    });

    for (const tc of displaySectionCases) {
      const [section, status, saved, activeChildren, pending, attention, held] = tc.args;
      const label = `(${section}, ${status}, saved=${saved}, active=${activeChildren}, pending=${pending}, attention=${attention}, held=${held})`;

      it(`${label} → ${tc.expected}`, () => {
        const result = displaySection(
          section as ArchiveState,
          status as ThreadStatus,
          saved,
          activeChildren,
          pending,
          attention,
          held,
        );
        expect(result).toBe(tc.expected as DisplaySection);
      });
    }
  });

  describe('changeWork', () => {
    const workCases = cases.filter((c): c is ChangeWorkCase => c.fn === 'changeWork');

    it(`has exhaustive coverage (${workCases.length} cases)`, () => {
      expect(workCases.length).toBe(3 * 2);
    });

    for (const tc of workCases) {
      const [change, waits] = tc.args;
      it(`(change=${change}, waits=${waits}) → ${tc.expected}`, () => {
        expect(changeWork(change, waits)).toBe(tc.expected);
      });
    }
  });

  describe('ownBlocker', () => {
    const ownCases = cases.filter((c): c is OwnBlockerCase => c.fn === 'ownBlocker');

    it(`has exhaustive coverage (${ownCases.length} cases)`, () => {
      expect(ownCases.length).toBe(2 * THREAD_STATUSES.length * 2 * 3 * 2);
    });

    for (const tc of ownCases) {
      const [threadType, status, section, change, externalRepo] = tc.args;
      const label = `(${threadType}, ${status}, ${section}, change=${change}, external=${externalRepo})`;

      it(`${label} → ${tc.expected}`, () => {
        const result = ownBlocker(
          threadType as ThreadType,
          status as ThreadStatus,
          section as ArchiveState,
          change as ChangeWork,
          externalRepo,
        );
        expect(result).toBe(tc.expected);
      });
    }
  });

  describe('actionBlocker', () => {
    const actionCases = cases.filter((c): c is ActionBlockerCase => c.fn === 'actionBlocker');

    it(`has exhaustive coverage (${actionCases.length} cases)`, () => {
      const ownOrNone = OWN_BLOCKER_PRIORITY.length + 1;
      expect(actionCases.length).toBe(ownOrNone * 2 * ownOrNone);
    });

    for (const tc of actionCases) {
      const [own, isHome, descendant] = tc.args;
      it(`(${own}, home=${isHome}, ${descendant}) → ${tc.expected}`, () => {
        expect(actionBlocker(own as OwnBlocker | null, isHome, descendant as OwnBlocker | null)).toBe(tc.expected);
      });
    }
  });

  describe('retention', () => {
    it('no case is both pinned and archived', () => {
      const illegal = cases.filter((c) =>
        (c.fn === 'availableThreadActions' && c.args[2] === 'archived' && c.args[7]) ||
        (c.fn === 'displaySection' && c.args[0] === 'archived' && c.args[2]),
      );
      expect(illegal).toEqual([]);
    });
  });

  describe('coverage meta-test', () => {
    it('every function requiring cross-validation has test cases in fixture', () => {
      const coveredFunctions: Set<string> = new Set(cases.map(c => c.fn));
      for (const fn of FUNCTIONS_REQUIRING_CROSS_VALIDATION) {
        expect(coveredFunctions.has(fn), `Missing cross-validation cases for '${fn}'`).toBe(true);
      }
    });

    it('no unexpected functions in fixture without validation requirement', () => {
      const coveredFunctions: Set<string> = new Set(cases.map(c => c.fn));
      for (const fn of coveredFunctions) {
        expect(
          FUNCTIONS_REQUIRING_CROSS_VALIDATION.has(fn),
          `Fixture has cases for '${fn}' but it's not in FUNCTIONS_REQUIRING_CROSS_VALIDATION`,
        ).toBe(true);
      }
    });
  });
});
