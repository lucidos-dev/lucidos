import { describe, it, expect, beforeEach } from 'vitest';
import type { ComponentChildren, VNode } from 'preact';
import { ToastList, toastProgressWidth } from '../Toast';
import { toasts, showToast, engineRestarting } from '../../../store/store';

/** Walk a vnode tree, returning every node whose className contains `cls`. */
function findByClass(node: ComponentChildren, cls: string, out: VNode[] = []): VNode[] {
  if (node === null || node === undefined || typeof node === 'boolean') return out;
  if (typeof node === 'string' || typeof node === 'number') return out;
  if (Array.isArray(node)) {
    for (const child of node) findByClass(child, cls, out);
    return out;
  }
  const v = node as VNode<{ class?: string; className?: string; children?: ComponentChildren }>;
  const classAttr = v.props?.class ?? v.props?.className ?? '';
  if (typeof classAttr === 'string' && classAttr.split(/\s+/).includes(cls)) {
    out.push(v);
  }
  findByClass(v.props?.children, cls, out);
  return out;
}

function fillWidth(): string | undefined {
  const fills = findByClass(ToastList(), 'progress-bar-fill');
  const style = (fills[0]?.props as { style?: { width?: string } } | undefined)?.style;
  return style?.width;
}

beforeEach(() => {
  toasts.value = [];
  engineRestarting.value = false;
});

describe('toastProgressWidth', () => {
  it('maps a fraction onto a percentage width', () => {
    expect(toastProgressWidth(0)).toBe('0%');
    expect(toastProgressWidth(0.5)).toBe('50%');
    expect(toastProgressWidth(1)).toBe('100%');
  });

  // The fraction comes from a byte count divided by a server-declared total, so
  // a bad total must not paint a bar running past its own track — or `NaN%`.
  it('clamps an out-of-range fraction into the track', () => {
    expect(toastProgressWidth(1.4)).toBe('100%');
    expect(toastProgressWidth(-0.2)).toBe('0%');
  });

  // A non-finite fraction means the division went wrong, i.e. we do NOT know how
  // far along we are. An empty track next to the spinner says that; a full one
  // would claim the transfer finished.
  it('renders an unknown fraction as empty, not complete', () => {
    expect(toastProgressWidth(NaN)).toBe('0%');
    expect(toastProgressWidth(Infinity)).toBe('0%');
  });
});

describe('Toast determinate progress', () => {
  it('renders the shared progress bar when a fraction is supplied', () => {
    showToast('Downloading Lucidos 2026.7.30 — 50 MB of 100 MB', 'info', {
      key: 'app-update-available',
      spinning: true,
      progress: 0.5,
    });
    expect(findByClass(ToastList(), 'toast-progress').length).toBe(1);
    expect(fillWidth()).toBe('50%');
  });

  // An unknown-size download has no honest percentage: the spinner and the byte
  // count carry it, and a bar would be a fabrication.
  it('renders no bar when the operation has no honest percentage', () => {
    showToast('Downloading Lucidos 2026.7.30 — 50 MB', 'info', {
      key: 'app-update-available',
      spinning: true,
      progress: null,
    });
    expect(findByClass(ToastList(), 'toast-progress').length).toBe(0);
  });

  it('renders no bar for an ordinary toast', () => {
    showToast('Copied to clipboard', 'success');
    expect(findByClass(ToastList(), 'toast-progress').length).toBe(0);
  });

  // A keyed re-show updates in place; the bar has to advance with it rather than
  // stick at the value the toast first appeared with.
  it('advances the bar when the keyed toast is re-shown', () => {
    showToast('Downloading', 'info', { key: 'app-update-available', progress: 0.25 });
    expect(fillWidth()).toBe('25%');
    showToast('Downloading', 'info', { key: 'app-update-available', progress: 0.75 });
    expect(toasts.value.length).toBe(1);
    expect(fillWidth()).toBe('75%');
  });

  // Zero is a real value, not "absent" — a download that has just started shows
  // an empty track rather than no track.
  it('renders an empty bar at zero rather than dropping it', () => {
    showToast('Downloading', 'info', { key: 'app-update-available', progress: 0 });
    expect(findByClass(ToastList(), 'toast-progress').length).toBe(1);
    expect(fillWidth()).toBe('0%');
  });
});
