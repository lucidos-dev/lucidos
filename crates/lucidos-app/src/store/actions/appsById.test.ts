import { describe, it, expect, vi, beforeEach } from 'vitest';
import { ApiError } from '../../api/client/_core';
import type { App } from '../types';

const getAppApi = vi.fn();
vi.mock('../../api/client/widgets', () => ({
  getAppApi: (...args: unknown[]) => getAppApi(...args),
}));

const { appById, appsById } = await import('../appsById');
const { forgetAppById, readAppsById } = await import('./appsById');

const moodPicker: App = { id: 'mood-picker', name: 'Mood Picker', description: '', reveal: 'on-load', kind: 'widget', reusable: false };

describe('reading apps by id', () => {
  beforeEach(() => {
    getAppApi.mockReset();
    appsById.value = new Map();
  });

  it('records the app the engine returns, a widget included', async () => {
    getAppApi.mockResolvedValueOnce(moodPicker);
    await readAppsById(['mood-picker']);
    expect(appById('mood-picker')).toEqual({ status: 'loaded', data: moodPicker });
  });

  it('records only a 404 as gone', async () => {
    getAppApi.mockRejectedValueOnce(new ApiError(404, 'App not found: gone'));
    getAppApi.mockRejectedValueOnce(new ApiError(503, 'Engine restarting'));
    await readAppsById(['gone', 'flaky']);
    expect(appById('gone')).toEqual({ status: 'loaded', data: null });
    expect(appById('flaky')).toMatchObject({ status: 'failed', httpCode: 503 });
  });

  it('reads each id once', async () => {
    getAppApi.mockResolvedValue(moodPicker);
    await readAppsById(['mood-picker']);
    await readAppsById(['mood-picker']);
    expect(getAppApi).toHaveBeenCalledTimes(1);
  });

  it('drops an answer that arrives after the id was forgotten', async () => {
    let answer!: (app: App) => void;
    getAppApi.mockReturnValueOnce(new Promise<App>((resolve) => { answer = resolve; }));
    const read = readAppsById(['mood-picker']);
    forgetAppById('mood-picker');
    answer(moodPicker);
    await read;
    expect(appById('mood-picker')).toEqual({ status: 'not-loaded' });
  });
});
