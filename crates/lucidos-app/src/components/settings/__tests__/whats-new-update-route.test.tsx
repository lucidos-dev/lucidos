// @vitest-environment jsdom
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';

const mocks = vi.hoisted(() => ({
  isTauri: vi.fn(() => false),
  thisDeviceIsMobile: vi.fn(() => false),
  engineChangelog: vi.fn(),
}));

vi.mock('../../../utils/platform', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../utils/platform')>()),
  isTauri: mocks.isTauri,
  thisDeviceIsMobile: mocks.thisDeviceIsMobile,
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
  updateRelay,
  whatsNewTargetRelease,
} from '../../../store/store';
import { isNewerVersion } from '../../../utils/version';
import type { ChangelogRelease } from '../../../api/client';
import type { ReleaseCheck } from '../../../api/client/control';

const RELEASES: ChangelogRelease[] = [
  { version: '0.31.1', date: '2026-08-27', notes: '### Fixed\n\n- a thing' },
  { version: '0.31.0', date: '2026-08-26', notes: '### Added\n\n- another thing' },
];

function gateway(over: Partial<ReleaseCheck> = {}): ReleaseCheck {
  return {
    enabled: true,
    supported: true,
    current_version: '0.31.0',
    checked_at: null,
    last_error: null,
    latest: null,
    update_address: null,
    update_address_error: null,
    ...over,
  };
}

describe('What’s New offers a route to every release it marks', () => {
  let host: HTMLElement;

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
    mocks.thisDeviceIsMobile.mockReturnValue(false);
    updateRelay.value = null;
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

  async function draw() {
    render(<WhatsNewPage />, host);
    await vi.waitFor(() => expect(mocks.engineChangelog).toHaveBeenCalled());
  }

  it('puts a control beside a release the updater has not offered', async () => {
    await draw();
    expect(row('0.31.1')).toEqual({ mark: 'Newer', action: 'How to Update' });
  });

  it('installs the release it names, in a client that can', async () => {
    mocks.isTauri.mockReturnValue(true);
    await draw();
    expect(row('0.31.1')).toEqual({ mark: null, action: 'Update & Restart' });
  });

  it('never offers to check for a release it has already named', async () => {
    for (const tauri of [true, false]) {
      for (const check of [gateway(), gateway({ supported: false })]) {
        mocks.isTauri.mockReturnValue(tauri);
        releaseCheck.value = check;
        render(null, host);
        await draw();
        const label = `${tauri}/${check.supported}`;
        expect(controls(), label).toHaveLength(1);
        expect(controls()[0].textContent, label).toBe(
          tauri ? 'Update & Restart' : 'How to Update',
        );
      }
    }
  });

  it('offers the relay for a release the gateway has not polled yet', async () => {
    updateRelay.value = { client: { version: '0.31.0', blocker: null }, request: null };
    for (const mobile of [true, false]) {
      mocks.thisDeviceIsMobile.mockReturnValue(mobile);
      render(null, host);
      await draw();
      expect(row('0.31.1'), `mobile=${mobile}`).toEqual({ mark: 'Newer', action: 'Update Desktop App' });
    }
  });

  it('keeps the old answer when the desktop app cannot take it', async () => {
    mocks.thisDeviceIsMobile.mockReturnValue(true);
    for (const desktop of [
      null,
      { client: null, request: null },
      { client: { version: '0.31.0', blocker: 'needs an admin password' }, request: null },
    ]) {
      updateRelay.value = desktop;
      render(null, host);
      await draw();
      expect(row('0.31.1').action, JSON.stringify(desktop)).toBe('How to Update');
    }
  });

  it('routes a source checkout to the page that can answer', async () => {
    releaseCheck.value = gateway({ supported: false });
    await draw();
    expect(row('0.31.1')).toEqual({ mark: 'Newer', action: 'How to Update' });
  });

  it('carries one control however many releases are ahead', async () => {
    lucidosRelease.value = '0.30.0';
    await draw();
    expect(controls()).toHaveLength(1);
    expect(row('0.31.1').action).toBe('How to Update');
    expect(row('0.31.0')).toEqual({ mark: 'Newer', action: null });
  });

  it('offers no control on the newest release', async () => {
    lucidosRelease.value = '0.31.1';
    await draw();
    expect(controls()).toHaveLength(0);
    expect(row('0.31.1').mark).toBe('Running');
  });

  it('offers no control while an install is running', async () => {
    appUpdateProgress.value = { version: '0.31.1', phase: 'installing' };
    await draw();
    expect(controls()).toHaveLength(0);
  });

  it('never lists a release ahead of the reader without a control on screen', async () => {
    for (const tauri of [true, false]) {
      for (const check of [gateway(), gateway({ supported: false })]) {
        for (const running of ['0.30.0', '0.31.0', '0.31.1']) {
          mocks.isTauri.mockReturnValue(tauri);
          releaseCheck.value = check;
          lucidosRelease.value = running;
          render(null, host);
          await draw();
          const ahead = RELEASES.some((r) => isNewerVersion(r.version, running));
          const label = `${tauri}/${check.supported}/${running}`;
          expect(controls().length, label).toBe(ahead ? 1 : 0);
        }
      }
    }
  });
});
