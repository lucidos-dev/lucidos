import { describe, it, expect, beforeEach } from 'vitest';
import { linkifyPaths, extractAppIdFromHref, extractNavTargetFromHref, extractLocalFileTarget, extractBareAppRef, extractTriggerIdFromHref, browserHandlesHref, _resetLinkifyCacheForTesting } from './linkifyPaths';

describe('extractNavTargetFromHref', () => {
  it.each([
    // Bare panel names — what the system prompt teaches the LLM to write
    ['notifications', 'notifications'],
    ['apps', 'apps'],
    ['app-store', 'app-store'],
    ['triggers', 'triggers'],
    ['changes', 'changes'],
    ['files', 'files'],
    ['settings', 'settings'],
    // data/ prefixed — the LLM naturally reaches for this shape mirroring artifact/app patterns
    ['data/notifications', 'notifications'],
    ['/data/notifications', 'notifications'],
    ['/notifications', 'notifications'],
    // Trailing slash / query / fragment must be tolerated
    ['notifications/', 'notifications'],
    ['notifications?foo=1', 'notifications'],
    ['notifications#x', 'notifications'],
    ['data/notifications/', 'notifications'],
  ])('extracts %s -> %s', (href, expected) => {
    expect(extractNavTargetFromHref(href)).toBe(expected);
  });

  it.each([
    ['', null],
    // Sub-paths must NOT match — `apps/foo` is an app entry, not the panel
    ['apps/foo', null],
    ['apps/foo/index.html', null],
    ['notifications/foo', null],
    ['data/apps/foo/index.html', null],
    // External URLs that happen to contain the panel name
    ['https://example.com/notifications', null],
    ['mailto:user@example.com', null],
    // Unknown panel names stay alone
    ['unknown-panel', null],
    ['data/random', null],
    // Artifact-like paths must stay artifact-like, not nav
    ['artifacts/notes.md', null],
    ['data/artifacts/notes.md', null],
  ])('returns null for %s', (href, expected) => {
    expect(extractNavTargetFromHref(href)).toBe(expected);
  });
});

describe('extractAppIdFromHref', () => {
  it.each([
    // Entry-point shapes — these mean "open the app"
    ['apps/todo', 'todo'],
    ['apps/todo/', 'todo'],
    ['apps/todo/index.html', 'todo'],
    ['/apps/todo/index.html', 'todo'],
    ['data/apps/todo/index.html', 'todo'],
    ['/data/apps/todo/index.html', 'todo'],
    ['apps/habit-tracker/index.html', 'habit-tracker'],
    // Query string / fragment on entry-point hrefs must be stripped before
    // matching — otherwise `apps.find(a => a.id === 'todo?refresh=1')`
    // always misses.
    ['apps/todo?refresh=1', 'todo'],
    ['apps/todo#section', 'todo'],
    ['apps/todo/index.html?v=2', 'todo'],
    ['apps/todo/index.html#anchor', 'todo'],
    // `app:<id>` custom-scheme shorthand. LLMs invent this by analogy to the
    // documented `thread:<UUID>` scheme — the bug report was a Habit Tracker-app
    // link rendered as `[Habit Tracker app](app:habit-tracker)` that dead-ended on
    // macOS Chrome because no handler recognized the scheme.
    ['app:todo', 'todo'],
    ['app:todo/', 'todo'],
    ['app:todo?refresh=1', 'todo'],
    ['app:todo#section', 'todo'],
    ['app:habit-tracker', 'habit-tracker'],
  ])('extracts %s -> %s', (href, expected) => {
    expect(extractAppIdFromHref(href)).toBe(expected);
  });

  it.each([
    // Empty / non-apps shapes
    ['', null],
    ['apps/', null],
    ['apps', null],
    ['notapps/foo', null],
    ['https://example.com/apps/foo/index.html', null],
    ['/some/other/path', null],
    ['#anchor', null],
    ['mailto:user@example.com', null],
    // Sub-files under an app: real files, must fall through to the
    // artifact (file-preview) pipeline, not be claimed as "open the app".
    ['apps/todo/styles.css', null],
    ['apps/todo/scripts/run.sh', null],
    ['apps/todo/main.html', null],
    ['apps/todo/nested/deep/file.json', null],
    // `app:` scheme rejections — empty id, sub-paths, and lookalike schemes
    // (`apple:`, `application:`) must NOT match. Sub-paths under the scheme
    // have no defined meaning; reject so they don't masquerade as an app id.
    ['app:', null],
    ['app:/', null],
    ['app:todo/styles.css', null],
    ['app:todo/scripts/run.sh', null],
    ['apple:todo', null],
    ['application:todo', null],
  ])('returns null for %s', (href, expected) => {
    expect(extractAppIdFromHref(href)).toBe(expected);
  });
});

describe('extractTriggerIdFromHref', () => {
  it.each([
    // The reported shape: the agent invented `trigger:<uuid>` by analogy with
    // `app:<id>` when told to link the trigger itself.
    ['trigger:3f9b21c4-0a7e-4d16-9c58-b2e40d7a1f63', '3f9b21c4-0a7e-4d16-9c58-b2e40d7a1f63'],
    ['trigger:abc-123', 'abc-123'],
    ['trigger:abc-123/', 'abc-123'],       // trailing slash means the same trigger
    ['trigger:abc-123?v=2', 'abc-123'],    // query stripped
    ['trigger:abc-123#top', 'abc-123'],    // fragment stripped
  ])('extracts %s -> %s', (href, expected) => {
    expect(extractTriggerIdFromHref(href)).toBe(expected);
  });

  it.each([
    // The PANEL keeps its own routing: no scheme, so nav claims it.
    ['triggers', null],
    ['data/triggers', null],
    ['/triggers', null],
    // A workspace path under the trigger's folder is the artifact rewriter's.
    ['triggers/nightly-digest', null],
    ['data/triggers/nightly-digest/scripts/run.py', null],
    // Empty id, and a sub-path with no meaning.
    ['trigger:', null],
    ['trigger:/', null],
    ['trigger:abc-123/run', null],
    // Lookalike schemes and other owners.
    ['triggers:abc-123', null],
    ['triggered:abc-123', null],
    ['app:habit-tracker', null],
    ['thread:dev/abc-123', null],
    ['https://example.com/trigger:abc', null],
    ['', null],
  ])('returns null for %s', (href, expected) => {
    expect(extractTriggerIdFromHref(href)).toBe(expected);
  });
});

describe('browserHandlesHref', () => {
  it.each([
    'https://example.com',
    'HTTPS://EXAMPLE.COM',   // scheme is case-insensitive
    'http://example.com/x',
    'mailto:a@example.com',
    'tel:+4712345678',
    'sms:+4712345678',
  ])('%s is the browser to open', (href) => {
    expect(browserHandlesHref(href)).toBe(true);
  });

  it.each([
    // A scheme nothing here claims. Clicked, it does nothing and says nothing,
    // which is what made the reported `trigger:<uuid>` link look dead.
    'trigger:abc-123',
    'app:habit-tracker',
    'note:abc',
    'vscode://file/tmp/x',
    'zoommtg://zoom.us/join',
    // No scheme at all: a relative href into an SPA that has no relative routes.
    'artifacts/notes.md',
    'README',
    '',
    '#section',
    // Not a scheme, despite the colon: a path that happens to contain one.
    'notes: draft',
  ])('%s is not', (href) => {
    expect(browserHandlesHref(href)).toBe(false);
  });
});

describe('extractLocalFileTarget', () => {
  it.each([
    // file:// URLs — always a local file/dir to hand to the OS
    ['file:///Users/me/Downloads/Lucidos_0.12.3_aarch64.dmg', 'file:///Users/me/Downloads/Lucidos_0.12.3_aarch64.dmg'],
    ['file:///Applications/Lucidos.app', 'file:///Applications/Lucidos.app'],
    ['FILE:///tmp/x', 'FILE:///tmp/x'], // scheme is case-insensitive
    // Bare absolute POSIX paths outside the workspace — a staged release dmg,
    // an app folder, etc.
    ['/Users/me/.lucidos/release-worktrees/0.12.3/Lucidos_0.12.3_aarch64.dmg', '/Users/me/.lucidos/release-worktrees/0.12.3/Lucidos_0.12.3_aarch64.dmg'],
    ['/Applications', '/Applications'],
    ['/tmp/build/out.dmg', '/tmp/build/out.dmg'],
    // A directory whose name merely starts with data/apps but isn't the route
    ['/data-backup/snapshot.tar', '/data-backup/snapshot.tar'],
    ['/apps-archive/old.zip', '/apps-archive/old.zip'],
  ])('extracts %s -> %s', (href, expected) => {
    expect(extractLocalFileTarget(href)).toBe(expected);
  });

  it.each([
    // Workspace absolute routes — owned by the artifact / app / nav handlers,
    // must NOT be handed to the OS as a disk path
    ['/data', null],
    ['/data/artifacts/report.pdf', null],
    ['/data/apps/todo/index.html', null],
    ['/apps', null],
    ['/apps/todo/index.html', null],
    ['/apps/todo/styles.css', null],
    // Relative paths are never OS targets (they're workspace-relative)
    ['data/artifacts/report.pdf', null],
    ['apps/todo/index.html', null],
    ['notifications', null],
    ['artifacts/notes.md', null],
    // External web URLs keep their browser / panel-webview behavior
    ['https://example.com/foo.dmg', null],
    ['http://example.com/foo.dmg', null],
    ['mailto:user@example.com', null],
    // Custom schemes the other handlers / renderers own
    ['app:todo', null],
    ['thread:abc-123', null],
    ['', null],
    ['#anchor', null],
  ])('returns null for %s', (href, expected) => {
    expect(extractLocalFileTarget(href)).toBe(expected);
  });
});

describe('extractBareAppRef', () => {
  it.each([
    // Bare single-segment tokens — the reported bug shape and variants.
    ['habit-tracker', 'habit-tracker'],
    ['Habit Tracker', 'Habit Tracker'],   // app name with a space
    ['/habit-tracker', 'habit-tracker'],    // leading slash
    ['habit-tracker/', 'habit-tracker'],    // trailing slash
    ['/habit-tracker/', 'habit-tracker'],   // both
    ['habit-tracker?v=2', 'habit-tracker'], // query stripped
    ['habit-tracker#top', 'habit-tracker'], // fragment stripped
    // Percent-decoded: markdown renders a spaced app-name destination encoded
    // (`[x](<Habit Tracker>)` / `[x](Habit%20Tracker)` → href="Habit%20Tracker").
    ['Habit%20Tracker', 'Habit Tracker'],
    ['Caf%C3%A9', 'Café'],
    ['foo%', 'foo%'], // malformed escape → keep raw, don't drop
  ])('normalizes %s -> %s', (href, expected) => {
    expect(extractBareAppRef(href)).toBe(expected);
  });

  it.each([
    // Any URL scheme disqualifies — real links or handled elsewhere.
    ['app:habit-tracker', null],
    ['https://example.com', null],
    ['http://example.com/x', null],
    ['mailto:user@example.com', null],
    ['file:///tmp/x', null],
    ['thread:abc-123', null],
    // Sub-paths belong to the app / artifact rewriters, not the bare-ref path.
    ['apps/habit-tracker/index.html', null],
    ['data/artifacts/report.pdf', null],
    ['foo/bar', null],
    // Empty / fragment-only.
    ['', null],
    ['/', null],
    ['#anchor', null],
  ])('returns null for %s', (href, expected) => {
    expect(extractBareAppRef(href)).toBe(expected);
  });
});

describe('linkifyPaths', () => {
  it('linkifies bare URLs in text', () => {
    const html = '<p>Visit https://example.com for details</p>';
    const result = linkifyPaths(html, [], []);
    expect(result).toContain('<a href="https://example.com" target="_blank" rel="noopener">');
  });

  it('does not create nested <a> tags for already-linked URLs', () => {
    const html = '<p><a href="https://example.com">https://example.com</a></p>';
    const result = linkifyPaths(html, [], []);
    // Should NOT wrap the link text in another <a>
    expect(result).toBe(html);
  });

  it('does not linkify URLs inside <code> blocks', () => {
    const html = '<p>Use <code>https://localhost:5174/oauth/callback</code></p>';
    const result = linkifyPaths(html, [], []);
    // URL inside <code> should remain as plain text
    expect(result).not.toContain('<a href=');
    expect(result).toContain('<code>https://localhost:5174/oauth/callback</code>');
  });

  it('linkifies artifact paths in text', () => {
    const html = '<p>See user_profile.md for details</p>';
    const result = linkifyPaths(html, ['user_profile.md'], []);
    expect(result).toContain('<a href="#" class="artifact-link" data-path="user_profile.md">user_profile.md</a>');
  });

  it('does not linkify artifact paths inside <a> tags', () => {
    // Neutral href that no rewriter claims — the point is the inner-text
    // artifact name must not become a nested anchor. Don't use `/files`
    // here: that's now a known nav-link target and would legitimately get
    // rewritten by rewriteNavAnchor.
    const html = '<p><a href="https://example.com/x">user_profile.md</a></p>';
    const result = linkifyPaths(html, ['user_profile.md'], []);
    expect(result).toBe(html);
  });

  it('rewrites anchors with data/<known-artifact> href to artifact-link', () => {
    // Real shape from the bug report: LLM wrote
    //   [`artifacts/foo/index.html`](data/artifacts/foo/index.html)
    // pulldown_cmark renders that as
    //   <a href="data/artifacts/foo/index.html"><code>artifacts/foo/index.html</code></a>
    // Without rewriting, the click hits the engine's /data/* static mount as a
    // top-level navigation instead of routing through openFilePreview.
    const html = '<p>Written to <a href="data/artifacts/foo/index.html"><code>artifacts/foo/index.html</code></a> in dev</p>';
    const result = linkifyPaths(html, ['artifacts/foo/index.html'], []);
    expect(result).toContain('class="artifact-link"');
    expect(result).toContain('data-path="artifacts/foo/index.html"');
    expect(result).not.toContain('href="data/artifacts/foo/index.html"');
    // Visible text (the inner <code>...) is preserved
    expect(result).toContain('<code>artifacts/foo/index.html</code>');
  });

  it('rewrites anchors with absolute /data/<known-artifact> href', () => {
    const html = '<p><a href="/data/artifacts/foo.md">link</a></p>';
    const result = linkifyPaths(html, ['artifacts/foo.md'], []);
    expect(result).toContain('class="artifact-link"');
    expect(result).toContain('data-path="artifacts/foo.md"');
  });

  it('rewrites anchors with bare artifacts/ href (no data/ prefix)', () => {
    const html = '<p><a href="artifacts/foo.md">link</a></p>';
    const result = linkifyPaths(html, ['artifacts/foo.md'], []);
    expect(result).toContain('class="artifact-link"');
    expect(result).toContain('data-path="artifacts/foo.md"');
  });

  it('rewrites a data-path anchor the cached artifact list does NOT know', () => {
    // The reported bug. `lucidos data write` lands a file and prints exactly
    // this link for the agent to paste, but the artifacts cache is refreshed by
    // SSE and does not have the path yet. Gating on the cache left a raw
    // relative href, so the click navigated to /<slug>/artifacts/... and the
    // SPA fallback reloaded the whole workspace. Resolved by shape instead.
    const html = '<p><a href="data/artifacts/unknown.md">link</a></p>';
    const result = linkifyPaths(html, ['artifacts/foo.md'], []);
    expect(result).toContain('class="artifact-link"');
    expect(result).toContain('data-path="artifacts/unknown.md"');
    expect(result).not.toContain('href="data/artifacts/unknown.md"');
  });

  it('rewrites a data-path anchor with an EMPTY artifact list (nothing loaded yet)', () => {
    const html = '<p><a href="artifacts/pr-review/pr-1582/index.html">report</a></p>';
    const result = linkifyPaths(html, [], []);
    expect(result).toContain('class="artifact-link"');
    expect(result).toContain('data-path="artifacts/pr-review/pr-1582/index.html"');
  });

  it.each([
    'knowhow/myapp/notes.md',
    'triggers/daily/run.md',
    'system-knowhow/js-sdk.md',
    '/artifacts/report.pdf',
    '/data/knowhow/x.md',
    'artifacts/report.html?v=2',
    'artifacts/report.html#top',
  ])('rewrites the data-path shape %s even with no cached paths', (href) => {
    const result = linkifyPaths(`<p><a href="${href}">x</a></p>`, [], []);
    expect(result).toContain('class="artifact-link"');
  });

  it.each([
    // A bare sub-tree is a directory, not a file. `apps` / `triggers` are also
    // nav panels and are claimed earlier; the rest simply name no file.
    'artifacts',
    'artifacts/',
    'README',
    'some/unknown/path.md',
    'https://example.com/artifacts/foo.md',
  ])('leaves %s alone (not a data-path shape)', (href) => {
    const html = `<p><a href="${href}">x</a></p>`;
    const result = linkifyPaths(html, [], []);
    expect(result).not.toContain('artifact-link');
    expect(result).toContain(`href="${href}"`);
  });

  it('leaves external-URL anchors alone even when artifact paths exist', () => {
    const html = '<p><a href="https://example.com">site</a></p>';
    const result = linkifyPaths(html, ['artifacts/foo.md'], []);
    expect(result).toBe(html);
  });

  it('preserves non-href attributes when rewriting an artifact anchor', () => {
    // pulldown_cmark doesn't emit title/target on plain markdown links today,
    // but other renderers might — the rewrite should keep them.
    const html = '<p><a href="data/artifacts/foo.md" title="hover" target="_blank">link</a></p>';
    const result = linkifyPaths(html, ['artifacts/foo.md'], []);
    expect(result).toContain('class="artifact-link"');
    expect(result).toContain('data-path="artifacts/foo.md"');
    expect(result).toContain('title="hover"');
    expect(result).toContain('target="_blank"');
    expect(result).not.toContain('href="data/artifacts/foo.md"');
  });

  it('linkifies artifact paths inside <code> tags (LLMs wrap paths in backticks)', () => {
    const html = '<p>Run <code>cat user_profile.md</code></p>';
    const result = linkifyPaths(html, ['user_profile.md'], []);
    expect(result).toContain('artifact-link');
    expect(result).toContain('data-path="user_profile.md"');
  });

  it('linkifies a bare prose path the cached list does NOT know', () => {
    // The reported bug. The agent is told to write full paths, because they
    // become links. Then ffmpeg's output was placed under `data/artifacts/` by
    // a shell `cp` inside run_bash, which announces nothing. So the cache never
    // learned the file and the promised link rendered as plain text. Resolved
    // by shape, exactly as a deliberate anchor already is.
    const html = '<p>artifacts/marketing/product-demo/product-demo-short-v8.mp4</p>';
    const result = linkifyPaths(html, [], []);
    expect(result).toContain('class="artifact-link"');
    expect(result).toContain('data-path="artifacts/marketing/product-demo/product-demo-short-v8.mp4"');
  });

  it.each([
    ['knowhow/domain/guide.md', 'knowhow/domain/guide.md'],
    ['triggers/daily/run.md', 'triggers/daily/run.md'],
    ['system-knowhow/js-sdk.md', 'system-knowhow/js-sdk.md'],
    // The written form is preserved as the link TEXT; `data-path` normalizes.
    ['data/artifacts/report.html', 'artifacts/report.html'],
    ['/artifacts/report.html', 'artifacts/report.html'],
    ['artifacts/.hidden.md', 'artifacts/.hidden.md'],
    ['artifacts/archive.tar.gz', 'artifacts/archive.tar.gz'],
  ])('resolves the prose path %s by shape with no cached paths', (written, stored) => {
    const result = linkifyPaths(`<p>See ${written} now</p>`, [], []);
    expect(result).toContain(`data-path="${stored}"`);
    expect(result).toContain(`>${written}</a>`);
  });

  it.each([
    // A directory, whichever depth. The extension rule is what tells these from
    // a file, and it is the guard an anchor does not need.
    ['Look in artifacts/marketing for it'],
    ['Everything is under artifacts now'],
    ['Look in artifacts/a/b/c for it'],
    // Shape cannot tell a bare filename from an ordinary word, so it stays
    // cache-gated. Covered as a positive above with the list loaded.
    ['See user_profile.md for details'],
    // Glued to a preceding word: not a path, and the boundary rejects it.
    ['the xartifacts/foo.md thing'],
  ])('leaves %s alone in prose', (text) => {
    const result = linkifyPaths(`<p>${text}</p>`, [], []);
    expect(result).not.toContain('artifact-link');
  });

  it('never carves a data path out of a URL', () => {
    // The boundary rejects a preceding `/`, so the URL keeps its own anchor and
    // no nested one appears inside it. Inside <code> the URL pass is skipped
    // while the path pass still runs, which is where this would break first.
    const anchored = '<p><a href="https://example.com/artifacts/foo.md">https://example.com/artifacts/foo.md</a></p>';
    expect(linkifyPaths(anchored, [], [])).toBe(anchored);
    const inCode = '<p>Run <code>curl https://example.com/artifacts/foo.md</code></p>';
    expect(linkifyPaths(inCode, [], [])).toBe(inCode);
  });

  it.each([
    ['See artifacts/notes.md.', '.'],
    ['See artifacts/notes.md, then', ','],
    ['See (artifacts/notes.md) now', ')'],
  ])('keeps trailing punctuation outside the link: %s', (text, punctuation) => {
    const result = linkifyPaths(`<p>${text}</p>`, [], []);
    expect(result).toContain('data-path="artifacts/notes.md"');
    expect(result).toContain(`>artifacts/notes.md</a>${punctuation}`);
  });

  it('linkifies a bare prose path inside <code> with no cached paths', () => {
    const html = '<p>Run <code>ffmpeg -i artifacts/a.mp4 artifacts/b.mp4</code></p>';
    const result = linkifyPaths(html, [], []);
    expect(result).toContain('data-path="artifacts/a.mp4"');
    expect(result).toContain('data-path="artifacts/b.mp4"');
  });

  it('stops at an HTML entity rather than half-eating it', () => {
    // renderMarkdown escapes `&`, so the segment holds `artifacts/a&amp;b.md`.
    // The match must terminate at the entity, leaving `artifacts/a` with no
    // extension and therefore no link, rather than linking a mangled path.
    const result = linkifyPaths('<p>See artifacts/a&amp;b.md here</p>', [], []);
    expect(result).not.toContain('artifact-link');
  });

  it.each([
    'file:artifacts/report.pdf',
    'https:artifacts/report.pdf',
    'mailto:artifacts/report.pdf',
  ])('never claims the workspace half of the scheme URL %s', (text) => {
    // `hasUrlScheme` owns "is this a relative path", and it rejects these as an
    // href. The prose boundary accepts `:`, so without the scheme guard the
    // suffix became an artifact link that no anchor would ever produce.
    const result = linkifyPaths(`<p>${text}</p>`, [], []);
    expect(result).not.toContain('artifact-link');
  });

  it('links nothing rather than a different file when the extension continues', () => {
    // The extension is alphanumeric, so a filename ending `-1` used to match
    // only its prefix and link `artifacts/archive.tar.zst`, a path the text
    // never named. No link beats a link to the wrong file.
    const result = linkifyPaths('<p>See artifacts/archive.tar.zst-1 now</p>', [], []);
    expect(result).not.toContain('artifact-link');
  });

  it.each([
    ['artifacts/file.d.ts.map', 'artifacts/file.d.ts.map'],
    ['artifacts/archive.tar.gz', 'artifacts/archive.tar.gz'],
    // A query string still links the base path, matching what the cached-list
    // matcher does. `extractDataPathTarget` strips it anyway.
    ['artifacts/report.html?v=2', 'artifacts/report.html'],
    ['artifacts/report.html#top', 'artifacts/report.html'],
  ])('the trailing guard leaves %s alone', (written, stored) => {
    const result = linkifyPaths(`<p>See ${written} now</p>`, [], []);
    expect(result).toContain(`data-path="${stored}"`);
  });

  it.each([
    // Inside <code> the URL pass never runs, so nothing downstream would notice
    // an anchor spliced into a shell command the reader is meant to copy.
    '<p>Run <code>curl https://example.com/?next=artifacts/foo.md</code></p>',
    '<p>Run <code>curl https://example.com/x#artifacts/foo.md</code></p>',
    '<pre><code>curl https://ex.com/a?p=artifacts/b.md&amp;q=1</code></pre>',
  ])('never links a workspace-shaped value inside a URL: %s', (html) => {
    expect(linkifyPaths(html, ['artifacts/foo.md', 'artifacts/b.md'], [])).toBe(html);
  });

  it('still links a path that merely follows a URL', () => {
    const result = linkifyPaths('<p><code>see https://example.com and artifacts/foo.md</code></p>', [], []);
    expect(result).toContain('data-path="artifacts/foo.md"');
  });

  it('prefers the longer shape match over a shorter cached one at the same start', () => {
    // Both matchers feed one precedence pass, so the span the text actually
    // names wins. Gating that on the cache would link `artifacts/notes.md` and
    // strand `.bak` outside it.
    const result = linkifyPaths('<p>See artifacts/notes.md.bak now</p>', ['artifacts/notes.md'], []);
    expect(result).toContain('data-path="artifacts/notes.md.bak"');
    expect(result).not.toContain('data-path="artifacts/notes.md"');
  });

  it('rewrites anchors with data/notifications href to nav-link (the bug-report shape)', () => {
    // Real shape from the bug report — last response in the thread
    // 664b657a-... wrote:
    //   Open it: [Notifications](data/notifications)
    // pulldown_cmark renders that as
    //   <a href="data/notifications">Notifications</a>
    // Without rewriting, the click hits the engine's /data/* static mount and
    // 404s (no `notifications` folder under data/) instead of opening the
    // notifications inbox panel.
    const html = '<p>Open it: <a href="data/notifications">Notifications</a>.</p>';
    const result = linkifyPaths(html, [], []);
    expect(result).toContain('class="nav-link"');
    expect(result).toContain('data-nav-target="notifications"');
    expect(result).toContain('href="#"');
    expect(result).not.toContain('href="data/notifications"');
    expect(result).toContain('>Notifications</a>');
  });

  it.each([
    'notifications',
    '/notifications',
    'data/notifications',
    '/data/notifications',
    'notifications/',
    'notifications?refresh=1',
  ])('rewrites anchor with href=%s to nav-link[notifications]', (href) => {
    const html = `<p><a href="${href}">Inbox</a></p>`;
    const result = linkifyPaths(html, [], []);
    expect(result).toContain('class="nav-link"');
    expect(result).toContain('data-nav-target="notifications"');
  });

  it.each([
    ['apps', 'apps'],
    ['triggers', 'triggers'],
    ['changes', 'changes'],
    ['files', 'files'],
    ['settings', 'settings'],
  ])('rewrites bare %s href to nav-link[%s]', (href, target) => {
    const html = `<p><a href="${href}">link</a></p>`;
    const result = linkifyPaths(html, [], []);
    expect(result).toContain('class="nav-link"');
    expect(result).toContain(`data-nav-target="${target}"`);
  });

  it('rewrites trigger:<id> to a trigger-link (the bug-report shape)', () => {
    // The exact href the agent wrote when told "u must link to the trigger".
    // Before the rewriter it stayed a plain anchor whose unknown scheme the
    // browser silently ignored.
    const html = '<p><a href="trigger:3f9b21c4-0a7e-4d16-9c58-b2e40d7a1f63">Nightly digest</a></p>';
    const result = linkifyPaths(html, [], []);
    expect(result).toContain('class="trigger-link"');
    expect(result).toContain('data-trigger-id="3f9b21c4-0a7e-4d16-9c58-b2e40d7a1f63"');
    expect(result).toContain('href="#"');
    expect(result).not.toContain('href="trigger:');
  });

  it('rewrites a trigger link with NO trigger list loaded', () => {
    // Deliberately unlike the app rewriter: the id is not checked against a
    // cached projection, so a trigger created moments ago still links.
    const html = '<p><a href="trigger:brand-new">New</a></p>';
    expect(linkifyPaths(html, [], [])).toContain('data-trigger-id="brand-new"');
  });

  it('does NOT rewrite the triggers PANEL href to a trigger-link', () => {
    // Bug 1 and bug 2 are different destinations and must stay different.
    const html = '<p><a href="triggers">Triggers</a></p>';
    const result = linkifyPaths(html, [], []);
    expect(result).toContain('class="nav-link"');
    expect(result).toContain('data-nav-target="triggers"');
    expect(result).not.toContain('trigger-link');
  });

  it('does NOT rewrite a triggers/<slug> path to a trigger-link', () => {
    // A file under the trigger's folder stays the artifact rewriter's.
    const html = '<p><a href="triggers/nightly-digest/scripts/run.py">run.py</a></p>';
    const result = linkifyPaths(html, [], []);
    expect(result).toContain('class="artifact-link"');
    expect(result).not.toContain('trigger-link');
  });

  it('does NOT rewrite apps/<id>/index.html to nav-link (app rewriter must win)', () => {
    // Regression guard: the nav rewriter must NOT claim `apps/foo/index.html`.
    // That's the app-entry shape and belongs to the app rewriter, which is
    // already tested above. The nav rewriter only matches the bare panel.
    const html = '<p><a href="apps/todo/index.html">Todo</a></p>';
    const result = linkifyPaths(html, [], [{ name: 'Todo', id: 'todo' }]);
    expect(result).toContain('class="app-link"');
    expect(result).toContain('data-app-id="todo"');
    expect(result).not.toContain('class="nav-link"');
  });

  it('does NOT rewrite unknown bare hrefs (no false positives)', () => {
    const html = '<p><a href="unknown-panel">link</a></p>';
    const result = linkifyPaths(html, [], []);
    expect(result).toContain('href="unknown-panel"');
    expect(result).not.toContain('class="nav-link"');
  });

  it('does NOT rewrite external URLs that happen to contain a panel name', () => {
    const html = '<p><a href="https://example.com/notifications">site</a></p>';
    const result = linkifyPaths(html, [], []);
    expect(result).toBe(html);
  });

  it('does NOT rewrite artifact paths to nav-link', () => {
    // `artifacts/...` must keep flowing to the artifact rewriter, not get
    // hijacked by a too-permissive nav matcher.
    const html = '<p><a href="data/artifacts/notes.md">notes</a></p>';
    const result = linkifyPaths(html, ['artifacts/notes.md'], []);
    expect(result).toContain('class="artifact-link"');
    expect(result).not.toContain('class="nav-link"');
  });

  it('does NOT linkify a bare app name in text (auto-scan removed)', () => {
    // The bare-text app-name/id scan was removed — an app named in prose is
    // plain text. Apps become links only via an explicit anchor (covered below).
    const html = '<p>Use the Todo app</p>';
    const result = linkifyPaths(html, [], [{ name: 'Todo', id: 'todo' }]);
    expect(result).toBe(html);
    expect(result).not.toContain('app-link');
    expect(result).not.toContain('data-app-id');
  });

  it('does NOT linkify a bare app id in text either (auto-scan removed)', () => {
    const html = '<p>Open Job Tracker: job-tracker</p>';
    const result = linkifyPaths(html, [], [{ name: 'Job Tracker', id: 'job-tracker' }]);
    expect(result).toBe(html);
    expect(result).not.toContain('app-link');
  });

  it('does not linkify app names inside <a> tags', () => {
    // Neutral href that no rewriter claims — the point is the inner-text
    // app name must not become a nested anchor. Don't use `/apps` here:
    // that's now a known nav-link target and would legitimately get
    // rewritten by rewriteNavAnchor.
    const html = '<p><a href="https://example.com/x">Todo</a></p>';
    const result = linkifyPaths(html, [], [{ name: 'Todo', id: 'todo' }]);
    expect(result).toBe(html);
  });

  it('REGRESSION: apps/<id>/index.html beats artifact-link even when path is in the artifact list', () => {
    // Real-world scenario the unit tests missed: lucidos.data.list() returns
    // ALL files under data/, NOT just artifacts/. So a real workspace's
    // `paths` array contains apps/<id>/index.html for every app. Without
    // the app-rewriter taking precedence, rewriteArtifactAnchor matches
    // first → .artifact-link → openFilePreview → user sees the rendered HTML
    // file in the preview panel instead of the running app.
    const paths = ['artifacts/notes.md', 'apps/habit-tracker/index.html'];
    const apps = [{ name: 'Habit Tracker', id: 'habit-tracker' }];
    const html = '<p>Watch it in <a href="apps/habit-tracker/index.html">Habit Tracker</a>.</p>';
    const result = linkifyPaths(html, paths, apps);
    expect(result).toContain('class="app-link"');
    expect(result).toContain('data-app-id="habit-tracker"');
    expect(result).not.toContain('class="artifact-link"');
  });

  it('REGRESSION: apps/<id>/<sub-file> still becomes artifact-link (user wants file preview)', () => {
    // Inverse: sub-files under an app's folder are real files; clicking
    // should preview them, not open the app. Only the canonical entry
    // (id, id/, id/index.html) routes to the app.
    const paths = ['apps/habit-tracker/scripts/run.sh'];
    const apps = [{ name: 'Habit Tracker', id: 'habit-tracker' }];
    const html = '<p><a href="apps/habit-tracker/scripts/run.sh">run.sh</a></p>';
    const result = linkifyPaths(html, paths, apps);
    expect(result).toContain('class="artifact-link"');
    expect(result).toContain('data-path="apps/habit-tracker/scripts/run.sh"');
    expect(result).not.toContain('class="app-link"');
  });

  it('rewrites anchors with apps/<id>/index.html href to app-link (bare prefix)', () => {
    // Real shape from the bug report: LLM wrote
    //   [Habit Tracker](apps/habit-tracker/index.html)
    // pulldown_cmark renders that as
    //   <a href="apps/habit-tracker/index.html">Habit Tracker</a>
    // Without rewriting, the click hits the engine's /data/* static mount as a
    // top-level navigation and the user sees a file preview, not the running app.
    const html = '<p>Watch it in <a href="apps/habit-tracker/index.html">Habit Tracker</a>.</p>';
    const result = linkifyPaths(html, [], [{ name: 'Habit Tracker', id: 'habit-tracker' }]);
    expect(result).toContain('class="app-link"');
    expect(result).toContain('data-app-id="habit-tracker"');
    expect(result).not.toContain('href="apps/habit-tracker/index.html"');
    expect(result).toContain('>Habit Tracker</a>');
  });

  it('rewrites anchors with /apps/<id>/index.html href (leading slash)', () => {
    const html = '<p><a href="/apps/todo/index.html">Todo</a></p>';
    const result = linkifyPaths(html, [], [{ name: 'Todo', id: 'todo' }]);
    expect(result).toContain('class="app-link"');
    expect(result).toContain('data-app-id="todo"');
  });

  it('rewrites anchors with data/apps/<id>/index.html href', () => {
    const html = '<p><a href="data/apps/todo/index.html">Todo</a></p>';
    const result = linkifyPaths(html, [], [{ name: 'Todo', id: 'todo' }]);
    expect(result).toContain('class="app-link"');
    expect(result).toContain('data-app-id="todo"');
  });

  it('rewrites anchors with /data/apps/<id>/index.html href', () => {
    const html = '<p><a href="/data/apps/todo/index.html">Todo</a></p>';
    const result = linkifyPaths(html, [], [{ name: 'Todo', id: 'todo' }]);
    expect(result).toContain('class="app-link"');
    expect(result).toContain('data-app-id="todo"');
  });

  it('rewrites anchors with apps/<id> href (no trailing file)', () => {
    const html = '<p><a href="apps/todo">Todo</a></p>';
    const result = linkifyPaths(html, [], [{ name: 'Todo', id: 'todo' }]);
    expect(result).toContain('class="app-link"');
    expect(result).toContain('data-app-id="todo"');
  });

  it('routes apps/<id>/<sub-file> to a file preview, not the app', () => {
    // Sub-files under an app's folder are real files. Clicking should preview
    // them, not open the app. Only the canonical entry (id, id/, id/index.html)
    // routes to the app. The app rewriter declines the sub-file, and the
    // artifact rewriter then claims it by shape.
    const html = '<p><a href="apps/todo/styles.css">Todo styles</a></p>';
    const result = linkifyPaths(html, [], [{ name: 'Todo', id: 'todo' }]);
    expect(result).not.toContain('class="app-link"');
    expect(result).toContain('class="artifact-link"');
    expect(result).toContain('data-path="apps/todo/styles.css"');
  });

  it('routes apps/<unknown-id>/index.html to a file preview, never the app', () => {
    // The app gate stays strict (only a KNOWN id opens an app), but the href is
    // still a path under data/apps/, so it previews as a file rather than
    // escaping to the browser and reloading the workspace.
    const html = '<p><a href="apps/no-such-app/index.html">link</a></p>';
    const result = linkifyPaths(html, [], [{ name: 'Todo', id: 'todo' }]);
    expect(result).not.toContain('app-link');
    expect(result).toContain('class="artifact-link"');
    expect(result).toContain('data-path="apps/no-such-app/index.html"');
  });

  it('rewrites anchors with app:<id> custom-scheme href to app-link', () => {
    // Real shape from the bug-report thread: LLM wrote
    //   Open the [Habit Tracker app](app:habit-tracker) and switch to the Backtest tab.
    // pulldown_cmark renders that as
    //   <a href="app:habit-tracker">Habit Tracker app</a>
    // Without rewriting, the click falls through to the browser's default
    // navigation, which tries to open the unknown `app:` URL scheme — Chrome
    // on macOS shows "address not understood".
    const html = '<p>Open the <a href="app:habit-tracker">Habit Tracker app</a>.</p>';
    const result = linkifyPaths(html, [], [{ name: 'Habit Tracker', id: 'habit-tracker' }]);
    expect(result).toContain('class="app-link"');
    expect(result).toContain('data-app-id="habit-tracker"');
    expect(result).not.toContain('href="app:habit-tracker"');
    expect(result).toContain('>Habit Tracker app</a>');
  });

  it.each([
    'app:todo',
    'app:todo/',
    'app:todo?refresh=1',
    'app:todo#section',
  ])('rewrites anchor with app:<id> variant href=%s to app-link', (href) => {
    const html = `<p><a href="${href}">Todo</a></p>`;
    const result = linkifyPaths(html, [], [{ name: 'Todo', id: 'todo' }]);
    expect(result).toContain('class="app-link"');
    expect(result).toContain('data-app-id="todo"');
  });

  it.each([
    'app',
    'app/',
    'app:',
    'app:/',
  ])('rewrites bare [Name](%s) (no id) to app-link via the anchor TEXT', (href) => {
    // Real shape from the bug-report thread: the coding agent, told only to
    // "mention the app name", over-helpfully wrote a markdown link with a bare
    // `app` href and no id:
    //   The preview auto-refreshes in [Site Publisher](app) — hit Publish.
    // pulldown_cmark renders that as `<a href="app">Site Publisher</a>`, which
    // matches neither the `app:<id>` / `apps/<id>` shapes (no id) nor a nav
    // panel (`app` singular isn't one). Left alone the browser resolves the
    // relative href against the gateway base (`/<slug>/`) → `/<slug>/app`, a
    // dead end. Recover the app from the anchor's visible text instead.
    const html = `<p>Open <a href="${href}">Site Publisher</a> and publish.</p>`;
    const result = linkifyPaths(html, [], [{ name: 'Site Publisher', id: 'site-publisher' }]);
    expect(result).toContain('class="app-link"');
    expect(result).toContain('data-app-id="site-publisher"');
    expect(result).toContain('>Site Publisher</a>');
    expect(result).not.toContain(`href="${href}"`);
  });

  it('resolves a bare [id](app) anchor by app id text too', () => {
    const html = '<p>See <a href="app">site-publisher</a>.</p>';
    const result = linkifyPaths(html, [], [{ name: 'Site Publisher', id: 'site-publisher' }]);
    expect(result).toContain('data-app-id="site-publisher"');
  });

  it('leaves a bare [unknown](app) anchor alone when the text names no known app', () => {
    const html = '<p>Open <a href="app">Some Other Thing</a>.</p>';
    const result = linkifyPaths(html, [], [{ name: 'Site Publisher', id: 'site-publisher' }]);
    expect(result).toContain('href="app"');
    expect(result).not.toContain('app-link');
  });

  it('does not treat the apps panel href as a bare app link', () => {
    // `apps` (plural) is a nav panel target — it must keep routing to the Apps
    // list, never get hijacked by the bare-app text fallback.
    const html = '<p>Browse <a href="apps">Site Publisher</a>.</p>';
    const result = linkifyPaths(html, [], [{ name: 'Site Publisher', id: 'site-publisher' }]);
    expect(result).toContain('class="nav-link"');
    expect(result).toContain('data-nav-target="apps"');
    expect(result).not.toContain('app-link');
  });

  it.each([
    'habit-tracker',      // bare id
    '/habit-tracker',     // leading slash
    'habit-tracker/',     // trailing slash
    'habit-tracker?v=2',  // query
  ])('rewrites a bare app-id href [text](%s) to an app-link', (href) => {
    // The reported bug: the LLM wrote a link with the app id as a bare relative
    // href, mirroring `[Notifications](notifications)`. None of the strict
    // rewriters claim it (no apps/ prefix, no app: scheme, not a nav panel), so
    // left alone the browser navigates to the relative href and the SPA fallback
    // reloads the whole workspace. Text ≠ name, so the match is via the href.
    const html = `<p>Open <a href="${href}">the tracker</a> here.</p>`;
    const result = linkifyPaths(html, [], [{ name: 'Habit Tracker', id: 'habit-tracker' }]);
    expect(result).toContain('href="#"');
    expect(result).toContain('class="app-link"');
    expect(result).toContain('data-app-id="habit-tracker"');
    expect(result).toContain('>the tracker</a>');
    expect(result).not.toContain(`href="${href}"`);
  });

  it('rewrites a bare app-NAME href to an app-link', () => {
    const html = '<p>Open <a href="Habit Tracker">the list</a>.</p>';
    const result = linkifyPaths(html, [], [{ name: 'Habit Tracker', id: 'habit-tracker' }]);
    expect(result).toContain('data-app-id="habit-tracker"');
    expect(result).not.toContain('href="Habit Tracker"');
  });

  it('rewrites a percent-encoded bare app-NAME href (spaced name)', () => {
    // Markdown renders `[x](<Habit Tracker>)` / `[x](Habit%20Tracker)` as
    // href="Habit%20Tracker"; the token must be decoded to match the raw name.
    const html = '<p>Open <a href="Habit%20Tracker">the tracker</a>.</p>';
    const result = linkifyPaths(html, [], [{ name: 'Habit Tracker', id: 'habit-tracker' }]);
    expect(result).toContain('class="app-link"');
    expect(result).toContain('data-app-id="habit-tracker"');
    expect(result).not.toContain('href="Habit%20Tracker"');
  });

  it('leaves a bare href that names no known app alone', () => {
    const html = '<p>See <a href="README">the readme</a>.</p>';
    const result = linkifyPaths(html, [], [{ name: 'Habit Tracker', id: 'habit-tracker' }]);
    expect(result).toContain('href="README"');
    expect(result).not.toContain('app-link');
  });

  it('a bare nav-panel href still wins over a same-named bare app', () => {
    // Reserved panel names route to their panel — the nav rewriter runs before
    // the bare-app-ref rewriter, so even an app literally named `notifications`
    // can't hijack the panel link.
    const html = '<p>Open <a href="notifications">alerts</a>.</p>';
    const result = linkifyPaths(html, [], [{ name: 'notifications', id: 'notifications' }]);
    expect(result).toContain('class="nav-link"');
    expect(result).toContain('data-nav-target="notifications"');
    expect(result).not.toContain('app-link');
  });

  it('leaves app:<unknown-id> anchors alone (same gate as apps/<unknown-id>)', () => {
    const html = '<p><a href="app:no-such-app">link</a></p>';
    const result = linkifyPaths(html, [], [{ name: 'Todo', id: 'todo' }]);
    expect(result).toContain('href="app:no-such-app"');
    expect(result).not.toContain('app-link');
  });

  it('emits href="#" on every generated link so iOS Safari/PWA fires tap→click', () => {
    // iOS Safari (and PWA in standalone mode) silently drops tap→click
    // translation on `<a>` without href — even with `cursor: pointer`, the
    // delegated chat click handler never fires and the user sees a dead link.
    // preventDefault in ChatExchange's handleLinkClick suppresses the `#`
    // scroll-to-top, so href="#" is purely an iOS-clickability marker, not a
    // navigation target. Covers the three href="#" producers: a text
    // artifact-link, a rewritten app anchor, and a rewritten artifact anchor.
    // (App names are no longer scanned in text, so the app-link comes from an
    // explicit `[the Todo app](app:todo)` link.)
    const html = '<p>Check user_profile.md or open <a href="app:todo">the Todo app</a>, or follow this <a href="data/artifacts/foo.md">file link</a></p>';
    const result = linkifyPaths(html, ['user_profile.md', 'artifacts/foo.md'], [{ name: 'Todo', id: 'todo' }]);
    expect(result).toContain('<a href="#" class="artifact-link" data-path="user_profile.md">');
    expect(result).toContain('<a href="#" class="app-link" data-app-id="todo">');
    expect(result).toContain('<a href="#" class="artifact-link" data-path="artifacts/foo.md">');
    // No bare `<a class=` (i.e. href-less <a>) for any of our linkifier classes.
    expect(result).not.toMatch(/<a class="(artifact-link|app-link)"/);
  });

  it('handles real-world pulldown_cmark HTML with URLs in <a> and <code>', () => {
    // Actual HTML from the bug report — pulldown_cmark output with auto-linked URL
    // and URL inside <code>
    const html = [
      '<p><strong><a href="https://portal.azure.com/#blade/Microsoft_AAD_RegisteredApps" target="_blank" rel="noopener">',
      'https://portal.azure.com/#blade/Microsoft_AAD_RegisteredApps</a></strong></p>',
      '<ul><li>Redirect URI: <code>https://localhost:5174/oauth/callback</code></li></ul>',
    ].join('');
    const result = linkifyPaths(html, [], []);
    // No nested <a> for the portal URL
    expect(result).not.toMatch(/<a[^>]*><a/);
    // No <a> inside <code>
    expect(result).toContain('<code>https://localhost:5174/oauth/callback</code>');
  });

  it('preserves artifacts/ prefix in data-path for API compatibility', () => {
    const html = '<p>See artifacts/projects/sample/notes.md for details</p>';
    const result = linkifyPaths(html, ['artifacts/projects/sample/notes.md'], []);
    // data-path must keep the artifacts/ prefix so the backend API validation passes
    expect(result).toContain('data-path="artifacts/projects/sample/notes.md"');
    expect(result).toContain('>artifacts/projects/sample/notes.md</a>');
  });

  it('resolves bare path to full store path with artifacts/ prefix', () => {
    const html = '<p>Check projects/sample/notes.md for updates</p>';
    const result = linkifyPaths(html, ['artifacts/projects/sample/notes.md'], []);
    // Even though text omits the prefix, data-path must include it for the API
    expect(result).toContain('data-path="artifacts/projects/sample/notes.md"');
    // Display text should match what the user wrote (without prefix)
    expect(result).toContain('>projects/sample/notes.md</a>');
  });

  it('preserves non-artifacts prefixes as-is (knowhow/, apps/)', () => {
    const html = '<p>Read knowhow/cooking.md</p>';
    const result = linkifyPaths(html, ['knowhow/cooking.md'], []);
    expect(result).toContain('data-path="knowhow/cooking.md"');
  });

  it('handles empty input', () => {
    expect(linkifyPaths('', [], [])).toBe('');
  });

  it('preserves HTML structure with no paths or apps', () => {
    const html = '<p>Hello <strong>world</strong></p>';
    expect(linkifyPaths(html, [], [])).toBe(html);
  });

  it('linkifies paths correctly when path list is very large', () => {
    // Simulates a workspace with thousands of artifacts — a real workspace had 7458
    // when WebKit's YARR threw "regular expression too large" at runtime.
    const paths = Array.from(
      { length: 5000 },
      (_, i) => `artifacts/path/file_${i.toString().padStart(6, '0')}.md`,
    );
    const html = '<p>See artifacts/path/file_002500.md for details</p>';
    const result = linkifyPaths(html, paths, []);
    expect(result).toContain('data-path="artifacts/path/file_002500.md"');
    expect(result).toContain('>artifacts/path/file_002500.md</a>');
  });

  it('prefers longest match across batches (length-desc tiebreak)', () => {
    // With batched regexes, a short prefix and a longer path could land in different
    // batches. The combined match selection must still prefer the longer one.
    const shortPath = 'notes.md';
    // 999 filler paths to push the longer path into a later batch (batch size = 500)
    const filler = Array.from({ length: 999 }, (_, i) => `filler/file_${i}.md`);
    const longPath = 'projects/sample/notes.md';
    const html = '<p>See projects/sample/notes.md</p>';
    const result = linkifyPaths(html, [shortPath, ...filler, longPath], []);
    expect(result).toContain('data-path="projects/sample/notes.md"');
    expect(result).toContain('>projects/sample/notes.md</a>');
    // Must NOT have a nested link of the short path inside the long-path anchor
    expect(result).not.toMatch(/<a[^>]*><a/);
  });

  it('keeps each compiled regex small enough for WebKit ("regex too large" guard)', () => {
    // WebKit's YARR engine throws SyntaxError "regular expression too large" when the
    // source exceeds an internal limit. V8 has no such limit, so this test asserts the
    // structural property directly: no single RegExp constructed by linkifyPaths may have
    // a source approaching the WebKit limit.
    const MAX_SAFE_SOURCE = 100_000;
    const sources: number[] = [];
    const RealRegExp = globalThis.RegExp;
    const Spy: any = function (pattern: any, flags?: string) {
      if (typeof pattern === 'string') sources.push(pattern.length);
      return new RealRegExp(pattern, flags);
    };
    Spy.prototype = RealRegExp.prototype;
    (globalThis as any).RegExp = Spy;
    try {
      const paths = Array.from(
        { length: 10000 },
        (_, i) => `artifacts/path/segment-${i.toString(36)}/file.md`,
      );
      linkifyPaths('<p>hello world</p>', paths, []);
    } finally {
      (globalThis as any).RegExp = RealRegExp;
    }
    expect(sources.length).toBeGreaterThan(0);
    const maxSource = Math.max(...sources);
    expect(maxSource).toBeLessThan(MAX_SAFE_SOURCE);
  });
});

describe('linkifyPaths caching', () => {
  beforeEach(() => _resetLinkifyCacheForTesting());

  it('a cached call returns output identical to a fresh compute', () => {
    const html = '<p>See user_profile.md and visit https://example.com</p>';
    const paths = ['user_profile.md'];
    const apps: { name: string; id: string }[] = [];
    const first = linkifyPaths(html, paths, apps); // miss → computes + caches
    const second = linkifyPaths(html, paths, apps); // hit
    expect(second).toBe(first);
    // And it matches a from-scratch compute (cache cleared) — proves the cached
    // value isn't stale/wrong.
    _resetLinkifyCacheForTesting();
    expect(linkifyPaths(html, paths, apps)).toBe(first);
  });

  it('invalidates when the artifact/app list changes (new reference, new content)', () => {
    // A deliberate app anchor: with no apps the strict app rewriter declines
    // (the artifact rewriter then claims the path by shape, so it previews as a
    // file); once the app list contains it, the SAME html must rewrite to an
    // app-link, proving the (paths, apps) change invalidates the cache.
    // (Bare-text app-name scanning was removed, so this uses an anchor.)
    const html = '<p><a href="apps/habit-tracker/index.html">Habit Tracker</a></p>';
    const before = linkifyPaths(html, [], []);
    expect(before).not.toContain('data-app-id');
    expect(before).toContain('data-path="apps/habit-tracker/index.html"');
    const after = linkifyPaths(html, [], [{ name: 'Habit Tracker', id: 'habit-tracker' }]);
    expect(after).toContain('data-app-id="habit-tracker"');
    expect(after).not.toContain('artifact-link');
  });

  it('cache:false produces the same output as the cached path but is not stored', () => {
    const html = '<p>See user_profile.md</p>';
    const paths = ['user_profile.md'];
    const cached = linkifyPaths(html, paths, []);
    _resetLinkifyCacheForTesting();
    const uncached = linkifyPaths(html, paths, [], { cache: false });
    expect(uncached).toBe(cached);
  });
});
