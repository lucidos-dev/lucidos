import { afterEach, describe, expect, it } from 'vitest';
import { readerDevices, readRequestedForMe } from '../store';
import { readIsMine } from '../actions/reader-devices';

/** On a team gateway one user's sighting leaves a read request for the
 *  others (ADR 0623). A page reading as the team sees every request. */
describe('a read request is each user’s own', () => {
  afterEach(() => {
    readerDevices.value = { status: 'not-loaded' };
  });

  const asSam = () => {
    readerDevices.value = { status: 'loaded', data: { devices: ['sam-phone', 'sam-laptop'] } };
  };

  it('stays for a user none of whose devices saw it', () => {
    asSam();
    expect(readRequestedForMe({ readRequested: true, replySeenDevices: ['kim-phone'] })).toBe(true);
  });

  it('leaves a user whose other device saw it', () => {
    asSam();
    expect(readRequestedForMe({ readRequested: true, replySeenDevices: ['sam-laptop'] })).toBe(false);
  });

  it('counts every pending request for the team, and before the page knows its user', () => {
    expect(readRequestedForMe({ readRequested: true, replySeenDevices: ['sam-laptop'] })).toBe(true);
    readerDevices.value = { status: 'loaded', data: { devices: null } };
    expect(readRequestedForMe({ readRequested: true, replySeenDevices: ['sam-laptop'] })).toBe(true);
  });

  it('never invents a request', () => {
    asSam();
    expect(readRequestedForMe({ readRequested: false, replySeenDevices: [] })).toBe(false);
  });
});

describe('a notification read is each user’s own', () => {
  afterEach(() => {
    readerDevices.value = { status: 'not-loaded' };
  });

  it('takes a read from one of the user’s devices, and no one else’s', () => {
    readerDevices.value = { status: 'loaded', data: { devices: ['sam-phone', 'sam-laptop'] } };
    expect(readIsMine({ kind: 'device', device_id: 'sam-laptop' })).toBe(true);
    expect(readIsMine({ kind: 'device', device_id: 'kim-phone' })).toBe(false);
    expect(readIsMine(undefined)).toBe(false);
  });

  it('takes every read as its own when the page reads as the team', () => {
    expect(readIsMine({ kind: 'device', device_id: 'kim-phone' })).toBe(true);
    readerDevices.value = { status: 'loaded', data: { devices: null } };
    expect(readIsMine(undefined)).toBe(true);
  });
});
