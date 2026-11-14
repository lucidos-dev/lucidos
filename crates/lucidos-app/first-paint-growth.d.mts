/** Types for `first-paint-growth.mjs`, so its tests need no suppression. */

export interface FirstPaintReport {
  entryBytes: number;
  setBytes: number;
  setGzipBytes: number;
  budgetBytes: number;
  chunks: { file: string; bytes: number }[];
}

export interface GrowthVerdict {
  kind: 'within' | 'shrink' | 'reasoned' | 'fail';
  deltaBytes: number;
  deltaGzipBytes: number;
  message: string;
}

export interface ReportedChange {
  to: string;
  subject: string;
  deltaBytes: number;
  deltaGzipBytes: number;
  reasons: string[];
}

export type Build = (args: { appDir: string; outDir: string; reportPath: string }) => void;
export type Install = (tree: string) => void;

export interface CommandResult {
  exitCode: number;
  text: string;
}

export const FIRST_PAINT_REPORT_FILE: string;
export const GROWTH_THRESHOLD_BYTES: number;
export const GROWTH_TRAILER: string;
export const ARCHIVE_PATHS: string[];

export function signedKb(bytes: number): string;
export function isFirstPaintInput(path: string): boolean;
export function parseReport(text: string): FirstPaintReport | null;
export function parseReasons(output: string): string[];
export function growthVerdict(base: FirstPaintReport, head: FirstPaintReport, reasons: string[]): GrowthVerdict;
export function changesFromReflog(output: string): { from: string; to: string }[];
export function formatReport(args: {
  since: string;
  latest: FirstPaintReport | null;
  changes: ReportedChange[];
  top?: number;
}): string;
export function resolveMergeBase(repoRoot: string, baseRef: string): string | null;
export function rulerConfigSource(appConfig: string, ruler?: string): string;
export function viteBuild(args: { appDir: string; outDir: string }): void;
export function prepareGenerated(args: { repoRoot: string; sha: string; tree: string }): void;
export function treeNpm(tree: string, args: string[], env?: Record<string, string | undefined>): string;
export function npmCi(tree: string): void;
export function measureWorktree(repoRoot: string, build?: Build): FirstPaintReport | null;
export function measureCommit(args: {
  repoRoot: string;
  sha: string;
  cacheDir: string;
  build?: Build;
  install?: Install;
}): { report: FirstPaintReport; error?: undefined } | { error: string; report?: undefined };
export function runCheck(args: {
  repoRoot: string;
  baseRef?: string;
  cacheDir?: string;
  build?: Build;
  install?: Install;
  measureHead?: () => FirstPaintReport | null;
}): CommandResult;
export function runReport(args: {
  repoRoot: string;
  ref?: string;
  hours?: number;
  top?: number;
  cacheDir?: string;
  build?: Build;
  install?: Install;
}): CommandResult;
