// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { existsSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { MARKDOWN_MODULES, evaluatedFiles, lateMarkdownMessage, reachesMarkdown } from './test-markdown-preload';

const SRC = dirname(fileURLToPath(import.meta.url));
const TEST_FILE = '/repo/crates/lucidos-app/src/store/some.test.ts';

describe('reachesMarkdown', () => {
  it('skips a file whose graph holds no markdown module', () => {
    expect(reachesMarkdown([TEST_FILE, '/repo/crates/lucidos-app/src/utils/escapeHtml.ts'], TEST_FILE)).toBe(false);
  });

  it.each(MARKDOWN_MODULES)('loads for a graph that holds %s', module => {
    expect(reachesMarkdown([TEST_FILE, `/repo/crates/lucidos-app/src/${module}`], TEST_FILE)).toBe(true);
  });

  it('loads when Vitest does not list the evaluated files', () => {
    expect(reachesMarkdown(null, TEST_FILE)).toBe(true);
  });

  it('loads when the list omits the test file, so it cannot be trusted', () => {
    expect(reachesMarkdown(['/repo/crates/lucidos-app/src/utils/escapeHtml.ts'], TEST_FILE)).toBe(true);
  });
});

describe('the markdown modules it names', () => {
  it.each(MARKDOWN_MODULES)('%s exists', module => {
    expect(existsSync(resolve(SRC, module))).toBe(true);
  });
});

// A Vitest upgrade that moves the registry turns the skip off for every file.
// The suite stays correct and gets slow, and only these two cases say so.
describe('evaluatedFiles reads the Vitest worker registry', () => {
  it('lists the running test file', () => {
    expect(evaluatedFiles()).toContain(expect.getState().testPath);
  });

  it('gains a module that a test imports dynamically', async () => {
    const leaf = resolve(SRC, 'utils/escapeHtmlAttr.ts');
    await import('./utils/escapeHtmlAttr');
    expect(evaluatedFiles()).toContain(leaf);
  });

  it('answers no for this file, which imports no markdown module', () => {
    expect(reachesMarkdown(evaluatedFiles(), expect.getState().testPath ?? '')).toBe(false);
  });
});

describe('lateMarkdownMessage', () => {
  it('names the file and the import that fixes it', () => {
    const message = lateMarkdownMessage(TEST_FILE);
    expect(message).toContain(TEST_FILE);
    expect(message).toContain('utils/renderMarkdown');
  });
});
