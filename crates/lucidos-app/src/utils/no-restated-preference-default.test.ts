/**
 * Permanent guard for one definition per preference value (ADR 0368).
 *
 * A preference's default and its value list live in the engine's catalog. The
 * frontend reads the generated copy, `@lucidos/preference-catalog`. A hand copy
 * drifts once the engine's default moves. This scan fails on the two shapes
 * that reintroduce one:
 *
 *   1. A distinctive text default spelled out as a scalar string literal in
 *      the app or SDK source, tests included. The list comes from the
 *      catalog. A choice in a list is not a default: an array element, or a
 *      property value of an object that is one (`[{ value: 'x', label }]`).
 *      Comments are not literals, so prose may still name a default.
 *   2. A `currentPreference(...)` call followed by `??` or `||`. The accessor
 *      already resolves the catalog default, so a fallback after it is a
 *      second default.
 *
 * No allowlist. A legitimate second mention is removed at its source.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync, readdirSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve, relative } from 'node:path';
import ts from 'typescript';
import { PREFERENCE_CATALOG } from '@lucidos/preference-catalog';

const here = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = resolve(here, '../../../..');
const APP_SRC = resolve(here, '..');
const SDK_SRC = resolve(REPO_ROOT, 'packages/lucidos-sdk/src');
const GENERATED_CATALOG = resolve(SDK_SRC, 'generated/preference-catalog.ts');

function tsFiles(dir: string): string[] {
  const out: string[] = [];
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = resolve(dir, entry.name);
    if (entry.isDirectory()) out.push(...tsFiles(full));
    else if (/\.tsx?$/.test(entry.name) && full !== GENERATED_CATALOG) out.push(full);
  }
  return out;
}

/** Parsed, not pattern-matched: a regex cannot tell a literal from a quote in
 *  a comment, a regex literal or a template. */
const SOURCES: { file: string; ast: ts.SourceFile }[] = [APP_SRC, SDK_SRC]
  .flatMap(tsFiles)
  .map((file) => ({
    file: relative(REPO_ROOT, file),
    ast: ts.createSourceFile(file, readFileSync(file, 'utf8'), ts.ScriptTarget.Latest, true,
      file.endsWith('.tsx') ? ts.ScriptKind.TSX : ts.ScriptKind.TS),
  }));

function walk(node: ts.Node, visit: (node: ts.Node) => void): void {
  visit(node);
  node.forEachChild((child) => walk(child, visit));
}

interface CatalogEntry {
  readonly type: string;
  readonly fallback: string | null;
}

/** Text defaults distinctive enough that a literal equal to one is a copy:
 *  model ids, the region, the resident sections, the local URL. A short plain
 *  word such as `standard` is too common to scan for. */
const DISTINCTIVE_DEFAULTS: string[] = [
  ...new Set(
    Object.values(PREFERENCE_CATALOG as Record<string, CatalogEntry>)
      .filter((entry) => entry.type === 'text')
      .map((entry) => entry.fallback)
      .filter((value): value is string => value !== null && value.length >= 6 && /[\d,/-]/.test(value)),
  ),
];

/** Whether a literal is one choice in a list rather than a scalar: an array
 *  element, or a property value of an object that is an array element. */
function isListChoice(literal: ts.Node): boolean {
  const parent = literal.parent;
  if (ts.isArrayLiteralExpression(parent)) return true;
  return ts.isPropertyAssignment(parent)
    && parent.initializer === literal
    && ts.isObjectLiteralExpression(parent.parent)
    && ts.isArrayLiteralExpression(parent.parent.parent);
}

/** Every scalar quoted literal, backtick literal with no interpolation, and
 *  JSX attribute string, as its text. */
function scalarStringLiterals(ast: ts.SourceFile): string[] {
  const out: string[] = [];
  walk(ast, (node) => {
    if ((ts.isStringLiteral(node) || ts.isNoSubstitutionTemplateLiteral(node)) && !isListChoice(node)) {
      out.push(node.text);
    }
  });
  return out;
}

/** Whether a call's value goes straight into `??` or `||` as the left side. */
function feedsAFallback(call: ts.Node): boolean {
  let node = call;
  while (ts.isParenthesizedExpression(node.parent)) node = node.parent;
  const parent = node.parent;
  return ts.isBinaryExpression(parent)
    && parent.left === node
    && (parent.operatorToken.kind === ts.SyntaxKind.QuestionQuestionToken
      || parent.operatorToken.kind === ts.SyntaxKind.BarBarToken);
}

describe('no preference default is restated outside the catalog', () => {
  it('finds the distinctive defaults it scans for', () => {
    // Guards the filter itself: a catalog shape change that empties the list
    // would otherwise pass every file vacuously.
    expect(DISTINCTIVE_DEFAULTS).toContain(PREFERENCE_CATALOG.chat_model.fallback);
    expect(DISTINCTIVE_DEFAULTS).toContain(PREFERENCE_CATALOG.voice_resident_sections.fallback);
  });

  it('spells no distinctive text default as a scalar string literal', () => {
    const offenders: string[] = [];
    for (const { file, ast } of SOURCES) {
      for (const literal of scalarStringLiterals(ast)) {
        if (DISTINCTIVE_DEFAULTS.includes(literal)) offenders.push(`${file}: '${literal}'`);
      }
    }
    expect(
      offenders,
      'These files restate a preference default. Read it from PREFERENCE_CATALOG '
        + `(@lucidos/preference-catalog) instead:\n${offenders.join('\n')}`,
    ).toEqual([]);
  });

  it('never follows a currentPreference call with a second fallback', () => {
    const offenders: string[] = [];
    for (const { file, ast } of SOURCES) {
      walk(ast, (node) => {
        if (ts.isCallExpression(node)
          && ts.isIdentifier(node.expression)
          && node.expression.text === 'currentPreference'
          && feedsAFallback(node)) {
          offenders.push(`${file}:${ast.getLineAndCharacterOfPosition(node.getStart(ast)).line + 1}`);
        }
      });
    }
    expect(
      offenders,
      `currentPreference already resolves the catalog default; drop the fallback after it:\n${offenders.join('\n')}`,
    ).toEqual([]);
  });
});
