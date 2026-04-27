// Tests that VDOM keys on PromptInput's conditional children prevent
// Preact from recycling prompt-box's DOM node when toggles mount/unmount.
import { describe, it, expect } from 'vitest';
import { h, VNode } from 'preact';

// eslint-disable-next-line @typescript-eslint/no-explicit-any
type AnyVNode = VNode<any>;

// Slots mirror PromptInput's JSX children: images, toggles, urlContext, promptBox, camera
type Slots = { images?: AnyVNode; toggles?: AnyVNode; promptBox?: AnyVNode };

function children(slots: Slots): (AnyVNode | null)[] {
  return [
    slots.images ?? null,
    slots.toggles ?? null,
    null, // url context
    slots.promptBox ?? null,
    null, // camera
  ];
}

function findMatch(
  oldChildren: (AnyVNode | null)[],
  used: Set<number>,
  newChild: AnyVNode | null,
): number {
  if (!newChild) return -1;
  for (let j = 0; j < oldChildren.length; j++) {
    if (used.has(j)) continue;
    const old = oldChildren[j];
    if (!old) continue;
    // Preact matches by key first, then type
    if (newChild.key != null || old.key != null) {
      if (newChild.key === old.key && newChild.type === old.type) return j;
    } else {
      if (newChild.type === old.type) return j;
    }
  }
  return -1;
}

function matchChildren(
  oldChildren: (AnyVNode | null)[],
  newChildren: (AnyVNode | null)[],
): Map<number, number> {
  const matches = new Map<number, number>();
  const used = new Set<number>();
  for (let i = 0; i < newChildren.length; i++) {
    const child = newChildren[i];
    if (!child) { matches.set(i, -1); continue; }
    const match = findMatch(oldChildren, used, child);
    matches.set(i, match);
    if (match >= 0) used.add(match);
  }
  return matches;
}

describe('PromptInput VDOM key stability', () => {
  it('without keys: prompt-box is recycled for toggles wrapper (BUG)', () => {
    const old = children({ promptBox: h('div', { class: 'prompt-box' }) });
    const next = children({
      toggles: h('div', { class: 'input-toggles-wrapper' }),
      promptBox: h('div', { class: 'prompt-box' }),
    });

    const matches = matchChildren(old, next);

    // Bug: toggles (index 1) steals prompt-box's DOM (index 3) — both unkeyed divs
    expect(matches.get(1)).toBe(3);
    expect(matches.get(3)).toBe(-1);
  });

  it('with keys: prompt-box is preserved across toggle mount (FIX)', () => {
    const old = children({ promptBox: h('div', { key: 'prompt-box' }) });
    const next = children({
      toggles: h('div', { key: 'toggles' }),
      promptBox: h('div', { key: 'prompt-box' }),
    });

    const matches = matchChildren(old, next);

    expect(matches.get(1)).toBe(-1); // toggles: new DOM
    expect(matches.get(3)).toBe(3);  // prompt-box: preserved
  });

  it('with keys: prompt-box preserved when toggles unmount (compose → thread)', () => {
    const old = children({
      toggles: h('div', { key: 'toggles' }),
      promptBox: h('div', { key: 'prompt-box' }),
    });
    const next = children({ promptBox: h('div', { key: 'prompt-box' }) });

    const matches = matchChildren(old, next);

    expect(matches.get(3)).toBe(3);
  });

  it('with keys: prompt-box preserved when images mount', () => {
    const old = children({
      toggles: h('div', { key: 'toggles' }),
      promptBox: h('div', { key: 'prompt-box' }),
    });
    const next = children({
      images: h('div', { key: 'images' }),
      toggles: h('div', { key: 'toggles' }),
      promptBox: h('div', { key: 'prompt-box' }),
    });

    const matches = matchChildren(old, next);

    expect(matches.get(0)).toBe(-1); // images: new
    expect(matches.get(1)).toBe(1);  // toggles: preserved
    expect(matches.get(3)).toBe(3);  // prompt-box: preserved
  });
});
