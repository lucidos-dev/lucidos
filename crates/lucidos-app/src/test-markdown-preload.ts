/**
 * Decides whether a test file gets the markdown engine loaded up front.
 * `test-setup.ts` calls it; the engine pulls in over a hundred modules, so a
 * file that never renders markdown skips the load.
 *
 * Every answer here must fail towards loading. The markdown facade renders
 * escaped plain text without an engine, so a wrong "no" passes tests silently.
 */

/** The modules through which a test file can render markdown or highlight code. */
export const MARKDOWN_MODULES = [
  'utils/renderMarkdown.ts',
  'utils/markdownEngine.ts',
  'utils/markdownLibraries.ts',
  'utils/markedConfig.ts',
  'utils/syntaxHighlight.ts',
  'utils/highlightLibrary.ts',
] as const;

const isMarkdownModule = (file: string): boolean =>
  MARKDOWN_MODULES.some(module => file.endsWith(`/src/${module}`));

interface VitestWorker {
  evaluatedModules?: { fileToModulesMap?: unknown };
}

/** The files this worker has evaluated, or null when Vitest does not say. */
export function evaluatedFiles(): string[] | null {
  const worker = (globalThis as { __vitest_worker__?: VitestWorker }).__vitest_worker__;
  const files = worker?.evaluatedModules?.fileToModulesMap;
  return files instanceof Map ? [...files.keys()] : null;
}

/**
 * True when the evaluated files hold a markdown module. Also true when the
 * list is missing, or omits the test file itself: then the list cannot be
 * trusted to answer.
 */
export function reachesMarkdown(files: string[] | null, testPath: string): boolean {
  if (!files || !files.includes(testPath)) return true;
  return files.some(isMarkdownModule);
}

/** What a test file sees when it reaches markdown after the preload decision. */
export function lateMarkdownMessage(testPath: string): string {
  return `${testPath} loaded a markdown module after its imports ran, through a dynamic import. `
    + 'The markdown engine is preloaded only for files that import one statically, '
    + 'so this file would render markdown as plain text. '
    + "Add a static `import '<path to>/utils/renderMarkdown';` to the test file.";
}
