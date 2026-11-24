#!/usr/bin/env node

import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import ts from 'typescript';

const K = ts.SyntaxKind;

const SCRIPT_KINDS = [
  ['.tsx', ts.ScriptKind.TSX],
  ['.jsx', ts.ScriptKind.JSX],
  ['.js', ts.ScriptKind.JS],
  ['.mjs', ts.ScriptKind.JS],
  ['.cjs', ts.ScriptKind.JS],
];
const holdsJsx = (file) => file.endsWith('.tsx') || file.endsWith('.jsx');

export function parse(file, text) {
  const kind = SCRIPT_KINDS.find(([suffix]) => file.endsWith(suffix))?.[1] ?? ts.ScriptKind.TS;
  return ts.createSourceFile(file, text, ts.ScriptTarget.Latest, false, kind);
}

export function jsxTextValue(raw) {
  const lines = raw.split(/\r\n|\n|\r/);
  const kept = [];
  lines.forEach((line, i) => {
    let s = line.replace(/\t/g, ' ');
    if (i > 0) s = s.replace(/^ +/, '');
    if (i < lines.length - 1) s = s.replace(/ +$/, '');
    if (s) kept.push(s);
  });
  return kept.join(' ');
}

const isEmptyJsxExpression = (node) =>
  node.kind === K.JsxExpression && !node.expression && !node.dotDotDotToken;

export function signature(file, text) {
  const sf = parse(file, text);
  const tokens = [];
  const starts = [];
  const line = (pos) => sf.getLineAndCharacterOfPosition(pos).line;
  let prevEnd = -1;
  let inJsxChildren = false;
  const walk = (node, parent) => {
    if (node.kind >= K.FirstJSDocNode && node.kind <= K.LastJSDocNode) return;
    if (isEmptyJsxExpression(node)) {
      inJsxChildren = true;
      return;
    }
    if (node.kind === K.JsxText) {
      const value = jsxTextValue(text.slice(node.pos, node.end));
      if (value) tokens.push({ kind: node.kind, parent: parent.kind, text: value, nl: null });
      inJsxChildren = true;
      prevEnd = node.end;
      return;
    }
    const children = node.getChildren(sf);
    if (children.length > 0) {
      for (const child of children) walk(child, node.kind === K.SyntaxList ? parent : node);
      return;
    }
    const start = node.getStart(sf);
    if (start === node.end) return;
    const nl = inJsxChildren || prevEnd < 0 ? null : line(prevEnd) !== line(start);
    tokens.push({ kind: node.kind, parent: parent.kind, text: text.slice(start, node.end), nl });
    starts.push(start);
    prevEnd = node.end;
    inJsxChildren = false;
  };
  walk(sf, sf);
  return { tokens, parseErrors: sf.parseDiagnostics.length, directives: directives(sf, text, starts) };
}

function directives(sf, text, starts) {
  const lineStarts = sf.getLineStarts();
  const line = (pos) => sf.getLineAndCharacterOfPosition(pos).line;
  const lineText = (n) => text.slice(lineStarts[n], lineStarts[n + 1] ?? text.length).trim();
  const codeLines = new Set(starts.map(line));
  const errors = (sf.commentDirectives ?? []).map((d) => {
    let target = line(d.range.end) + 1;
    while (target < lineStarts.length && (lineText(target) === '' || lineText(target).startsWith('//'))) target += 1;
    return `${d.type} ${codeLines.has(target)}`;
  });
  const withoutRange = (key, value) => (key === 'pos' || key === 'end' || key === 'range' ? undefined : value);
  const pragmas = [...(sf.pragmas ?? new Map())].map(([name, args]) => `${name} ${JSON.stringify(args, withoutRange)}`);
  return JSON.stringify([errors, pragmas.sort()]);
}

export function compare(file, before, after) {
  const a = signature(file, before);
  const b = signature(file, after);
  if (a.parseErrors > 0) return 'the original does not parse';
  if (b.parseErrors > 0) return 'the stripped file does not parse';
  if (a.directives !== b.directives) return 'a comment the compiler reads changed';
  const n = Math.min(a.tokens.length, b.tokens.length);
  for (let i = 0; i < n; i++) {
    const x = a.tokens[i];
    const y = b.tokens[i];
    if (x.kind !== y.kind || x.text !== y.text) return `token ${i}: \`${x.text}\` became \`${y.text}\``;
    if (x.parent !== y.parent) return `token ${i}: \`${x.text}\` moved to another statement`;
    if (x.nl !== y.nl) return `token ${i}: the line terminator before \`${x.text}\` changed`;
  }
  if (a.tokens.length !== b.tokens.length) return `${a.tokens.length} tokens became ${b.tokens.length}`;
  return null;
}

function emptyJsxExpressions(sf) {
  const found = [];
  const visit = (node, parent) => {
    if (isEmptyJsxExpression(node)) found.push({ node, parent });
    ts.forEachChild(node, (child) => visit(child, node));
  };
  visit(sf, undefined);
  return found;
}

const isBlank = (s) => /^[ \t]*$/.test(s);

function withoutContainer(sf, text, { node, parent }) {
  if (!parent || (parent.kind !== K.JsxElement && parent.kind !== K.JsxFragment)) return null;
  const at = parent.children.indexOf(node);
  const textSibling = (s) => (s && s.kind === K.JsxText ? s : undefined);
  const before = textSibling(parent.children[at - 1]);
  const after = textSibling(parent.children[at + 1]);
  let start = node.getStart(sf);
  let end = node.end;
  const lineStart = text.lastIndexOf('\n', start - 1) + 1;
  const nextBreak = text.indexOf('\n', end);
  const lineEnd = nextBreak === -1 ? text.length : nextBreak;
  if (isBlank(text.slice(lineStart, start)) && isBlank(text.slice(end, lineEnd)) && nextBreak !== -1) {
    const [lo, hi] = [before ? before.pos : start, after ? after.end : end];
    if (lineStart >= lo && lineEnd + 1 <= hi) {
      start = lineStart;
      end = lineEnd + 1;
    }
  }
  const was = [before, after].map((s) => (s ? jsxTextValue(text.slice(s.pos, s.end)) : '')).filter(Boolean);
  const merged = (before ? text.slice(before.pos, start) : '') + (after ? text.slice(end, after.end) : '');
  const now = [jsxTextValue(merged)].filter(Boolean);
  if (JSON.stringify(was) !== JSON.stringify(now)) return null;
  return text.slice(0, start) + text.slice(end);
}

export function removeEmptiedJsx(file, original, stripped) {
  const inner = (text, { node }, sf) => text.slice(node.getStart(sf) + 1, node.end - 1);
  const origSf = parse(file, original);
  const emptied = emptyJsxExpressions(origSf).map((e) => inner(original, e, origSf).trim() !== '');
  let text = stripped;
  let removed = 0;
  for (let i = 0; i < emptied.length; ) {
    const sf = parse(file, text);
    const found = emptyJsxExpressions(sf);
    if (found.length !== emptied.length) throw new Error(`${file}: the empty JSX expressions do not line up`);
    const next = emptied[i] && inner(text, found[i], sf).trim() === '' ? withoutContainer(sf, text, found[i]) : null;
    if (next === null) {
      i += 1;
    } else {
      text = next;
      emptied.splice(i, 1);
      removed += 1;
    }
  }
  const left = emptied.filter(Boolean).length;
  return { text, removed, left };
}

function main(argv) {
  const [command, origRoot, newRoot] = argv;
  if (!['verify', 'jsx'].includes(command) || !origRoot || !newRoot) {
    process.stderr.write('usage: ts_proof.mjs verify|jsx <orig-root> <new-root>, paths on stdin\n');
    return 2;
  }
  const files = fs.readFileSync(0, 'utf8').split('\n').filter(Boolean);
  let failed = false;
  const totals = { removed: 0, left: 0 };
  for (const rel of files) {
    const original = fs.readFileSync(path.join(origRoot, rel), 'utf8');
    const target = path.join(newRoot, rel);
    const stripped = fs.readFileSync(target, 'utf8');
    try {
      if (command === 'verify') {
        const why = compare(rel, original, stripped);
        if (why) {
          failed = true;
          process.stdout.write(`DIFF\t${rel}\t${why}\n`);
        }
      } else if (holdsJsx(rel)) {
        const result = removeEmptiedJsx(rel, original, stripped);
        totals.removed += result.removed;
        totals.left += result.left;
        if (result.text !== stripped) fs.writeFileSync(target, result.text);
      }
    } catch (error) {
      failed = true;
      process.stdout.write(`DIFF\t${rel}\t${error.message}\n`);
    }
  }
  if (command === 'jsx') process.stdout.write(`JSX\t${totals.removed}\t${totals.left}\n`);
  return failed ? 1 : 0;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) process.exitCode = main(process.argv.slice(2));
