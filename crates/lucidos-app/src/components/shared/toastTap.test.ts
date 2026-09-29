import { describe, expect, it } from 'vitest';
import { toastTap } from './toastTap';

const noop = () => {};
const OPEN = { label: 'Open', onClick: noop };

describe('toastTap', () => {
  it('runs the toast onClick when it has one, whatever its type', () => {
    expect(toastTap({ type: 'info', onClick: noop })).toBe('click');
    expect(toastTap({ type: 'error', onClick: noop })).toBe('click');
  });

  it('turns the lone neutral action of an info or success toast into the card tap', () => {
    expect(toastTap({ type: 'info', action: OPEN })).toBe('action');
    expect(toastTap({ type: 'success', action: OPEN })).toBe('action');
  });

  it('keeps the button on a warning or an error, since a tap to read it must not act', () => {
    expect(toastTap({ type: 'warning', action: OPEN })).toBeNull();
    expect(toastTap({ type: 'error', action: { label: 'Retry build', onClick: noop } })).toBeNull();
  });

  it('keeps both buttons when there are two actions', () => {
    expect(toastTap({ type: 'info', action: { label: 'Refresh', onClick: noop }, secondaryAction: { label: 'Later', onClick: noop } })).toBeNull();
  });

  it('keeps a lone danger or confirm action as a button a stray tap cannot reach', () => {
    expect(toastTap({ type: 'info', action: { label: 'Cancel', onClick: noop, variant: 'danger' } })).toBeNull();
    expect(toastTap({ type: 'info', action: { label: 'Update & restart', onClick: noop, variant: 'confirm' } })).toBeNull();
  });

  it('never closes a passive toast, since a reader may tap it looking for more', () => {
    expect(toastTap({ type: 'info' })).toBeNull();
    expect(toastTap({ type: 'success' })).toBeNull();
    expect(toastTap({ type: 'warning' })).toBeNull();
    expect(toastTap({ type: 'error' })).toBeNull();
  });
});
