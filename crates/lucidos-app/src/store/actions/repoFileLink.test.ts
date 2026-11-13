// A `repo:` link in chat opens the repository file preview. It may name the
// repository by id or by name, and says so in the app when it can't open. The
// reported bug: macOS got the link and answered "unsupported scheme".
import { describe, it, expect, beforeEach, vi } from 'vitest';
import type { Repository } from '../store';

const mocks = vi.hoisted(() => ({
  handleNavigationRequest: vi.fn(),
  refreshRepositories: vi.fn(async () => {}),
  showToast: vi.fn(),
}));

vi.mock('./navigation-request', () => ({ handleNavigationRequest: mocks.handleNavigationRequest }));
vi.mock('./repositoriesLoader', () => ({ refreshRepositories: mocks.refreshRepositories }));
vi.mock('../store', async () => {
  const actual = await vi.importActual<typeof import('../store')>('../store');
  return { ...actual, showToast: mocks.showToast };
});

const { matchRepository, openRepoFileLink } = await import('./repoFileLink');
const { repositories, parseRepoPath } = await import('../store');

const LUCIDOS: Repository = { id: '6f1c2a90-0000-5000-8000-000000000001', name: 'Lucidos', path: '/src/lucidos' };
const EXAMPLE: Repository = { id: '6f1c2a90-0000-5000-8000-000000000002', name: 'example-repo', path: '/src/example' };
const EXAMPLE_FORK: Repository = { id: '6f1c2a90-0000-5000-8000-000000000003', name: 'example-repo', path: '/src/fork' };

function link(encoded: string, line?: number, lineEnd?: number) {
  const locator = parseRepoPath(encoded);
  if (!locator) throw new Error(`not a repo path: ${encoded}`);
  return { locator, line, lineEnd };
}

describe('matchRepository', () => {
  it('matches an id', () => {
    expect(matchRepository([LUCIDOS, EXAMPLE], EXAMPLE.id)).toEqual({ kind: 'found', repo: EXAMPLE });
  });

  it('matches an exact name', () => {
    expect(matchRepository([LUCIDOS, EXAMPLE], 'Lucidos')).toEqual({ kind: 'found', repo: LUCIDOS });
  });

  it('matches a name in any case', () => {
    expect(matchRepository([LUCIDOS, EXAMPLE], 'lucidos')).toEqual({ kind: 'found', repo: LUCIDOS });
  });

  it('prefers an id over a repository NAMED like that id', () => {
    const impostor: Repository = { ...EXAMPLE_FORK, name: LUCIDOS.id };
    expect(matchRepository([impostor, LUCIDOS], LUCIDOS.id)).toEqual({ kind: 'found', repo: LUCIDOS });
  });

  it('prefers the exact spelling when two names differ only in case', () => {
    const lower: Repository = { ...EXAMPLE_FORK, name: 'lucidos' };
    expect(matchRepository([LUCIDOS, lower], 'lucidos')).toEqual({ kind: 'found', repo: lower });
  });

  it('calls a shared name ambiguous rather than guessing', () => {
    expect(matchRepository([EXAMPLE, EXAMPLE_FORK], 'example-repo')).toEqual({ kind: 'ambiguous', count: 2 });
  });

  it('reports a miss', () => {
    expect(matchRepository([LUCIDOS], 'nope')).toEqual({ kind: 'missing' });
  });
});

describe('openRepoFileLink', () => {
  beforeEach(() => {
    for (const m of Object.values(mocks)) m.mockClear();
    mocks.refreshRepositories.mockImplementation(async () => {});
    repositories.value = { status: 'loaded', data: [LUCIDOS, EXAMPLE] };
  });

  it('opens a link by id through the file navigate, without re-reading the registry', async () => {
    await openRepoFileLink(link(`repo:${LUCIDOS.id}:file:crates/lucidos-app/src/main.tsx`), 'a chat');
    expect(mocks.refreshRepositories).not.toHaveBeenCalled();
    expect(mocks.handleNavigationRequest).toHaveBeenCalledWith(
      { target: 'file', file_path: `repo:${LUCIDOS.id}:file:crates/lucidos-app/src/main.tsx`, line: undefined, line_end: undefined },
      { source: 'a chat' },
    );
    expect(mocks.showToast).not.toHaveBeenCalled();
  });

  it('opens on the click itself when the cache has the repository', () => {
    // No await: a cache hit must navigate in the click's own task, so the
    // navigation pushes its history row before any fold can run.
    void openRepoFileLink(link(`repo:${LUCIDOS.id}:file:README.md`));
    expect(mocks.handleNavigationRequest).toHaveBeenCalledTimes(1);
  });

  it('rewrites a repository name to its id', async () => {
    await openRepoFileLink(link('repo:lucidos:file:README.md'));
    expect(mocks.handleNavigationRequest).toHaveBeenCalledWith(
      expect.objectContaining({ file_path: `repo:${LUCIDOS.id}:file:README.md` }),
      expect.anything(),
    );
  });

  // A link in agent prose opens the file preview modal instead. The registry
  // match, and the id it resolves to, are the same either way.
  it('hands the resolved file and lines to the opener it is given', async () => {
    const open = vi.fn();
    await openRepoFileLink(link('repo:lucidos:file:src/a.rs', 5, 9), 'a chat', open);
    expect(open).toHaveBeenCalledWith({ file_path: `repo:${LUCIDOS.id}:file:src/a.rs`, line: 5, line_end: 9 });
    expect(mocks.handleNavigationRequest).not.toHaveBeenCalled();
  });

  it('keeps the ref and the cited lines of a `file#<ref>` link', async () => {
    await openRepoFileLink(link('repo:Lucidos:file#origin/main:src/a:b.rs', 10, 20));
    expect(mocks.handleNavigationRequest).toHaveBeenCalledWith(
      { target: 'file', file_path: `repo:${LUCIDOS.id}:file#origin/main:src/a:b.rs`, line: 10, line_end: 20 },
      { source: undefined },
    );
  });

  it('re-reads the registry before calling a repository missing', async () => {
    // Registered moments ago by a sibling thread: the cache has not caught up.
    const fresh: Repository = { id: '6f1c2a90-0000-5000-8000-000000000009', name: 'fresh', path: '/src/fresh' };
    mocks.refreshRepositories.mockImplementation(async () => {
      repositories.value = { status: 'loaded', data: [LUCIDOS, EXAMPLE, fresh] };
    });
    await openRepoFileLink(link('repo:fresh:file:x.md'));
    expect(mocks.refreshRepositories).toHaveBeenCalledTimes(1);
    expect(mocks.handleNavigationRequest).toHaveBeenCalledWith(
      expect.objectContaining({ file_path: `repo:${fresh.id}:file:x.md` }),
      expect.anything(),
    );
  });

  it('loads a cold registry before resolving', async () => {
    repositories.value = { status: 'not-loaded' };
    mocks.refreshRepositories.mockImplementation(async () => {
      repositories.value = { status: 'loaded', data: [LUCIDOS] };
    });
    await openRepoFileLink(link('repo:Lucidos:file:x.md'));
    expect(mocks.handleNavigationRequest).toHaveBeenCalledTimes(1);
  });

  it('toasts a repository that is not registered, naming the file, the repository and the origin', async () => {
    await openRepoFileLink(link('repo:gone:file:src/main.rs'), 'a chat');
    expect(mocks.handleNavigationRequest).not.toHaveBeenCalled();
    expect(mocks.showToast).toHaveBeenCalledWith(
      'Can\'t open "src/main.rs" in repository "gone" (requested by a chat): no repository with that id or name is registered in this workspace',
      'error',
      { key: 'repo-file-link-gone' },
    );
  });

  it('toasts a name two repositories share', async () => {
    repositories.value = { status: 'loaded', data: [EXAMPLE, EXAMPLE_FORK] };
    await openRepoFileLink(link('repo:example-repo:file:a.md'));
    expect(mocks.handleNavigationRequest).not.toHaveBeenCalled();
    expect(mocks.showToast).toHaveBeenCalledWith(
      expect.stringContaining('2 registered repositories share that name, so link it by id'),
      'error',
      expect.anything(),
    );
  });

  it('reports a failed registry read instead of calling the repository missing, and keeps the old list', async () => {
    const before = repositories.value;
    mocks.refreshRepositories.mockImplementation(async () => {
      repositories.value = { status: 'failed', error: 'HTTP 503' };
    });
    await openRepoFileLink(link('repo:elsewhere:file:a.md'));
    expect(mocks.handleNavigationRequest).not.toHaveBeenCalled();
    expect(mocks.showToast).toHaveBeenCalledWith(
      expect.stringContaining('failed to load repositories: HTTP 503'),
      'error',
      expect.anything(),
    );
    expect(repositories.value).toBe(before);
  });
});
