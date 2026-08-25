import { describe, it, expect } from 'vitest';
import { releaseNoticeRows } from '../ReleaseNoticesSection';
import type { ReleaseNotice, ReleaseNoticeView } from '../../../api/client';

function notice(id: string, since: string, resolved: boolean): ReleaseNotice {
  return { id, since, title: `Notice ${id}`, body: 'Do the thing.', resolved };
}

function view(notices: ReleaseNotice[], next_id: string | null): ReleaseNoticeView {
  return { notices, next_id };
}

/** `<id>:<state>` per row, which is the whole of what this function decides. */
function shape(v: ReleaseNoticeView): string[] {
  return releaseNoticeRows(v).map((r) => `${r.notice.id}:${r.state}`);
}

describe('the What you need to do rows', () => {
  it('leads with what is owed, in the order it must be worked through', () => {
    const v = view(
      [notice('a', '1.0.0', false), notice('b', '2.0.0', false), notice('c', '3.0.0', false)],
      'a',
    );
    expect(shape(v)).toEqual(['a:owed', 'b:queued', 'c:queued']);
  });

  // The ordering rule surviving outside the modal: only the notice whose turn
  // it is can be acted on, and the panel shows every row at once.
  it('marks exactly one row owed, whatever the workspace has already read', () => {
    const v = view(
      [notice('a', '1.0.0', true), notice('b', '2.0.0', false), notice('c', '3.0.0', false)],
      'b',
    );
    expect(shape(v)).toEqual(['b:owed', 'c:queued', 'a:resolved']);
  });

  it('puts what has been read underneath, newest first', () => {
    const v = view(
      [notice('a', '1.0.0', true), notice('b', '2.0.0', true), notice('c', '3.0.0', true)],
      null,
    );
    expect(shape(v)).toEqual(['c:resolved', 'b:resolved', 'a:resolved']);
  });

  // The engine names the owed notice; the panel never re-derives it. A page
  // that guessed "first unresolved" would disagree with the modal the moment
  // the two read the list at different times.
  it('queues every unresolved row when the engine names none', () => {
    const v = view([notice('a', '1.0.0', false)], null);
    expect(shape(v)).toEqual(['a:queued']);
  });

  it('has nothing to show for a workspace with no notices', () => {
    expect(shape(view([], null))).toEqual([]);
  });
});
