import { describe, it, expect } from 'vitest';
import { contentViewKey, inlineFormKey } from './contentViewKey';
import type { InlineForm } from '../../store/store';

// The content pane's one answer to "has this pane navigated". Two consumers key
// off it and must agree: the scroll memory (which restores a remembered
// scrollTop per view) and the navigation cover (which hides the swap frame).
// A key too coarse to tell two views apart breaks both at once, in the same
// direction: the pane restores the outgoing view's scroll onto the incoming one
// AND hard-cuts to it.

const emailRequest = {
  to: ['someone@example.com'],
  subject: 'Weekly summary',
  body: 'Body text',
  account: 'me@example.com',
  from: 'me@example.com',
};

describe('contentViewKey', () => {
  it('falls back to the active menu item when no overlay is showing', () => {
    expect(contentViewKey('triggers', null)).toBe('triggers');
  });

  it('is null when there is nothing to key on', () => {
    // No view to remember a scroll for, and none arriving to cover.
    expect(contentViewKey(null, null)).toBeNull();
  });

  it('lets an overlay win over the menu item behind it', () => {
    expect(contentViewKey('files', { type: 'file-preview', path: 'notes.md' })).toBe('file:notes.md');
  });

  it('separates two previews of different things', () => {
    const a = contentViewKey(null, { type: 'file-preview', path: 'a.md' });
    const b = contentViewKey(null, { type: 'file-preview', path: 'b.md' });
    expect(a).not.toBe(b);
    expect(contentViewKey(null, { type: 'url-preview', url: 'https://example.com/1' }))
      .not.toBe(contentViewKey(null, { type: 'url-preview', url: 'https://example.com/2' }));
  });

  it('separates two inline forms of the same type', () => {
    // The regression this guards: `overlay.type` is `'form'` for every inline
    // form, so a Back/Forward walk from one trigger's form to another's read as
    // no navigation at all.
    const a = contentViewKey(null, { type: 'form', form: { type: 'trigger', triggerId: 'alpha' } });
    const b = contentViewKey(null, { type: 'form', form: { type: 'trigger', triggerId: 'beta' } });
    expect(a).not.toBe(b);
  });

  it('separates two inline forms of different types', () => {
    const a = contentViewKey(null, { type: 'form', form: { type: 'new-app' } });
    const b = contentViewKey(null, { type: 'form', form: { type: 'app-edit', appId: 'alpha' } });
    expect(a).not.toBe(b);
  });

  it('keeps the same form stable across a re-render', () => {
    // The other half of the contract: an unchanged view must NOT re-key, or the
    // pane would re-cover and reset its scroll on every unrelated signal write.
    const key = () => contentViewKey(null, { type: 'form', form: { type: 'trigger', triggerId: 'alpha' } });
    expect(key()).toBe(key());
  });

  it('keys every app-ui overlay the same, whichever app it holds', () => {
    // Deliberate: an app switch keeps the same iframe and the frame's own load
    // cover re-covers it until the incoming document loads, which beats a timed
    // fade over a frame that may still be blank. There is no scroll to keep
    // apart either, the body being `overflow: hidden` under an app.
    const app = { id: 'alpha', name: 'Alpha' } as never;
    const other = { id: 'beta', name: 'Beta' } as never;
    expect(contentViewKey(null, { type: 'app-ui', app })).toBe('app-ui');
    expect(contentViewKey(null, { type: 'app-ui', app: other })).toBe('app-ui');
  });
});

describe('inlineFormKey', () => {
  it('distinguishes editing an existing credential from creating one', () => {
    expect(inlineFormKey({ type: 'credential', editing: 'github' }))
      .not.toBe(inlineFormKey({ type: 'credential' }));
  });

  it('distinguishes credential requests for different services', () => {
    expect(inlineFormKey({ type: 'credential', request: { service: 'github' } }))
      .not.toBe(inlineFormKey({ type: 'credential', request: { service: 'slack' } }));
  });

  it('distinguishes editing a credential from a fresh request for the same service', () => {
    // Two different panels that a bare `editing ?? service` collapsed into one:
    // the stored "github" credential's edit form, and an agent asking for a
    // "github" credential it has not got. Reachable by a Back/Forward walk, or
    // by a request arriving over an open edit form.
    expect(inlineFormKey({ type: 'credential', editing: 'github' }))
      .not.toBe(inlineFormKey({ type: 'credential', request: { service: 'github' } }));
  });

  it('distinguishes a new trigger from an existing one', () => {
    expect(inlineFormKey({ type: 'trigger' })).not.toBe(inlineFormKey({ type: 'trigger', triggerId: 'alpha' }));
  });

  it('keys plugin install and uninstall on their request ids', () => {
    const install = (id: string): InlineForm => ({
      type: 'plugin-install',
      request: {
        install_id: id,
        source: 'git',
        source_type: 'git',
        manifest: {},
        files: [],
        overwrites: [],
        plugin_id: 'p',
        plugin_version: '1.0.0',
        plugin_name: 'P',
      },
    });
    expect(inlineFormKey(install('one'))).not.toBe(inlineFormKey(install('two')));

    const uninstall = (id: string): InlineForm => ({
      type: 'plugin-uninstall',
      request: {
        uninstall_id: id,
        plugin_id: 'p',
        plugin_version: '1.0.0',
        plugin_name: 'P',
        files_present: [],
        files_missing: [],
      },
    });
    expect(inlineFormKey(uninstall('one'))).not.toBe(inlineFormKey(uninstall('two')));

    // …and ignores the receipt marker, for the same reason `sentAt` is ignored
    // below: a panel flipping to its receipt is the same panel mutating in
    // place, so re-keying would cover it and reset its scroll at the moment the
    // files landed or went.
    expect(inlineFormKey({
      ...install('one'),
      installed: { at: 'now', summary: 's', installed_files: [] },
    } as InlineForm)).toBe(inlineFormKey(install('one')));
    expect(inlineFormKey({
      ...uninstall('one'),
      removed: { at: 'now', summary: 's', files_deleted: [], files_missing: [] },
    } as InlineForm)).toBe(inlineFormKey(uninstall('one')));
  });

  it('ignores sentAt, so a sent email is still the same panel', () => {
    // The panel turning into a read-only receipt is not a navigation. Keying on
    // `sentAt` would re-cover it, and reset its scroll, the moment the mail went
    // out, in the middle of the user reading what they just sent.
    expect(inlineFormKey({ type: 'email-confirm', request: emailRequest }))
      .toBe(inlineFormKey({ type: 'email-confirm', request: emailRequest, sentAt: '2026-08-05T12:00:00Z' }));
  });

  it('keeps the email itself out of the key', () => {
    // The key is not confined to memory: `contentScrollKey` prefixes it into a
    // localStorage key, and nothing prunes those. A raw tuple would park the
    // recipient and the subject line in client storage for good.
    const key = inlineFormKey({ type: 'email-confirm', request: emailRequest });
    expect(key).not.toContain('someone@example.com');
    expect(key).not.toContain('me@example.com');
    expect(key).not.toContain('Weekly summary');
  });

  it('separates two different emails awaiting confirmation', () => {
    expect(inlineFormKey({ type: 'email-confirm', request: emailRequest }))
      .not.toBe(inlineFormKey({
        type: 'email-confirm',
        request: { ...emailRequest, subject: 'Something else' },
      }));
    expect(inlineFormKey({ type: 'email-confirm', request: emailRequest }))
      .not.toBe(inlineFormKey({
        type: 'email-confirm',
        request: { ...emailRequest, to: ['other@example.com'] },
      }));
  });
});
