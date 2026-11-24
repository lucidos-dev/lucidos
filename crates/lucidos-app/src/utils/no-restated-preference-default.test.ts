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

const DISTINCTIVE_DEFAULTS: string[] = [
  ...new Set(
    Object.values(PREFERENCE_CATALOG as Record<string, CatalogEntry>)
      .filter((entry) => entry.type === 'text')
      .map((entry) => entry.fallback)
      .filter((value): value is string => value !== null && value.length >= 6 && /[\d,/-]/.test(value)),
  ),
];

function isListChoice(literal: ts.Node): boolean {
  const parent = literal.parent;
  if (ts.isArrayLiteralExpression(parent)) return true;
  return ts.isPropertyAssignment(parent)
    && parent.initializer === literal
    && ts.isObjectLiteralExpression(parent.parent)
    && ts.isArrayLiteralExpression(parent.parent.parent);
}

function scalarStringLiterals(ast: ts.SourceFile): string[] {
  const out: string[] = [];
  walk(ast, (node) => {
    if ((ts.isStringLiteral(node) || ts.isNoSubstitutionTemplateLiteral(node)) && !isListChoice(node)) {
      out.push(node.text);
    }
  });
  return out;
}

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
    expect(DISTINCTIVE_DEFAULTS).toContain(PREFERENCE_CATALOG.chat_model.fallback);
    expect(DISTINCTIVE_DEFAULTS).toContain(PREFERENCE_CATALOG.trigger_model.fallback);
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
