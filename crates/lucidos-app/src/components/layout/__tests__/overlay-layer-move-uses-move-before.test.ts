// @vitest-environment jsdom
import { describe, it, expect, vi } from 'vitest';
import { moveOverlayLayer } from '../OverlayLayer';

describe('moving the overlay layer uses moveBefore where the browser has it', () => {
  it('moves the node with moveBefore, which keeps focus, caret and iframe state in place', () => {
    const parent = document.createElement('div');
    const node = document.createElement('div');
    document.body.append(parent, node);
    const moveBefore = vi.fn();
    Object.assign(parent, { moveBefore });
    moveOverlayLayer(node, parent);
    expect(moveBefore).toHaveBeenCalledWith(node, null);
  });

  it('restores the focused field and its caret when appendChild drops them', () => {
    const parent = document.createElement('div');
    const node = document.createElement('div');
    const field = document.createElement('textarea');
    field.value = 'a draft in progress';
    node.append(field);
    document.body.append(node, parent);
    field.focus();
    field.setSelectionRange(2, 7);
    const append = parent.appendChild.bind(parent);
    vi.spyOn(parent, 'appendChild').mockImplementation((child) => {
      const moved = append(child);
      field.blur();
      field.setSelectionRange(0, 0);
      return moved;
    });
    moveOverlayLayer(node, parent);
    expect(parent.lastChild).toBe(node);
    expect(document.activeElement).toBe(field);
    expect([field.selectionStart, field.selectionEnd]).toEqual([2, 7]);
  });
});
