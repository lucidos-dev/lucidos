// @vitest-environment jsdom
import { describe, it, expect } from 'vitest';
import {
  releaseRowIsOpen,
  releaseNotesBody,
  ReleaseNotes,
  leadAheadVersion,
  offeredRelease,
  releaseRowMark,
  releaseRowRoute,
  releaseRowStatus,
  stripReleaseHeading,
  defaultOpenRelease,
} from '../WhatsNewPage';
import type { ChangelogRelease } from '../../../api/client';
import type { VNode } from 'preact';

const RELEASE: ChangelogRelease = {
  version: '0.26.3',
  date: '2026-08-11',
  notes: '### Fixed\n\n- a thing',
};

const isRunning = (version: string, running: string | null) => version === running;

describe('releaseRowIsOpen', () => {
  it('opens the running release and leaves the rest shut', () => {
    expect(releaseRowIsOpen('0.26.3', isRunning('0.26.3', '0.26.3'), {})).toBe(true);
    expect(releaseRowIsOpen('0.26.2', isRunning('0.26.2', '0.26.3'), {})).toBe(false);
  });

  it('honours a toggle in either direction', () => {
    expect(releaseRowIsOpen('0.26.3', true, { '0.26.3': false })).toBe(false);
    expect(releaseRowIsOpen('0.26.2', false, { '0.26.2': true })).toBe(true);
  });

  it('opens the running release even when it arrives after the changelog', () => {
    expect(releaseRowIsOpen('0.26.3', isRunning('0.26.3', null), {})).toBe(false);
    expect(releaseRowIsOpen('0.26.3', isRunning('0.26.3', '0.26.3'), {})).toBe(true);
  });

  it('opens nothing when the running release has no section of its own', () => {
    expect(releaseRowIsOpen('0.26.3', isRunning('0.26.3', '9.9.9'), {})).toBe(false);
  });
});

describe('defaultOpenRelease', () => {
  const LIST: ChangelogRelease[] = [
    { version: '0.27.0', date: '2026-08-14', notes: '- the new thing' },
    { version: '0.26.3', date: '2026-08-11', notes: '- an old thing' },
  ];

  it('opens the release an update offer sent the reader here to read', () => {
    expect(defaultOpenRelease('0.27.0', '0.26.3', LIST)).toBe('0.27.0');
  });

  it('opens the running release when no offer sent them', () => {
    expect(defaultOpenRelease(null, '0.26.3', LIST)).toBe('0.26.3');
  });

  it('falls back when the target names nothing on screen', () => {
    expect(defaultOpenRelease('2026.08.13.1', '0.26.3', LIST)).toBe('0.26.3');
  });

  it('opens nothing when neither the target nor the running release is listed', () => {
    expect(defaultOpenRelease('9.9.9', null, LIST)).toBe(null);
  });

  it('still answers before the list has loaded', () => {
    expect(defaultOpenRelease('0.27.0', '0.26.3', [])).toBe('0.26.3');
  });

  it('leaves a toggled row alone, whatever it decided', () => {
    const open = defaultOpenRelease('0.27.0', '0.26.3', LIST);
    expect(releaseRowIsOpen('0.27.0', open === '0.27.0', { '0.27.0': false })).toBe(false);
  });
});

describe('releaseNotesBody', () => {
  type Body = { open: boolean; children: VNode<{ release: ChangelogRelease }> };

  it('parses no markdown while the row is shut', () => {
    const body = releaseNotesBody(RELEASE, false).props as Body;
    expect(body.open).toBe(false);
    expect(body.children.type).toBe(ReleaseNotes);
  });

  it('renders the notes as markdown once the row is open', () => {
    const body = releaseNotesBody(RELEASE, true).props as Body;
    expect(body.open).toBe(true);
    const notes = ReleaseNotes(body.children.props);
    const props = notes.props as unknown as { dangerouslySetInnerHTML: { __html: string } };
    const html = props.dangerouslySetInnerHTML.__html;
    expect(html).toContain('Fixed');
    expect(html).toContain('a thing');
  });
});

const EM_DASH = String.fromCharCode(0x2014);
const EN_DASH = String.fromCharCode(0x2013);

describe('stripReleaseHeading', () => {

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
    const offered = offeredRelease('9.9.9', '### Added\n\n- the new thing');
    expect(offered).toEqual({ version: '9.9.9', date: null, notes: '### Added\n\n- the new thing' });
  });

  it('strips the heading the manifest ships, so the version is not printed twice', () => {
    const offered = offeredRelease('9.9.9', `## v9.9.9 ${EM_DASH} 2026-09-01\n\n- the new thing`);
    expect(offered).toEqual({ version: '9.9.9', date: '2026-09-01', notes: '- the new thing' });
  });

  it('renders no row when the notes were nothing but a heading', () => {
    expect(offeredRelease('9.9.9', `## v9.9.9 ${EM_DASH} 2026-09-01\n`)).toBe(null);
  });

  it('renders no row when there is no update to describe', () => {
    expect(offeredRelease(null, '### Added')).toBe(null);
  });

  it('names the release its own notes name, not the argument', () => {
    const offered = offeredRelease('2026.08.13.1', `## v9.9.9 ${EM_DASH} 2026-09-01\n\n- a thing`);
    expect(offered?.version).toBe('9.9.9');
  });

  it('still takes the argument when the notes name no release', () => {
    expect(offeredRelease('9.9.9', '- a thing')?.version).toBe('9.9.9');
  });

  it('renders no row for a release the list below already carries', () => {
    const known = [{ version: '9.9.9', date: '2026-09-01', notes: '- a thing' }];
    expect(offeredRelease('9.9.9', `## v9.9.9\n\n- a thing`, known)).toBe(null);
    expect(offeredRelease('9.9.9', `## v9.9.10\n\n- a thing`, known)?.version).toBe('9.9.10');
  });

  it('renders no row when the manifest carried no notes', () => {
    expect(offeredRelease('9.9.9', null)).toBe(null);
  });
});

describe('releaseRowStatus', () => {
  it('marks the running release', () => {
    expect(releaseRowStatus('0.28.0', '0.28.0', null)).toBe('running');
  });

  it('marks the release on offer', () => {
    expect(releaseRowStatus('0.29.0', '0.28.0', '0.29.0')).toBe('available');
  });

  it('marks a published release the check has not offered', () => {
    expect(releaseRowStatus('0.29.0', '0.28.0', null)).toBe('newer');
  });

  it('marks nothing older than the running release', () => {
    expect(releaseRowStatus('0.27.0', '0.28.0', null)).toBe('none');
    expect(releaseRowStatus('0.27.0', '0.28.0', '0.29.0')).toBe('none');
  });

  it('marks nothing while the running release is unknown', () => {
    expect(releaseRowStatus('0.29.0', null, null)).toBe('none');
  });

  it('still marks the offer while the running release is unknown', () => {
    expect(releaseRowStatus('0.29.0', null, '0.29.0')).toBe('available');
  });

  it('marks nothing against a running version that outranks the list', () => {
    expect(releaseRowStatus('0.29.0', '2026.8.13.1', null)).toBe('none');
  });
});

describe('leadAheadVersion', () => {
  const LIST: ChangelogRelease[] = [
    { version: '0.29.0', date: null, notes: '- c' },
    { version: '0.28.0', date: null, notes: '- b' },
    { version: '0.27.0', date: null, notes: '- a' },
  ];

  it('picks the newest release ahead of the running one', () => {
    expect(leadAheadVersion(LIST, '0.27.0', null)).toBe('0.29.0');
  });

  it('picks the offer when the updater has made one', () => {
    expect(leadAheadVersion(LIST, '0.27.0', '0.29.0')).toBe('0.29.0');
  });

  it('picks the offer even when the list reaches past it', () => {
    expect(leadAheadVersion(LIST, '0.27.0', '0.28.0')).toBe('0.28.0');
  });

  it('falls back to the newest ahead row when the offer is not listed', () => {
    expect(leadAheadVersion(LIST, '0.27.0', '9.9.9')).toBe('0.29.0');
  });

  it('picks nothing when the reader is on the newest release', () => {
    expect(leadAheadVersion(LIST, '0.29.0', null)).toBeNull();
  });

  it('picks nothing while the running release is unknown', () => {
    expect(leadAheadVersion(LIST, null, null)).toBeNull();
  });

  it('picks nothing from an empty list', () => {
    expect(leadAheadVersion([], '0.27.0', null)).toBeNull();
  });
});

describe('releaseRowRoute', () => {
  it('carries whatever route this session takes, on the lead row', () => {
    expect(releaseRowRoute('available', true, 'install')).toBe('install');
    expect(releaseRowRoute('newer', true, 'check')).toBe('check');
    expect(releaseRowRoute('newer', true, 'guide')).toBe('guide');
  });

  it('always answers for a marked lead row', () => {
    for (const status of ['available', 'newer'] as const) {
      for (const route of ['install', 'check', 'guide'] as const) {
        expect(releaseRowRoute(status, true, route), `${status}/${route}`).toBe(route);
      }
    }
  });

  it('carries nothing on a row that is not the lead', () => {
    expect(releaseRowRoute('newer', false, 'check')).toBeNull();
    expect(releaseRowRoute('available', false, 'install')).toBeNull();
  });

  it('carries nothing on the running release or an older one', () => {
    expect(releaseRowRoute('running', true, 'install')).toBeNull();
    expect(releaseRowRoute('none', true, 'install')).toBeNull();
  });
});

describe('releaseRowMark', () => {
  it('drops the chip from a row that carries the install', () => {
    expect(releaseRowMark('available', 'install')).toBeNull();
  });

  it('keeps the chip beside a check or a route to Settings', () => {
    expect(releaseRowMark('newer', 'check')).toBe('newer');
    expect(releaseRowMark('newer', 'guide')).toBe('newer');
    expect(releaseRowMark('available', 'guide')).toBe('available');
  });

  it('keeps the chip where the row has no control', () => {
    expect(releaseRowMark('available', null)).toBe('available');
    expect(releaseRowMark('newer', null)).toBe('newer');
    expect(releaseRowMark('running', null)).toBe('running');
  });

  it('marks nothing on a release older than the running one', () => {
    expect(releaseRowMark('none', null)).toBeNull();
  });
});
