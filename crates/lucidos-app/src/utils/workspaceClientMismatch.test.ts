import { describe, expect, it } from 'vitest';
import { directWorkspaceHref, mismatchMessage } from '../components/shared/WorkspaceClientMismatchDialog';
import { mismatchFromPickerQuery, mismatchPickerHref, readWorkspaceClientMismatch } from './workspaceClientMismatch';

describe('workspace client mismatch', () => {
  it.each([
    ['this-gateway', '3-4', 'Update this gateway.'],
    ['remote-gateway', '1-1', "Update the gateway on 'cloud'."],
    ['remote-engine', null, "Update the workspace engine for 'demo--two' on 'cloud'."],
  ])('carries the %s diagnosis from a data response to the picker', (behind, theirs, action) => {
    const mismatch = readWorkspaceClientMismatch({
      error: 'workspace_client_mismatch', incompatible: 'demo--two@cloud', behind, ours: 2, theirs, release: '0.63.0',
    })!;
    const href = mismatchPickerHref(mismatch);
    expect(mismatchFromPickerQuery(href.slice(href.indexOf('?')))).toEqual(mismatch);
    const message = mismatchMessage(mismatch);
    expect(message).toContain('This gateway (0.63.0) speaks workspace-client contract 2.');
    expect(message).toContain(action);
    if (theirs === null) expect(message).toContain('unknown contract version');
  });

  it('opens only a gateway from the paired list and ignores any address in the query', () => {
    const query = '?pick&incompatible=demo%40cloud&behind=this-gateway&ours=2&theirs=3-4&address=https://fake.example';
    const mismatch = mismatchFromPickerQuery(query)!;
    expect(directWorkspaceHref(mismatch, [{ name: 'elsewhere', address: 'https://paired.example' }])).toBeNull();
    expect(directWorkspaceHref(mismatch, [{ name: 'cloud', address: 'https://paired.example:4433' }]))
      .toBe('https://paired.example:4433/demo/');
  });
});
