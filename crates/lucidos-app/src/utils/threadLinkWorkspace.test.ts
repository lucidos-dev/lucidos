import { describe, expect, it } from 'vitest';
import {
  THREAD_LINK_RE,
  isThisWorkspace,
  linkedWorkspaceRef,
  linkedWorkspaceRoute,
  resolveLinkedWorkspace,
} from './threadLinkWorkspace';

const UUID = '0b4c1f6e-8d2a-4a51-9c3e-2f6d7a8b9c0d';

describe('THREAD_LINK_RE', () => {
  it('reads a bare, a named and a gateway-qualified link', () => {
    expect(`thread:${UUID}`.match(THREAD_LINK_RE)?.slice(1)).toEqual([undefined, UUID]);
    expect(`thread:dev/${UUID}`.match(THREAD_LINK_RE)?.slice(1)).toEqual(['dev', UUID]);
    expect(`thread:dev@cloud/${UUID}`.match(THREAD_LINK_RE)?.slice(1)).toEqual(['dev@cloud', UUID]);
  });

  it('refuses a qualifier that is no gateway name', () => {
    expect(THREAD_LINK_RE.test(`thread:dev@Cloud/${UUID}`)).toBe(false);
    expect(THREAD_LINK_RE.test(`thread:dev@/${UUID}`)).toBe(false);
  });
});

describe('resolveLinkedWorkspace', () => {
  it('resolves a qualified link as written, wherever the page is', () => {
    expect(resolveLinkedWorkspace('dev@cloud', null)).toEqual({ name: 'dev', gateway: 'cloud' });
    expect(resolveLinkedWorkspace('dev@cloud', 'box')).toEqual({ name: 'dev', gateway: 'cloud' });
  });

  it('resolves an unqualified link on a local page to this gateway', () => {
    expect(resolveLinkedWorkspace('dev', null)).toEqual({ name: 'dev', gateway: null });
  });

  it("resolves an unqualified link inside a remote workspace to that remote gateway, never to this gateway's workspace of that name", () => {
    expect(resolveLinkedWorkspace('dev', 'cloud')).toEqual({ name: 'dev', gateway: 'cloud' });
    expect(linkedWorkspaceRoute(resolveLinkedWorkspace('dev', 'cloud'))).toBe('dev@cloud');
  });
});

describe('isThisWorkspace', () => {
  it('matches the page only on both the name and the gateway', () => {
    expect(isThisWorkspace({ name: 'dev', gateway: 'cloud' }, 'cloud', 'dev')).toBe(true);
    expect(isThisWorkspace({ name: 'dev', gateway: null }, 'cloud', 'dev')).toBe(false);
    expect(isThisWorkspace({ name: 'dev', gateway: 'cloud' }, null, 'dev')).toBe(false);
    expect(isThisWorkspace({ name: 'dev', gateway: null }, null, 'dev')).toBe(true);
  });
});

describe('linkedWorkspaceRef', () => {
  it('qualifies a copied ref inside a remote workspace, so it keeps meaning on a local page', () => {
    expect(linkedWorkspaceRef('dev', 'cloud')).toBe('dev@cloud');
    expect(linkedWorkspaceRef('dev', null)).toBe('dev');
  });
});

describe('linkedWorkspaceRoute', () => {
  it('slugifies the name the way a local link always has', () => {
    expect(linkedWorkspaceRoute({ name: 'My-Notes', gateway: null })).toBe('my-notes');
    expect(linkedWorkspaceRoute({ name: 'My-Notes', gateway: 'cloud' })).toBe('my-notes@cloud');
  });
});
