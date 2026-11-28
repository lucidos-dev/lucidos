import { expect, it, vi } from 'vitest';

const replaceDocument = vi.hoisted(() => vi.fn());

vi.mock('../../utils/documentNavigation', () => ({ replaceDocument }));
vi.mock('../../utils/basePath', async (importOriginal) => ({
  ...await importOriginal<typeof import('../../utils/basePath')>(),
  REMOTE_GATEWAY: 'cloud',
  WORKSPACE_ID: 'demo@cloud',
}));

import { ApiError, throwIfNotOk } from './_core';

it('routes a remote mismatch to the picker once and preserves request failure', async () => {
  const reply = () => new Response(JSON.stringify({
    error: 'workspace_client_mismatch',
    incompatible: 'demo@cloud',
    behind: 'this-gateway',
    ours: 2,
    theirs: '3-4',
    release: '0.63.0',
  }), { status: 426 });
  await expect(throwIfNotOk(reply())).rejects.toBeInstanceOf(ApiError);
  await expect(throwIfNotOk(reply())).rejects.toBeInstanceOf(ApiError);
  expect(replaceDocument).toHaveBeenCalledExactlyOnceWith('/~/?pick&incompatible=demo%40cloud&behind=this-gateway&ours=2&theirs=3-4&release=0.63.0');
});
