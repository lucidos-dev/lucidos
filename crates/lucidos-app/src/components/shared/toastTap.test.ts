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

  it('turns a lone OK into the card tap even with no X', () => {
    expect(toastTap({ type: 'info', action: { label: 'OK', onClick: noop }, dismissable: false })).toBe('action');
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

  it('closes a passive info or success toast', () => {
    expect(toastTap({ type: 'info' })).toBe('dismiss');
    expect(toastTap({ type: 'success' })).toBe('dismiss');
  });

  it('leaves a warning or an error up, since the reader may still be reading it', () => {
    expect(toastTap({ type: 'warning' })).toBeNull();
    expect(toastTap({ type: 'error' })).toBeNull();
  });

  it('never closes a non-dismissable toast, or one narrating work in flight', () => {
    expect(toastTap({ type: 'info', dismissable: false })).toBeNull();
    expect(toastTap({ type: 'info', spinning: true })).toBeNull();
    expect(toastTap({ type: 'info', progress: 0.4 })).toBeNull();
  });
});
