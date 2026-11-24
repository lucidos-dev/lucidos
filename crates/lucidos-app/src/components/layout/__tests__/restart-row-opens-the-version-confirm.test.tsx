// @vitest-environment jsdom
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';

const mocks = vi.hoisted(() => ({
  confirmAndRestartEngine: vi.fn(() => Promise.resolve()),
  initiateEngineRestart: vi.fn(() => Promise.resolve()),
}));

vi.mock('../../../store/actions/chat-changes', () => ({
  confirmAndRestartEngine: mocks.confirmAndRestartEngine,
  initiateEngineRestart: mocks.initiateEngineRestart,
}));

import { WorkspaceRestartRow } from '../WorkspaceMenuRows';
import { engineVersionReady, restartRequired, enginePackaged } from '../../../store/store';

let host: HTMLDivElement;
let calls: string[];

function renderRow(): void {
  render(<WorkspaceRestartRow onClose={() => calls.push('close')} />, host);
}

function tapRow(): void {
  host.querySelector<HTMLButtonElement>('button.brand-menu-item')?.click();
}

function settled(): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, 0));
}

beforeEach(() => {
  host = document.createElement('div');
  document.body.appendChild(host);
  calls = [];
  engineVersionReady.value = false;
  restartRequired.value = false;
  enginePackaged.value = false;
  mocks.confirmAndRestartEngine.mockClear().mockImplementation(() => {
    calls.push('confirm');
    return Promise.resolve();
  });
  mocks.initiateEngineRestart.mockClear().mockImplementation(() => {
    calls.push('restart');
    return Promise.resolve();
  });
});

afterEach(() => {
  render(null, host);
  host.remove();
});

describe('a new version is waiting', () => {
  beforeEach(() => {
    engineVersionReady.value = true;
    renderRow();
  });

  it('wears the pill, so the row says there is something to read', () => {
    expect(host.textContent).toContain('New version');
  });

  it('closes the menu FIRST, then opens the confirm', () => {
    tapRow();
    expect(calls).toEqual(['close', 'confirm']);
  });

  it('restarts nothing on its own, since the confirm owns that', () => {
    tapRow();
    expect(mocks.initiateEngineRestart).not.toHaveBeenCalled();
    expect(host.querySelector('.brand-menu-confirm')).toBeNull();
  });
});

describe('nothing newer', () => {
  beforeEach(renderRow);

  it('keeps its inline OK, with no modal and no pill', async () => {
    tapRow();
    await settled();
    expect(host.textContent).not.toContain('New version');
    expect(mocks.confirmAndRestartEngine).not.toHaveBeenCalled();
    expect(host.querySelector('.brand-menu-confirm')?.textContent).toBe('OK');
  });

  it('restarts only once that OK is pressed, and closes the menu with it', async () => {
    tapRow();
    await settled();
    expect(calls).toEqual([]);
    host.querySelector<HTMLButtonElement>('.brand-menu-confirm')?.click();
    expect(calls).toEqual(['close', 'restart']);
  });

  it('hands a version that arrived while the prompt was open to the confirm', async () => {
    tapRow();
    await settled();
    engineVersionReady.value = true;
    await settled();
    host.querySelector<HTMLButtonElement>('.brand-menu-confirm')?.click();
    expect(calls).toEqual(['close', 'confirm']);
    expect(mocks.initiateEngineRestart).not.toHaveBeenCalled();
  });
});
