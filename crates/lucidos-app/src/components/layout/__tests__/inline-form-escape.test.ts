/**
 * One Escape does one thing. The central dispatcher blurs a focused input and
 * calls `preventDefault`, but lets the keydown bubble on to the document. The
 * inline form's listener must treat that Escape as spent. Otherwise Escape in
 * the composer also closes the form beside it and drops its edits.
 */
import { afterEach, describe, expect, it } from 'vitest';
import { closeInlineFormOnEscape } from '../InlineForm';
import { panelOverlay } from '../../../store/store';

const openForm = () => { panelOverlay.value = { type: 'form', form: { type: 'new-app' } }; };

describe('closeInlineFormOnEscape', () => {
  afterEach(() => { panelOverlay.value = null; });

  it('closes the open form on a fresh Escape', () => {
    openForm();
    closeInlineFormOnEscape({ key: 'Escape', defaultPrevented: false });
    expect(panelOverlay.value).toBeNull();
  });

  it('keeps the form open when an earlier handler already spent the Escape', () => {
    openForm();
    closeInlineFormOnEscape({ key: 'Escape', defaultPrevented: true });
    expect(panelOverlay.value).toEqual({ type: 'form', form: { type: 'new-app' } });
  });

  it('ignores other keys', () => {
    openForm();
    closeInlineFormOnEscape({ key: 'Enter', defaultPrevented: false });
    expect(panelOverlay.value).not.toBeNull();
  });
});
