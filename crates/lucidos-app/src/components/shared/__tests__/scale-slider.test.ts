import { describe, it, expect } from 'vitest';
import { UI_SCALE_MIN, UI_SCALE_MAX, UI_SCALE_STEP } from '@lucidos/appearance';
import { fractionForScale, scaleAfterKey, scaleAtPointer } from '../scaleSlider';

const TRACK = { left: 24, width: 224, thumbWidth: 20 };
const USABLE = TRACK.width - TRACK.thumbWidth;
const TRACK_START = TRACK.left + TRACK.thumbWidth / 2;

const STOPS: number[] = [];
for (let v = UI_SCALE_MIN; v <= UI_SCALE_MAX; v += UI_SCALE_STEP) STOPS.push(v);

describe('scale slider geometry', () => {
  it('puts the ends of the travel at the ends of the range', () => {
    expect(fractionForScale(UI_SCALE_MIN)).toBe(0);
    expect(fractionForScale(UI_SCALE_MAX)).toBe(1);
    expect(fractionForScale(137.5)).toBe(0.5);
  });

  it('reads back the value the thumb is drawn at, for every stop', () => {
    for (const stop of STOPS) {
      const drawnAt = TRACK_START + fractionForScale(stop) * USABLE;
      expect(scaleAtPointer(drawnAt, TRACK)).toBe(stop);
    }
  });

  it('snaps a press between two stops to the nearer one', () => {
    expect(scaleAtPointer(TRACK_START + 0.25 * USABLE, TRACK)).toBe(112.5);
    expect(scaleAtPointer(TRACK_START + 0.22 * USABLE, TRACK)).toBe(100);
  });

  it('clamps a press on the dead track at either end', () => {
    expect(scaleAtPointer(TRACK.left, TRACK)).toBe(UI_SCALE_MIN);
    expect(scaleAtPointer(TRACK.left + TRACK.width, TRACK)).toBe(UI_SCALE_MAX);
  });

  it('clamps a drag that carries the finger off the row entirely', () => {
    expect(scaleAtPointer(TRACK.left - 400, TRACK)).toBe(UI_SCALE_MIN);
    expect(scaleAtPointer(TRACK.left + TRACK.width + 400, TRACK)).toBe(UI_SCALE_MAX);
  });

  it('has no answer for a row that has not been laid out', () => {
    expect(scaleAtPointer(100, { left: 0, width: 0, thumbWidth: 20 })).toBeNull();
  });
});

describe('scale slider keys', () => {
  it('steps by one stop in each direction', () => {
    expect(scaleAfterKey('ArrowRight', 100)).toBe(112.5);
    expect(scaleAfterKey('ArrowUp', 100)).toBe(112.5);
    expect(scaleAfterKey('ArrowLeft', 100)).toBe(87.5);
    expect(scaleAfterKey('ArrowDown', 100)).toBe(87.5);
  });

  it('jumps to either end', () => {
    expect(scaleAfterKey('Home', 150)).toBe(UI_SCALE_MIN);
    expect(scaleAfterKey('End', 150)).toBe(UI_SCALE_MAX);
  });

  it('holds at the clamp rather than walking past it', () => {
    expect(scaleAfterKey('ArrowRight', UI_SCALE_MAX)).toBe(UI_SCALE_MAX);
    expect(scaleAfterKey('ArrowLeft', UI_SCALE_MIN)).toBe(UI_SCALE_MIN);
  });

  it('leaves a key it does not own alone', () => {
    expect(scaleAfterKey('Escape', 100)).toBeNull();
    expect(scaleAfterKey('Tab', 100)).toBeNull();
    expect(scaleAfterKey('a', 100)).toBeNull();
  });
});
