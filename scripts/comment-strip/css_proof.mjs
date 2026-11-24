#!/usr/bin/env node

import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import postcss from 'postcss';

const QUOTED = /("(?:[^"\\]|\\[^])*"|'(?:[^'\\]|\\[^])*')/;

function spaced(value) {
  return String(value ?? '')
    .split(QUOTED)
    .map((part, i) => (i % 2 ? part : part.replace(/\s+/g, ' ')))
    .join('')
    .trim();
}

export function outline(css) {
  const lines = [];
  const visit = (node, depth) => {
    if (node.type === 'comment') return;
    if (node.type === 'rule') lines.push(`${depth} rule ${spaced(node.selector)}`);
    if (node.type === 'atrule') lines.push(`${depth} at ${node.name} ${spaced(node.params)}`);
    if (node.type === 'decl') {
      lines.push(`${depth} decl ${node.prop}: ${spaced(node.value)}${node.important ? ' !important' : ''}`);
    }
    for (const child of node.nodes ?? []) visit(child, depth + 1);
  };
  visit(postcss.parse(css), 0);
  return lines;
}

export function compare(before, after) {
  let a;
  let b;
  try {
    a = outline(before);
  } catch (error) {
    return `the original does not parse: ${error.reason ?? error.message}`;
  }
  try {
    b = outline(after);
  } catch (error) {
    return `the stripped file does not parse: ${error.reason ?? error.message}`;
  }
  const n = Math.max(a.length, b.length);
  for (let i = 0; i < n; i++) {
    if (a[i] !== b[i]) return `node ${i}: \`${a[i] ?? 'nothing'}\` became \`${b[i] ?? 'nothing'}\``;
  }
  return null;
}

function main(argv) {
  const [command, origRoot, newRoot] = argv;
  if (command !== 'verify' || !origRoot || !newRoot) {
    process.stderr.write('usage: css_proof.mjs verify <orig-root> <new-root>, paths on stdin\n');
    return 2;
  }
  let failed = false;
  for (const rel of fs.readFileSync(0, 'utf8').split('\n').filter(Boolean)) {
    const read = (root) => fs.readFileSync(path.join(root, rel), 'utf8');
    const why = compare(read(origRoot), read(newRoot));
    if (why) {
      failed = true;
      process.stdout.write(`DIFF\t${rel}\t${why}\n`);
    }
  }
  return failed ? 1 : 0;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) process.exitCode = main(process.argv.slice(2));
