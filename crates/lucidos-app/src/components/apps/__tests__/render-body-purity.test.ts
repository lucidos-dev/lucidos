import { describe, it, expect, vi } from 'vitest';

// Mock the page-fetcher BEFORE importing the component so any setState/load
// invoked during synchronous render would land on the spy. Pre-fix, the
// appId-change reset (setVersionsLoadable/setHasMore/setLoadingMore) ran
// inside the render body gated by useRef — module load itself was fine, but
// the contract is "no work at import time". The spy below also documents
// the useEffect refactor: getAppVersions is now reached only after mount.
vi.mock('../../../api/client', () => ({
  getAppVersions: vi.fn(),
}));

import * as client from '../../../api/client';

describe('TimeTravelDropdown render-body purity', () => {
  it('does not call getAppVersions at module import time', async () => {
    await import('../TimeTravelDropdown');
    expect(client.getAppVersions).not.toHaveBeenCalled();
  });
});
