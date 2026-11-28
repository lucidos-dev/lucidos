import assert from 'node:assert/strict';
import { test } from 'node:test';
import { compare } from './css_proof.mjs';

test('a comment-only change is equal', () => {
  const before = '/* a */\n.a { color: red; /* b */ margin: 0 /* c */ auto; }\n@media (min-width: 1px) /* d */ { .b { top: 0 } }\n';
  const after = '.a { color: red; margin: 0  auto; }\n@media (min-width: 1px) { .b { top: 0 } }\n';
  assert.equal(compare(before, after), null);
});

test('a changed value differs', () => {
  assert.match(compare('.a { color: red; }', '.a { color: blue; }'), /became/);
});

test('a lost rule differs', () => {
  assert.match(compare('.a { color: red; }\n.b { top: 0; }', '.a { color: red; }'), /became `nothing`/);
});

test('a lost space between two selectors differs', () => {
  assert.match(compare('.a /* c */ .b { top: 0; }', '.a.b { top: 0; }'), /became/);
});

test('an added space between two selectors differs', () => {
  assert.match(compare('.a/* c */.b { top: 0; }', '.a .b { top: 0; }'), /became/);
});

test('whitespace inside a string counts', () => {
  assert.match(compare(".a { content: 'a  b'; }", ".a { content: 'a b'; }"), /became/);
});

test('a comment marker inside a string is no comment', () => {
  const css = ".a { content: '/* x */'; }";
  assert.equal(compare(css, css), null);
  assert.match(compare(css, ".a { content: ''; }"), /became/);
});

test('a file that stops parsing differs', () => {
  assert.match(compare('.a { color: red; }', '.a { color: red; '), /does not parse/);
});
