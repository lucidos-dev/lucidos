// @vitest-environment jsdom
// The header draws the waiting clock beside Help, so every ringed glyph in it
// takes one ring. A clock drawn on a smaller ring with a mitred hand read as
// uneven next to the question mark.
import { describe, it, expect } from 'vitest';
import { render } from 'preact';
import type { JSX } from 'preact';
import { EventWaitClockIcon, HelpIcon, InfoIcon } from '../icons';

function ringOf(icon: JSX.Element) {
  const host = document.createElement('div');
  render(icon, host);
  const svg = host.querySelector('svg')!;
  const ring = host.querySelector('circle')!;
  const style = (name: string) => ring.getAttribute(name) ?? svg.getAttribute(name);
  return {
    viewBox: svg.getAttribute('viewBox'),
    r: ring.getAttribute('r'),
    strokeWidth: style('stroke-width'),
    linecap: style('stroke-linecap'),
    linejoin: style('stroke-linejoin'),
  };
}

describe('ringed icons', () => {
  it('draw the waiting clock on the same ring and joins as Help and Info', () => {
    const help = ringOf(<HelpIcon />);
    expect(ringOf(<InfoIcon />)).toEqual(help);
    expect(ringOf(<EventWaitClockIcon />)).toEqual(help);
  });
});
