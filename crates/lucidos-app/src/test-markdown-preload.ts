
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

export function evaluatedFiles(): string[] | null {
  const worker = (globalThis as { __vitest_worker__?: VitestWorker }).__vitest_worker__;
  const files = worker?.evaluatedModules?.fileToModulesMap;
  return files instanceof Map ? [...files.keys()] : null;
}

export function reachesMarkdown(files: string[] | null, testPath: string): boolean {
  if (!files || !files.includes(testPath)) return true;
  return files.some(isMarkdownModule);
}

export function lateMarkdownMessage(testPath: string): string {
  return `${testPath} loaded a markdown module after its imports ran, through a dynamic import. `
    + 'The markdown engine is preloaded only for files that import one statically, '
    + 'so this file would render markdown as plain text. '
    + "Add a static `import '<path to>/utils/renderMarkdown';` to the test file.";
}
