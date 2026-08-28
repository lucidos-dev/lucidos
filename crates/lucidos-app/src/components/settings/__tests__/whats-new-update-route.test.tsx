// @vitest-environment jsdom
/**
 * The rule the whole panel is held to: it may not say a release is newer than
 * the one you run and then offer no way to get it.
 *
 * The pure decisions are pinned next door, in `whats-new-page.test.tsx`. This
 * file renders the panel, because the defect was an ABSENCE on screen. Only a
 * render can prove a marked row and a control arrive together.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';

const mocks = vi.hoisted(() => ({
  isTauri: vi.fn(() => false),
  engineChangelog: vi.fn(),
}));

vi.mock('../../../utils/platform', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../utils/platform')>()),
  isTauri: mocks.isTauri,
}));
vi.mock('../../../api/client', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../api/client')>()),
  engineChangelog: mocks.engineChangelog,
}));

import { WhatsNewPage } from '../WhatsNewPage';
import {
  appUpdateProgress,
  changelogReleases,
  latestTauriAppNotes,
  lucidosRelease,
  releaseCheck,
  whatsNewTargetRelease,
} from '../../../store/store';
import type { ChangelogRelease } from '../../../api/client';
import type { ReleaseCheck } from '../../../api/client/control';

/** The reported state: the published changelog carries a release the running
 *  engine does not. */
const RELEASES: ChangelogRelease[] = [
  { version: '0.31.1', date: '2026-08-27', notes: '### Fixed\n\n- a thing' },
  { version: '0.31.0', date: '2026-08-26', notes: '### Added\n\n- another thing' },
];

/** A gateway answer. `supported: false` is every source checkout, and it is the
 *  state the report came from. */
function gateway(over: Partial<ReleaseCheck> = {}): ReleaseCheck {
  return {
    enabled: true,
    supported: true,
    current_version: '0.31.0',
    checked_at: null,
    last_error: null,
    latest: null,
    ...over,
  };
}

describe('What’s New offers a route to every release it marks', () => {
  let host: HTMLElement;

  /** The chip a row wears, and the control beside it, for one release. */
  function row(version: string) {
    const el = host.querySelector(`[data-release="${version}"]`);
    return {
      mark: el?.querySelector('.whats-new-mark')?.textContent ?? null,
      action: el?.querySelector('.whats-new-release-action')?.textContent ?? null,
    };
  }

  const controls = () => host.querySelectorAll('.whats-new-release-action');

  beforeEach(async () => {
    host = document.createElement('div');
    document.body.appendChild(host);
    mocks.isTauri.mockReturnValue(false);
    mocks.engineChangelog.mockResolvedValue(RELEASES);
    changelogReleases.value = { status: 'loaded', data: RELEASES };
    lucidosRelease.value = '0.31.0';
    latestTauriAppNotes.value = null;
    appUpdateProgress.value = null;
    whatsNewTargetRelease.value = null;
    releaseCheck.value = gateway();
  });

  afterEach(() => {
    render(null, host);
    host.remove();
    vi.restoreAllMocks();
  });

  /** Render and let the mount effects (the changelog fetch) settle. */
  async function draw() {
    render(<WhatsNewPage />, host);
    await vi.waitFor(() => expect(mocks.engineChangelog).toHaveBeenCalled());
  }

  // The report, verbatim: 0.31.1 wore a Newer chip and nothing else.
  it('puts a control beside a release the updater has not offered', async () => {
    await draw();
    expect(row('0.31.1')).toEqual({ mark: 'Newer', action: 'Check for Updates' });
  });

  // A source checkout never polls (ADR 0108), so a check would fail every time.
  // Settings, System carries its answer, which is pull and rebuild.
  it('routes a source checkout to the page that can answer', async () => {
    releaseCheck.value = gateway({ supported: false });
    await draw();
    expect(row('0.31.1')).toEqual({ mark: 'Newer', action: 'How to Update' });
  });

  // A control per row would repeat one global answer down the list.
  it('carries one control however many releases are ahead', async () => {
    lucidosRelease.value = '0.30.0';
    await draw();
    expect(controls()).toHaveLength(1);
    expect(row('0.31.1').action).toBe('Check for Updates');
    expect(row('0.31.0')).toEqual({ mark: 'Newer', action: null });
  });

  // Nothing is ahead, so there is nothing to route to.
  it('offers no control on the newest release', async () => {
    lucidosRelease.value = '0.31.1';
    await draw();
    expect(controls()).toHaveLength(0);
    expect(row('0.31.1').mark).toBe('Running');
  });

  // The progress dialog narrates the run, and a row offering to start another
  // would be a lie.
  it('offers no control while an install is running', async () => {
    appUpdateProgress.value = { version: '0.31.1', phase: 'installing' };
    await draw();
    expect(controls()).toHaveLength(0);
  });

  // The whole rule, as one assertion over the rendered panel.
  it('never marks a row ahead of the reader without a control on screen', async () => {
    for (const check of [gateway(), gateway({ supported: false })]) {
      for (const running of ['0.30.0', '0.31.0', '0.31.1']) {
        releaseCheck.value = check;
        lucidosRelease.value = running;
        render(null, host);
        await draw();
        const marked = host.querySelectorAll('.whats-new-mark.is-newer').length;
        const label = `${running}/${check.supported}`;
        if (marked > 0) expect(controls().length, label).toBe(1);
      }
    }
  });
});
