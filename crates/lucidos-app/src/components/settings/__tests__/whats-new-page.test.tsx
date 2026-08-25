// @vitest-environment jsdom
// This file renders markdown, and the sanitizer runs on a real DOM.
// The default `node` environment has none.
/**
 * Settings > System > What's New: the four decisions the panel makes, all
 * pulled out as pure functions so they can be held here rather than inferred
 * from a component that reads a hook.
 */
import { describe, it, expect } from 'vitest';
import {
  releaseRowIsOpen,
  releaseNotesBody,
  offeredRelease,
  releaseRowAction,
  releaseRowStatus,
  stripReleaseHeading,
  defaultOpenRelease,
} from '../WhatsNewPage';
import type { ChangelogRelease } from '../../../api/client';

const RELEASE: ChangelogRelease = {
  version: '0.26.3',
  date: '2026-08-11',
  notes: '### Fixed\n\n- a thing',
};

/** The panel's own derivation for a history row, restated so the tests below
 *  exercise the same expression the list passes in. */
const isRunning = (version: string, running: string | null) => version === running;

describe('releaseRowIsOpen', () => {
  it('opens the running release and leaves the rest shut', () => {
    expect(releaseRowIsOpen('0.26.3', isRunning('0.26.3', '0.26.3'), {})).toBe(true);
    expect(releaseRowIsOpen('0.26.2', isRunning('0.26.2', '0.26.3'), {})).toBe(false);
  });

  it('honours a toggle in either direction', () => {
    // Shutting the running release must stick, or the row springs back open on
    // the next render of the list.
    expect(releaseRowIsOpen('0.26.3', true, { '0.26.3': false })).toBe(false);
    expect(releaseRowIsOpen('0.26.2', false, { '0.26.2': true })).toBe(true);
  });

  it('opens the running release even when it arrives after the changelog', () => {
    // /health can answer after the changelog fetch does. The default is derived
    // per render rather than seeded into state precisely so this window resolves
    // itself: with the release still unknown nothing is open, and the moment it
    // lands its row is.
    expect(releaseRowIsOpen('0.26.3', isRunning('0.26.3', null), {})).toBe(false);
    expect(releaseRowIsOpen('0.26.3', isRunning('0.26.3', '0.26.3'), {})).toBe(true);
  });

  it('opens nothing when the running release has no section of its own', () => {
    // A RELEASE bump ahead of its changelog entry. Marking the newest instead
    // would state something untrue about what is running.
    expect(releaseRowIsOpen('0.26.3', isRunning('0.26.3', '9.9.9'), {})).toBe(false);
  });
});

describe('defaultOpenRelease', () => {
  const LIST: ChangelogRelease[] = [
    { version: '0.27.0', date: '2026-08-14', notes: '- the new thing' },
    { version: '0.26.3', date: '2026-08-11', notes: '- an old thing' },
  ];

  it('opens the release an update offer sent the reader here to read', () => {
    // The bug: the offer announced 0.27.0 and the panel expanded 0.26.3, the
    // release already running. The Available row used to cover for it, and no
    // longer does once the list itself carries the offered release.
    expect(defaultOpenRelease('0.27.0', '0.26.3', LIST)).toBe('0.27.0');
  });

  it('opens the running release when no offer sent them', () => {
    // Every other way in: the Lucidos menu's version row, Search Everywhere,
    // the System sub-panel list.
    expect(defaultOpenRelease(null, '0.26.3', LIST)).toBe('0.26.3');
  });

  it('falls back when the target names nothing on screen', () => {
    // A packaged client's announced version can disagree with every row. On a
    // dev workspace it is a CalVer app build id. Opening nothing at all would
    // be worse than opening what the panel opens anyway.
    expect(defaultOpenRelease('2026.08.13.1', '0.26.3', LIST)).toBe('0.26.3');
  });

  it('opens nothing when neither the target nor the running release is listed', () => {
    expect(defaultOpenRelease('9.9.9', null, LIST)).toBe(null);
  });

  it('still answers before the list has loaded', () => {
    // The panel renders an empty list while the changelog is in flight, and the
    // target must not be adopted against nothing.
    expect(defaultOpenRelease('0.27.0', '0.26.3', [])).toBe('0.26.3');
  });

  it('leaves a toggled row alone, whatever it decided', () => {
    // The two compose: this picks the DEFAULT, and the reader's own answer
    // still wins through `releaseRowIsOpen`.
    const open = defaultOpenRelease('0.27.0', '0.26.3', LIST);
    expect(releaseRowIsOpen('0.27.0', open === '0.27.0', { '0.27.0': false })).toBe(false);
  });
});

describe('releaseNotesBody', () => {
  it('renders nothing at all while the row is shut', () => {
    // Not a hidden element: 43 sections of markdown are parsed only as asked
    // for, which is the whole reason the list is collapsible.
    expect(releaseNotesBody(RELEASE, false)).toBe(null);
  });

  it('renders the notes as markdown once the row is open', () => {
    const body = releaseNotesBody(RELEASE, true);
    expect(body).not.toBe(null);
    const props = body!.props as unknown as { dangerouslySetInnerHTML: { __html: string } };
    const html = props.dangerouslySetInnerHTML.__html;
    expect(html).toContain('Fixed');
    expect(html).toContain('a thing');
  });
});

/** The separator this repo's changelog headings use. Built from its code point
 *  rather than typed, so this file stays clean under
 *  `.claude/rules/no-em-dashes.md`. */
const EM_DASH = String.fromCharCode(0x2014);
/** An en dash, which that rule does NOT ban. Built the same way only so it
 *  cannot be mistaken for the one above at a glance. */
const EN_DASH = String.fromCharCode(0x2013);

describe('stripReleaseHeading', () => {
  // The manifest's notes come from `release_notes_extract_section`, which writes
  // the CHANGELOG section HEADER INCLUDED, unlike the engine's parser. Left on,
  // the offered row prints its version a second time as an h2 inside its body.

  it('takes the heading off and keeps the version and date it named', () => {
    const notes = `## v9.9.9 ${EM_DASH} 2026-09-01\n\n### Added\n\n- the new thing\n`;
    expect(stripReleaseHeading(notes)).toEqual({
      version: '9.9.9',
      date: '2026-09-01',
      body: '### Added\n\n- the new thing',
    });
  });

  it('reads the date whatever separates it from the version', () => {
    for (const sep of [` ${EM_DASH} `, ` ${EN_DASH} `, ' - ']) {
      expect(stripReleaseHeading(`## v1.0.0${sep}2026-01-01\n\nbody`).date).toBe('2026-01-01');
    }
  });

  it('leaves notes that carry no heading exactly as they are', () => {
    expect(stripReleaseHeading('### Added\n\n- a thing')).toEqual({
      version: null,
      date: null,
      body: '### Added\n\n- a thing',
    });
  });

  it('does not mistake a prose heading for a release one', () => {
    const notes = '## various notes\n\n- a thing';
    expect(stripReleaseHeading(notes)).toEqual({ version: null, date: null, body: notes });
  });

  it('yields no date for a heading that carries only a version', () => {
    expect(stripReleaseHeading('## v1.0.0\n\nbody')).toEqual({
      version: '1.0.0',
      date: null,
      body: 'body',
    });
  });
});

describe('offeredRelease', () => {
  it('carries the OFFERED version and the notes that came with it', () => {
    // The whole point of the row: these notes describe the release being
    // offered, which postdates the binary whose changelog the list below shows.
    const offered = offeredRelease('9.9.9', '### Added\n\n- the new thing');
    expect(offered).toEqual({ version: '9.9.9', date: null, notes: '### Added\n\n- the new thing' });
  });

  it('strips the heading the manifest ships, so the version is not printed twice', () => {
    const offered = offeredRelease('9.9.9', `## v9.9.9 ${EM_DASH} 2026-09-01\n\n- the new thing`);
    expect(offered).toEqual({ version: '9.9.9', date: '2026-09-01', notes: '- the new thing' });
  });

  it('renders no row when the notes were nothing but a heading', () => {
    // Same reasoning as a notes-free manifest: an Available row that expands
    // onto an empty body is an affordance that opens onto nothing.
    expect(offeredRelease('9.9.9', `## v9.9.9 ${EM_DASH} 2026-09-01\n`)).toBe(null);
  });

  it('renders no row when there is no update to describe', () => {
    expect(offeredRelease(null, '### Added')).toBe(null);
  });

  it('names the release its own notes name, not the argument', () => {
    // The argument comes from `latestTauriAppVersion`, which the health poll
    // overwrites with the engine's `latest_tauri_app_version`. On a dev
    // workspace that is a CalVer app build id, and the row wore it.
    const offered = offeredRelease('2026.08.13.1', `## v9.9.9 ${EM_DASH} 2026-09-01\n\n- a thing`);
    expect(offered?.version).toBe('9.9.9');
  });

  it('still takes the argument when the notes name no release', () => {
    // A hand-cut manifest whose body is bare notes. The argument is the only
    // thing left that can name the row.
    expect(offeredRelease('9.9.9', '- a thing')?.version).toBe('9.9.9');
  });

  it('renders no row for a release the list below already carries', () => {
    // The list can now reach past the running release, so the offer and the
    // history can name the same version. One version, one row.
    const known = [{ version: '9.9.9', date: '2026-09-01', notes: '- a thing' }];
    expect(offeredRelease('9.9.9', `## v9.9.9\n\n- a thing`, known)).toBe(null);
    expect(offeredRelease('9.9.9', `## v9.9.10\n\n- a thing`, known)?.version).toBe('9.9.10');
  });

  it('renders no row when the manifest carried no notes', () => {
    // An affordance that opens onto nothing is worse than no affordance. It must
    // NOT fall back to the installed changelog: that would show the notes for
    // the version already running under a heading naming a different one.
    expect(offeredRelease('9.9.9', null)).toBe(null);
  });
});

/**
 * A changelog row can now be acted on.
 *
 * The panel lists the PUBLISHED changelog, so it can carry a release newer than
 * the running one. It could say nothing about that and offer nothing, which is
 * what the report was: the update is right there on screen and unreachable.
 */
describe('releaseRowStatus', () => {
  it('marks the running release', () => {
    expect(releaseRowStatus('0.28.0', '0.28.0', null)).toBe('running');
  });

  it('marks the release on offer', () => {
    expect(releaseRowStatus('0.29.0', '0.28.0', '0.29.0')).toBe('available');
  });

  // The two sources are independent: the changelog is fetched from the public
  // mirror, and the offer comes from the gateway's own periodic poll.
  it('marks a published release the check has not offered', () => {
    expect(releaseRowStatus('0.29.0', '0.28.0', null)).toBe('newer');
  });

  it('marks nothing older than the running release', () => {
    expect(releaseRowStatus('0.27.0', '0.28.0', null)).toBe('none');
    expect(releaseRowStatus('0.27.0', '0.28.0', '0.29.0')).toBe('none');
  });

  // /health can answer after the changelog does. Guessing in that window would
  // put a Newer chip on every row in the list.
  it('marks nothing while the running release is unknown', () => {
    expect(releaseRowStatus('0.29.0', null, null)).toBe('none');
  });

  it('still marks the offer while the running release is unknown', () => {
    expect(releaseRowStatus('0.29.0', null, '0.29.0')).toBe('available');
  });

  // On a dev workspace the running release is a CalVer app build id, whose
  // first component outranks every release. The comparison is the shared one,
  // so the list reads as history rather than as a wall of Newer chips.
  it('marks nothing against a running version that outranks the list', () => {
    expect(releaseRowStatus('0.29.0', '2026.8.13.1', null)).toBe('none');
  });
});

describe('releaseRowAction', () => {
  it('offers the install for the release actually on offer', () => {
    expect(releaseRowAction('available', true)).toBe('install');
  });

  // A browser or PWA session and a headless install can both see the offer and
  // neither can act on it. Their route is Settings, System.
  it('offers no install where this session cannot install', () => {
    expect(releaseRowAction('available', false)).toBeNull();
  });

  // The updater installs whatever the manifest resolves, so a row cannot ask
  // for a version by name. A check is what could turn this row into an offer.
  it('offers a check for a published release the updater has not offered', () => {
    expect(releaseRowAction('newer', true)).toBe('check');
    expect(releaseRowAction('newer', false)).toBe('check');
  });

  it('offers nothing on the running release or an older one', () => {
    expect(releaseRowAction('running', true)).toBeNull();
    expect(releaseRowAction('none', true)).toBeNull();
  });
});
