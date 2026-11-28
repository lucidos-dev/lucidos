import { describe, it, expect } from 'vitest';
import { contentViewKey, inlineFormKey, settingsViewKey } from './contentViewKey';
import type { InlineForm } from '../../store/store';


const emailRequest = {
  to: ['someone@example.com'],
  subject: 'Weekly summary',
  body: 'Body text',
  account: 'me@example.com',
  from: 'me@example.com',
};

describe('contentViewKey', () => {
  it('falls back to the active menu item when no overlay is showing', () => {
    expect(contentViewKey('triggers', null, 'main')).toBe('triggers');
  });

  it('is null when there is nothing to key on', () => {
    expect(contentViewKey(null, null, 'main')).toBeNull();
  });

  it('lets an overlay win over the menu item behind it', () => {
    expect(contentViewKey('files', { type: 'file-preview', path: 'notes.md' }, 'main')).toBe('file:notes.md');
  });

  it('separates two Settings sub-sections', () => {
    expect(contentViewKey('settings', null, 'whats-new'))
      .not.toBe(contentViewKey('settings', null, 'models'));
  });

  it('separates a Settings sub-section from the Settings home list', () => {
    expect(contentViewKey('settings', null, 'main'))
      .not.toBe(contentViewKey('settings', null, 'whats-new'));
  });

  it('keeps a Settings sub-section stable across a re-render', () => {
    expect(contentViewKey('settings', null, 'whats-new'))
      .toBe(contentViewKey('settings', null, 'whats-new'));
  });

  it('ignores the sub-section for any other menu item', () => {
    expect(contentViewKey('triggers', null, 'whats-new')).toBe('triggers');
  });

  it('lets an overlay win over a Settings sub-section too', () => {
    expect(contentViewKey('settings', { type: 'file-preview', path: 'notes.md' }, 'whats-new'))
      .toBe('file:notes.md');
  });

  it('separates two previews of different things', () => {
    const a = contentViewKey(null, { type: 'file-preview', path: 'a.md' }, 'main');
    const b = contentViewKey(null, { type: 'file-preview', path: 'b.md' }, 'main');
    expect(a).not.toBe(b);
    expect(contentViewKey(null, { type: 'url-preview', url: 'https://example.com/1' }, 'main'))
      .not.toBe(contentViewKey(null, { type: 'url-preview', url: 'https://example.com/2' }, 'main'));
  });

  it('separates two inline forms of the same type', () => {
    const a = contentViewKey(null, { type: 'form', form: { type: 'trigger', triggerId: 'alpha' } }, 'main');
    const b = contentViewKey(null, { type: 'form', form: { type: 'trigger', triggerId: 'beta' } }, 'main');
    expect(a).not.toBe(b);
  });

  it('separates two inline forms of different types', () => {
    const a = contentViewKey(null, { type: 'form', form: { type: 'new-app' } }, 'main');
    const b = contentViewKey(null, { type: 'form', form: { type: 'app-edit', appId: 'alpha' } }, 'main');
    expect(a).not.toBe(b);
  });

  it('keeps the same form stable across a re-render', () => {
    const key = () => contentViewKey(null, { type: 'form', form: { type: 'trigger', triggerId: 'alpha' } }, 'main');
    expect(key()).toBe(key());
  });

  it('keys every app-ui overlay the same, whichever app it holds', () => {
    const app = { id: 'alpha', name: 'Alpha' } as never;
    const other = { id: 'beta', name: 'Beta' } as never;
    expect(contentViewKey(null, { type: 'app-ui', app }, 'main')).toBe('app-ui');
    expect(contentViewKey(null, { type: 'app-ui', app: other }, 'main')).toBe('app-ui');
  });
});

describe('settingsViewKey', () => {
  it('is what contentViewKey answers for that sub-section', () => {
    expect(contentViewKey('settings', null, 'whats-new')).toBe(settingsViewKey('whats-new'));
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
    expect(inlineFormKey({ type: 'email-confirm', request: emailRequest }))
      .toBe(inlineFormKey({ type: 'email-confirm', request: emailRequest, sentAt: '2026-08-05T12:00:00Z' }));
  });

  it('keeps the email itself out of the key', () => {
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
