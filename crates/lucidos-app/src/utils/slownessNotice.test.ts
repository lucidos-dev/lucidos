import { describe, expect, it } from 'vitest';
import type { MemoryUser, ProcessorUser } from '../api/client/control';
import {
  busyEnoughToName,
  formatGigabytes,
  memoryRecommendation,
  memoryUsersPhrase,
  processorRecommendation,
  processorUsersPhrase,
} from './slownessNotice';

const chrome: MemoryUser = { name: 'Google Chrome', bytes: 7e9, kind: 'app' };
const slack: MemoryUser = { name: 'Slack', bytes: 6e8, kind: 'app' };
const lucidos: MemoryUser = { name: 'Lucidos', bytes: 1.4e9, kind: 'lucidos' };
const vm: MemoryUser = { name: 'com.apple.Virtualization.VirtualMachine', bytes: 3.3e9, kind: 'process' };

describe('memoryRecommendation names one thing to do', () => {
  it('names the biggest app when it is not Lucidos', () => {
    expect(memoryRecommendation([chrome, lucidos, slack])).toBe(
      'Quit or restart Google Chrome to free memory.',
    );
  });

  it('points at coding agents when Lucidos is biggest, never at quitting Lucidos', () => {
    const text = memoryRecommendation([lucidos, chrome]);
    expect(text).toBe('Stop coding-agent threads you are not using.');
    expect(text).not.toMatch(/quit/i);
  });

  it('never names a bare process, which may be a system service', () => {
    expect(memoryRecommendation([vm, chrome])).toBe('Quit apps you are not using to free memory.');
  });

  it('falls back to general advice with nothing measured', () => {
    expect(memoryRecommendation([])).toBe('Quit apps you are not using to free memory.');
  });
});

describe('memoryUsersPhrase', () => {
  it('lists each group with decimal gigabytes', () => {
    expect(memoryUsersPhrase([chrome, slack, lucidos])).toBe(
      'Google Chrome 7.0 GB, Slack 0.6 GB, Lucidos 1.4 GB',
    );
  });

  it('formats one decimal place', () => {
    expect(formatGigabytes(865e6)).toBe('0.9 GB');
  });
});

const xcode: ProcessorUser = { name: 'Xcode', percent: 40, kind: 'app' };
const busyLucidos: ProcessorUser = { name: 'Lucidos', percent: 30, kind: 'lucidos' };
const rustc: ProcessorUser = { name: 'rustc', percent: 55, kind: 'process' };
const idleChrome: ProcessorUser = { name: 'Google Chrome', percent: 4, kind: 'app' };

describe('processorRecommendation names one thing to do', () => {
  it('names the busiest app when it is not Lucidos', () => {
    expect(processorRecommendation([xcode, busyLucidos])).toBe('Quit or restart Xcode.');
  });

  it('points at coding agents when Lucidos is busiest, never at quitting Lucidos', () => {
    const text = processorRecommendation([busyLucidos, xcode]);
    expect(text).toBe('Stop coding-agent threads you are not using.');
    expect(text).not.toMatch(/quit/i);
  });

  it('never names a bare process', () => {
    expect(processorRecommendation([rustc, xcode])).toBe('Quit apps you are not using.');
  });

  it('says so honestly when nothing stands out, or nothing was measured', () => {
    const nothing = 'Nothing on this computer stands out as busy. If it lasts, restart the computer.';
    expect(processorRecommendation([idleChrome])).toBe(nothing);
    expect(processorRecommendation([])).toBe(nothing);
  });
});

describe('busyEnoughToName', () => {
  it('keeps the whole list once the busiest app stands out', () => {
    expect(busyEnoughToName([xcode, idleChrome])).toEqual([xcode, idleChrome]);
  });

  it('names nobody when the busiest app is under ten percent', () => {
    expect(busyEnoughToName([idleChrome])).toEqual([]);
    expect(busyEnoughToName([])).toEqual([]);
  });
});

describe('processorUsersPhrase', () => {
  it('lists each group with its share of the computer', () => {
    expect(processorUsersPhrase([xcode, busyLucidos])).toBe('Xcode 40%, Lucidos 30%');
  });
});
