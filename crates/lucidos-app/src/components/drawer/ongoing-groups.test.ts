/**
 * The Ongoing grouping's membership, order and selection.
 *
 * Each group reads its own predicate, so its count is that predicate's count,
 * and a thread shows in every group it matches. A thread matching none is in
 * no group: there is no Idle. The order never changes, and exactly one group
 * is selected.
 */
import { describe, it, expect, beforeEach } from 'vitest';
import {
    ongoingGroups, ongoingGroupRows, ongoingGroupOnSwitch, threadIsInFlight, ongoingGroupLists,
} from './family-graph';
import {
    ONGOING_GROUPS, blockedThreadCount, threadMap,
    threadIsBlocked, threadInReview, type OngoingGroup,
} from '../../store/store';
import { makeThreadState } from '../../store/actions/threads-test-helpers';
import { _resetComposeDraftsForTesting } from '../../store/composeDrafts';
import type { ThreadState } from '../../store/thread-events';

const live = { section: 'inbox' as const };

/** One thread per overlap the plan names, plus plain idle ones. */
function fixture(): Map<string, ThreadState> {
    const threads = [
        // Failed with a ready change: Blocked AND Review.
        makeThreadState('failed-change', { meta: { ...live, status: 'failed', channel: 'claude_code', codingAgentChangeState: { kind: 'proposed', requires_restart: false }, createdAt: '2026-01-09T00:00:00Z' } }),
        // A question: Blocked only.
        makeThreadState('question', { meta: { ...live, status: 'waiting_for_user_answer', createdAt: '2026-01-08T00:00:00Z' } }),
        // A stopped child with running children: Blocked, not In flight.
        makeThreadState('stopped', { meta: { ...live, isStoppedChild: true, activeChildrenCount: 1, totalChildrenCount: 1, createdAt: '2026-01-07T00:00:00Z' } }),
        // Running with a draft: In flight AND Drafts.
        makeThreadState('draft-running', { meta: { ...live, status: 'running', composeText: 'follow up', createdAt: '2026-01-06T00:00:00Z' } }),
        // Parked on its own event wait: In flight (its dot reads waiting).
        makeThreadState('parked', { meta: { ...live, liveEventWaitCount: 1, createdAt: '2026-01-05T12:00:00Z' } }),
        // Paused: no predicate takes it, since the engine resumes it by
        // itself (threadIsBlocked). So it is in no group.
        makeThreadState('paused', { meta: { ...live, status: 'paused', createdAt: '2026-01-05T00:00:00Z' } }),
        // A ready change: Review only.
        makeThreadState('change', { meta: { ...live, channel: 'claude_code', codingAgentChangeState: { kind: 'proposed', requires_restart: false }, createdAt: '2026-01-04T00:00:00Z' } }),
        // Quiet, current and pinned, with a child: in no group.
        makeThreadState('idle-current', { meta: { ...live, createdAt: '2026-01-03T00:00:00Z' } }),
        makeThreadState('idle-child', { meta: { ...live, parentThreadId: 'idle-current', createdAt: '2026-01-03T01:00:00Z' } }),
        makeThreadState('idle-pinned', { meta: { ...live, saved: true, createdAt: '2026-01-02T00:00:00Z' } }),
        // Archived and home: listed in no group.
        makeThreadState('archived', { meta: { section: 'archived', createdAt: '2026-01-01T00:00:00Z' } }),
        makeThreadState('home', { meta: { ...live, home: true, createdAt: '2026-01-01T00:00:00Z' } }),
    ];
    return new Map(threads.map(t => [t.meta.id, t]));
}

const ids = (map: Map<string, ThreadState>, group: OngoingGroup) => ongoingGroupRows(map, group).map(n => n.thread.meta.id);

beforeEach(() => {
    _resetComposeDraftsForTesting();
});

describe('ongoing group membership', () => {
    it('lists the four groups in the fixed order, Drafts first', () => {
        expect(ongoingGroups(fixture(), ONGOING_GROUPS).map(g => g.group))
            .toEqual(['blocked', 'review', 'drafts', 'in-flight']);
    });

    it('shows a thread in every group it matches', () => {
        const map = fixture();
        expect(ids(map, 'blocked')).toEqual(expect.arrayContaining(['failed-change', 'question', 'stopped']));
        expect(ids(map, 'review')).toEqual(expect.arrayContaining(['failed-change', 'change']));
        expect(ids(map, 'in-flight')).toEqual(expect.arrayContaining(['draft-running', 'parked']));
        expect(ids(map, 'drafts')).toEqual(['draft-running']);
        expect(ids(map, 'in-flight')).not.toContain('stopped');
    });

    it('lists a thread matching no status in no group', () => {
        const map = fixture();
        const listed = new Set(ONGOING_GROUPS.flatMap(g => ids(map, g)));
        for (const id of ['paused', 'idle-current', 'idle-child', 'idle-pinned', 'archived', 'home']) {
            expect(listed.has(id), id).toBe(false);
        }
    });

    it("counts each group as its predicate does, and attention as its badge", () => {
        const map = fixture();
        threadMap.value = map;
        const count = (g: OngoingGroup) => ongoingGroupRows(map, g).length;
        const all = [...map.values()];
        expect(count('blocked')).toBe(blockedThreadCount.value);
        expect(all.filter(threadIsBlocked)).toHaveLength(count('blocked'));
        expect(all.filter(threadInReview)).toHaveLength(count('review'));
        expect(all.filter(threadIsInFlight)).toHaveLength(count('in-flight'));
    });

    it('keeps the memoized lists in step with the loaded threads', () => {
        threadMap.value = fixture();
        expect(ongoingGroupLists.value.map(g => g.rows.length)).toEqual(
            ONGOING_GROUPS.map(g => ongoingGroupRows(threadMap.value, g).length));
        threadMap.value = new Map();
        expect(ongoingGroupLists.value.every(g => g.rows.length === 0)).toBe(true);
    });
});

/** Every ongoing group, with one row in each group `filled` names. */
const groups = (filled: OngoingGroup[] = []) => ONGOING_GROUPS.map(group => ({
    group, rows: filled.includes(group) ? [{ thread: makeThreadState(group), depth: 0 }] : [],
}));

describe('the group selected on a switch to Ongoing', () => {
    it('selects Blocked when the button carried a badge', () => {
        expect(ongoingGroupOnSwitch('review', groups(['blocked', 'review']), true)).toBe('blocked');
        expect(ongoingGroupOnSwitch('drafts', groups(), true)).toBe('blocked');
    });

    it('keeps the stored group while it has rows', () => {
        expect(ongoingGroupOnSwitch('review', groups(['drafts', 'review']), false)).toBe('review');
    });

    it('moves to the first group with rows when the stored one is empty', () => {
        expect(ongoingGroupOnSwitch('drafts', groups(['review']), false)).toBe('review');
        expect(ongoingGroupOnSwitch('blocked', groups(['review', 'in-flight']), false)).toBe('review');
    });

    it('keeps the stored group when every group is empty', () => {
        expect(ongoingGroupOnSwitch('review', groups(), false)).toBe('review');
    });
});
