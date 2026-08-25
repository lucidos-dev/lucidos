import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
import { releaseNoticeSplit, releaseNoticeRows } from '../ReleaseNoticesPage';
import type { ReleaseNotice, ReleaseNoticeView } from '../../../api/client';

const SECTION = readFileSync(
  fileURLToPath(new URL('../ReleaseNoticesPage.tsx', import.meta.url)), 'utf8',
);
const WHATS_NEW_CSS = readFileSync(
  fileURLToPath(new URL('../../../styles/settings/whats-new.css', import.meta.url)), 'utf8',
);

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

describe('the panel rows, in order', () => {
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

/**
 * A heading is a claim about every row under it.
 *
 * The reader who tapped an action came back to their answered notice, still
 * under "What you need to do", telling them to do it again. So the two halves
 * are drawn apart: owed rows keep the instruction heading, answered ones fold
 * away behind a disclosure.
 */
describe('the two halves of the panel', () => {
  /** `<half>: <ids>`, which is the whole of what the split decides. */
  function halves(v: ReleaseNoticeView): string[] {
    const { owed, answered } = releaseNoticeSplit(v);
    return [
      `owed: ${owed.map((r) => r.notice.id).join(',')}`,
      `answered: ${answered.map((r) => r.notice.id).join(',')}`,
    ];
  }

  it('leaves nothing owed once every notice is answered', () => {
    expect(halves(view([notice('a', '1.0.0', true)], null))).toEqual(['owed: ', 'answered: a']);
  });

  it('keeps an unanswered notice on the owed side', () => {
    expect(halves(view([notice('a', '1.0.0', false)], 'a'))).toEqual(['owed: a', 'answered: ']);
  });

  it('splits a mixed list without disturbing either order', () => {
    const v = view(
      [notice('a', '1.0.0', true), notice('b', '2.0.0', false), notice('c', '3.0.0', false)],
      'b',
    );
    expect(halves(v)).toEqual(['owed: b,c', 'answered: a']);
  });

  it('has nothing on either side for a workspace with no notices', () => {
    expect(halves(view([], null))).toEqual(['owed: ', 'answered: ']);
  });
});

/**
 * What the two halves look like, as a source scan.
 *
 * The heading only renders over rows that are owed, and the answered ones sit
 * behind a shut disclosure. So a workspace that has dealt with everything opens
 * on one quiet line rather than on an instruction it has already carried out.
 */
describe('the answered half', () => {
  it('gives the page ONE heading, with the owed rows straight under it', () => {
    // A second heading over the owed rows would restate the tab's own name.
    expect(SECTION).toContain('Release notices');
    expect(SECTION).toContain('{owed.length > 0 && (');
    expect(SECTION).not.toContain('What you need to do');
  });

  it('folds the answered rows behind a disclosure, shut on arrival', () => {
    expect(SECTION).toContain('useState(false)');
    expect(SECTION).toContain('aria-expanded={showAnswered}');
    expect(SECTION).toContain('Already answered ({answered.length})');
    expect(SECTION).toContain('{showAnswered && (');
  });

  it('ticks an answered row, so the state is stated and not merely implied', () => {
    expect(SECTION).toContain("{state === 'resolved' && (");
    expect(SECTION).toContain('<CheckIcon');
  });

  it('strikes the title of an answered row through', () => {
    expect(WHATS_NEW_CSS).toMatch(
      /\.release-notice-row\[data-state="resolved"\] \.release-notice-row-title \{[^}]*text-decoration: line-through/,
    );
  });
});

/**
 * Every unresolved notice can be answered from here, action or no action.
 *
 * `action_label` is optional in `release-notices.toml`, and the actions block
 * used to hang off it. A notice carrying none could then be answered only in
 * the modal, which Escape closes for the page's life. The *System attention badge*
 * points at this tab, so the reader would have arrived at a dot with nothing
 * to press.
 *
 * A source scan rather than a render: the section fetches on mount and pulls
 * the store in, the reason `settings-nav-structure.test.ts` gives.
 */
describe('answering a notice from the panel', () => {
  it('offers Got it on every unresolved row, not only one carrying an action', () => {
    expect(SECTION).toContain("{state !== 'resolved' && (");
    expect(SECTION).toContain('acknowledgeReleaseNotice(notice)');
  });

  it('keeps the action button conditional on the notice authoring one', () => {
    expect(SECTION).toContain('{notice.action_label && (');
  });

  // An answered row keeps the button as a record of what the notice offered,
  // greyed rather than gone.
  it('leaves a resolved row its action button', () => {
    expect(SECTION).toContain("{(notice.action_label || state !== 'resolved') && (");
  });

  it('lets only the OWED row act', () => {
    // One predicate for both buttons: a queued notice is waiting its turn, and
    // an answered one is done. Neither can be pressed.
    expect(SECTION.match(/disabled=\{state !== 'owed'\}/g) ?? []).toHaveLength(2);
  });
});
