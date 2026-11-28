import assert from 'node:assert/strict';
import { test } from 'node:test';
import { compare, jsxTextValue, removeEmptiedJsx } from './ts_proof.mjs';

const same = (before, after, file = 'a.ts') => assert.equal(compare(file, before, after), null);
const differs = (before, after, pattern, file = 'a.ts') => assert.match(compare(file, before, after) ?? 'equal', pattern);

test('a comment-only change is equal', () => {
  same('// a\nconst x = 1; /* b */\n/** c */\nexport function f() {}\n', 'const x = 1;\nexport function f() {}\n');
});

test('a changed token differs', () => {
  differs('const x = 1;\n', 'const x = 2;\n', /became/);
});

test('a regex and a template holding comment markers are code', () => {
  const src = 'const re = /a\\/\\/b/;\nconst t = `// ${x /* c */} /* d */`;\n';
  same(src, 'const re = /a\\/\\/b/;\nconst t = `// ${x } /* d */`;\n');
  differs(src, 'const re = /a/;\nconst t = `// ${x } /* d */`;\n', /became/);
});

const asi = {
  return: ['function f() {\n  return /* a\n  b */ 1;\n}\n', 'function f() {\n  return 1;\n}\n'],
  throw: ['function f() {\n  throw /* a\n  b */ e;\n}\n', 'function f() {\n  throw e;\n}\n'],
  break: ['x: for (;;) {\n  break /* a\n  b */ x;\n}\n', 'x: for (;;) {\n  break x;\n}\n'],
  continue: ['x: for (;;) {\n  continue /* a\n  b */ x;\n}\n', 'x: for (;;) {\n  continue x;\n}\n'],
  yield: ['function* g() {\n  yield /* a\n  b */ 1;\n}\n', 'function* g() {\n  yield 1;\n}\n'],
  'postfix ++': ['a /* a\n b */ ++\nb;\n', 'a ++\nb;\n'],
  'postfix --': ['a /* a\n b */ --\nb;\n', 'a --\nb;\n'],
  'a line starting with (': ['const a = b /* a\n b */ (c);\n', 'const a = b (c);\n'],
  'a line starting with [': ['const a = b /* a\n b */ [c];\n', 'const a = b [c];\n'],
  'a line starting with a template': ['const a = b /* a\n b */ `c`;\n', 'const a = b `c`;\n'],
};

for (const [name, [before, joined]] of Object.entries(asi)) {
  test(`a lost line terminator differs: ${name}`, () => {
    differs(before, joined, /line terminator|another statement|became|tokens became/);
    same(before, before.replace(/ ?\/\*[^]*?\*\/ ?/, '\n'));
  });
}

test('an added line terminator differs', () => {
  differs('function f() {\n  return /* a */ 1;\n}\n', 'function f() {\n  return\n  1;\n}\n', /line terminator|another statement/);
});

test('jsx text follows the jsx whitespace rule', () => {
  assert.equal(jsxTextValue('\n   '), '');
  assert.equal(jsxTextValue('\n  a b\n  c\n'), 'a b c');
  assert.equal(jsxTextValue(' a '), ' a ');
  assert.equal(jsxTextValue('\n  a '), 'a ');
});

const strip = (src) => src.replace(/\{\/\*[^]*?\*\/\}/g, '{ }');
const jsx = (src) => removeEmptiedJsx('a.tsx', src, strip(src));

test('a wrapper on its own line goes with its line', () => {
  const src = 'const el = (\n  <div>\n    {/* note */}\n    <b />\n  </div>\n);\n';
  const out = jsx(src);
  assert.equal(out.text, 'const el = (\n  <div>\n    <b />\n  </div>\n);\n');
  assert.deepEqual([out.removed, out.left], [1, 0]);
  same(src, out.text, 'a.tsx');
});

test('a wrapper beside one text goes', () => {
  const src = 'const el = (\n  <p>\n    {/* note */}\n    hello\n  </p>\n);\n';
  const out = jsx(src);
  assert.equal(out.text, 'const el = (\n  <p>\n    hello\n  </p>\n);\n');
  same(src, out.text, 'a.tsx');
});

test('a wrapper between two texts stays as an empty expression', () => {
  const src = 'const el = (\n  <p>\n    hello\n    {/* note */}\n    world\n  </p>\n);\n';
  const out = jsx(src);
  assert.equal(out.text, strip(src));
  assert.deepEqual([out.removed, out.left], [0, 1]);
  differs(src, src.replace('    {/* note */}\n', ''), /became/, 'a.tsx');
});

test('a wrapper that ends a line of text stays, since the text would lose its space', () => {
  const src = 'const el = (\n  <p>\n    hello {/* note */}\n  </p>\n);\n';
  assert.equal(jsx(src).text, strip(src));
});

test('a wrapper after text that keeps its space goes', () => {
  const src = 'const el = <p>hello {/* note */}</p>;\n';
  const out = jsx(src);
  assert.equal(out.text, 'const el = <p>hello </p>;\n');
  same(src, out.text, 'a.tsx');
});

test('two wrappers are judged one after the other', () => {
  const src = 'const el = (\n  <p>\n    hello\n    {/* a */}\n    {/* b */}\n    world\n  </p>\n);\n';
  const out = jsx(src);
  assert.deepEqual([out.removed, out.left], [1, 1]);
  same(src, out.text, 'a.tsx');
});

test('an expression that was already empty is not the strip\'s', () => {
  const src = 'const el = (\n  <div>\n    {}\n    {/* note */}\n    <b />\n  </div>\n);\n';
  assert.equal(jsx(src).text, 'const el = (\n  <div>\n    {}\n    <b />\n  </div>\n);\n');
});

test('an empty block that is not jsx keeps its braces', () => {
  const src = 'function f() {/* nothing */}\nconst o = {/* none */};\ntry { f(); } catch {/* ignore */}\n';
  assert.equal(removeEmptiedJsx('a.tsx', src, strip(src)).text, strip(src));
  differs(src, 'function f() \nconst o = ;\ntry { f(); } catch \n', /does not parse|became/, 'a.tsx');
});

test('a lost directive differs, in any form the compiler reads', () => {
  differs('// @ts-expect-error\nf(1);\n', 'f(1);\n', /compiler reads/);
  differs('/* why\n * @ts-expect-error */\nf(1);\n', 'f(1);\n', /compiler reads/);
  differs('/* @jsxImportSource preact */\nconst el = <div />;\n', 'const el = <div />;\n', /compiler reads/, 'a.tsx');
  differs('/// <reference types="node" />\nf(1);\n', 'f(1);\n', /compiler reads/);
});

test('a directive a block comment kept off its line stays off', () => {
  const before = '// @ts-expect-error\n/* why */\nconst b: number = "";\n';
  differs(before, '// @ts-expect-error\nconst b: number = "";\n', /compiler reads/);
  same('// @ts-expect-error\n// why\n\nconst b: number = "";\n', '// @ts-expect-error\n\nconst b: number = "";\n');
  const inline = 'const a = 1; /* @ts-ignore */ const c = 2;\n';
  differs(`${inline}/* why */\nconst b: number = "";\n`, `${inline}const b: number = "";\n`, /compiler reads/);
  const odd = 'const s = "a\u2028b";\n// @ts-ignore\n';
  differs(`${odd}/* why */\n\nconst b: number = "";\n`, `${odd}\nconst b: number = "";\n`, /compiler reads/);
});

test('javascript parses in the compiler\'s JavaScript mode', () => {
  const src = '#!/usr/bin/env node\n/** @param {string} a */\nexport function f(a) {\n  return a; // gone\n}\n';
  same(src, '#!/usr/bin/env node\nexport function f(a) {\n  return a;\n}\n', 'a.mjs');
  same('// gone\nmodule.exports = { a: 1 };\n', 'module.exports = { a: 1 };\n', 'a.cjs');
  differs('const a = 1;\n', 'const a = 2;\n', /became/, 'a.js');
  differs('const a = b\n/* c */ (d);\n', 'const a = b (d);\n', /line terminator/, 'a.js');
  differs('const a: number = 1;\n', 'const a = 1;\n', /became|tokens/, 'a.js');
  same('const el = (\n  <div>\n    <b />\n  </div>\n);\n', 'const el = (\n  <div>\n    <b />\n  </div>\n);\n', 'a.jsx');
});

test('a jsx file gets the emptied-wrapper pass', () => {
  const src = 'const el = (\n  <div>\n    {/* gone */}\n    <b />\n  </div>\n);\n';
  const out = removeEmptiedJsx('a.jsx', src, strip(src));
  assert.equal(out.removed, 1);
  same(src, out.text, 'a.jsx');
});
