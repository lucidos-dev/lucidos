import { describe, it, expect, vi } from 'vitest';
import { dragCarriesFiles, installStrayFileDropGuard } from './strayFileDrop';

/** A minimal drag-event stand-in: only `dataTransfer.types` is read. */
const drag = (types?: readonly string[]) => ({
  dataTransfer: types ? { types } : null,
});

describe('dragCarriesFiles', () => {
  it('is true for an OS file drag', () => {
    expect(dragCarriesFiles(drag(['Files']))).toBe(true);
    expect(dragCarriesFiles(drag(['text/plain', 'Files']))).toBe(true);
  });

  it('is false for a text or link drag', () => {
    expect(dragCarriesFiles(drag(['text/plain']))).toBe(false);
    expect(dragCarriesFiles(drag(['text/uri-list']))).toBe(false);
  });

  it('is false when the event carries no dataTransfer at all', () => {
    expect(dragCarriesFiles(drag())).toBe(false);
    expect(dragCarriesFiles({})).toBe(false);
  });
});

describe('installStrayFileDropGuard', () => {
  installStrayFileDropGuard();

  /** Dispatch a drag event at the document (the test stub's `dispatchEvent`
   *  calls every registered listener) and report what the guard did to it. */
  function fire(type: 'dragover' | 'drop', types?: readonly string[]) {
    const e = new Event(type, { bubbles: true, cancelable: true });
    Object.defineProperty(e, 'dataTransfer', { value: types ? { types } : null });
    const stopPropagation = vi.fn();
    e.stopPropagation = stopPropagation;
    document.dispatchEvent(e);
    return { prevented: e.defaultPrevented, stopped: stopPropagation.mock.calls.length > 0 };
  }

  it('cancels the default on a file dragover and drop, so nothing navigates', () => {
    expect(fire('dragover', ['Files']).prevented).toBe(true);
    expect(fire('drop', ['Files']).prevented).toBe(true);
  });

  it('leaves a text drag completely alone', () => {
    expect(fire('dragover', ['text/plain']).prevented).toBe(false);
    expect(fire('drop', ['text/plain']).prevented).toBe(false);
  });

  it('never stops propagation, so a real drop zone still receives the event', () => {
    expect(fire('drop', ['Files']).stopped).toBe(false);
    expect(fire('dragover', ['Files']).stopped).toBe(false);
  });

  it('is idempotent: a second install adds no second listener', () => {
    const spy = vi.spyOn(document, 'addEventListener');
    installStrayFileDropGuard();
    expect(spy).not.toHaveBeenCalled();
    spy.mockRestore();
  });
});
