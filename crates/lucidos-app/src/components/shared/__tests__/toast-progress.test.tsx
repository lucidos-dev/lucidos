import { describe, it, expect, beforeEach } from 'vitest';
import type { ComponentChildren, VNode } from 'preact';
import { ToastList } from '../Toast';
import { progressFillWidth } from '../progressBar';
import { toasts, showToast, engineRestarting } from '../../../store/store';

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

describe('progressFillWidth', () => {
  it('maps a fraction onto a percentage width', () => {
    expect(progressFillWidth(0)).toBe('0%');
    expect(progressFillWidth(0.5)).toBe('50%');
    expect(progressFillWidth(1)).toBe('100%');
  });

  it('clamps an out-of-range fraction into the track', () => {
    expect(progressFillWidth(1.4)).toBe('100%');
    expect(progressFillWidth(-0.2)).toBe('0%');
  });

  it('renders an unknown fraction as empty, not complete', () => {
    expect(progressFillWidth(NaN)).toBe('0%');
    expect(progressFillWidth(Infinity)).toBe('0%');
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

  it('advances the bar when the keyed toast is re-shown', () => {
    showToast('Downloading', 'info', { key: 'app-update-available', progress: 0.25 });
    expect(fillWidth()).toBe('25%');
    showToast('Downloading', 'info', { key: 'app-update-available', progress: 0.75 });
    expect(toasts.value.length).toBe(1);
    expect(fillWidth()).toBe('75%');
  });

  it('renders an empty bar at zero rather than dropping it', () => {
    showToast('Downloading', 'info', { key: 'app-update-available', progress: 0 });
    expect(findByClass(ToastList(), 'toast-progress').length).toBe(1);
    expect(fillWidth()).toBe('0%');
  });
});
