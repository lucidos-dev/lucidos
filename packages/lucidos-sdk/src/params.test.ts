import { describe, expect, it } from 'vitest';
import { WIDGET_PARAMS_QUERY, parseWidgetParams } from './params';

function search(params: unknown): string {
  const q = new URLSearchParams({ thread_id: 't-1', device: 'd-1' });
  q.set(WIDGET_PARAMS_QUERY, JSON.stringify(params));
  return `?${q.toString()}`;
}

describe('parseWidgetParams', () => {
  it('reads the object back beside host-owned keys', () => {
    const value = { clip: 'artifacts/voices/marin.mp3', loop: false, n: 2, tags: ['a'] };
    expect(parseWidgetParams(search(value))).toEqual(value);
  });

  it('never returns a host key', () => {
    expect(parseWidgetParams('?thread_id=t-1&device=d-1')).toEqual({});
  });

  it('reads anything but a JSON object as empty', () => {
    expect(parseWidgetParams(search([1, 2]))).toEqual({});
    expect(parseWidgetParams(search(3))).toEqual({});
    expect(parseWidgetParams(search(null))).toEqual({});
    expect(parseWidgetParams(`?${WIDGET_PARAMS_QUERY}=%7Bnot-json`)).toEqual({});
  });
});
