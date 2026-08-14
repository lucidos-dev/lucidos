/**
 * Settings > System > What's New: the three decisions the panel makes, all
 * pulled out as pure functions so they can be held here rather than inferred
 * from a component that reads a hook.
 */
import { describe, it, expect } from 'vitest';
import { releaseRowIsOpen, releaseNotesBody, offeredRelease, stripReleaseHeading } from '../WhatsNewPage';
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
