import { describe, it, expect } from 'vitest';
import type { MessageOrigin, ActorMode, EngineReason, ThreadEvent } from './thread-events';

describe('MessageOrigin type', () => {
  it('accepts engine variant with EngineReason discriminator', () => {
    const o: MessageOrigin = { kind: 'engine', reason: { kind: 'session_recovered' } };
    expect(o.kind).toBe('engine');
  });

  it('accepts engine scheduler variant with trigger metadata', () => {
    const reason: EngineReason = { kind: 'scheduler', trigger_id: 'abc', trigger_name: 'nightly' };
    const o: MessageOrigin = { kind: 'engine', reason };
    if (o.kind === 'engine' && o.reason.kind === 'scheduler') {
      expect(o.reason.trigger_id).toBe('abc');
      expect(o.reason.trigger_name).toBe('nightly');
    }
  });

  it('parent_thread carries optional mode field', () => {
    const o: MessageOrigin = {
      kind: 'parent_thread', thread_id: 'x', mode: 'engine',
    };
    if (o.kind === 'parent_thread') expect(o.mode).toBe('engine');
  });

  it('workspace carries optional mode field', () => {
    const o: MessageOrigin = {
      kind: 'workspace', workspace: 'personal', mode: 'agent',
    };
    if (o.kind === 'workspace') expect(o.mode).toBe('agent');
  });
});

describe('ThreadEvent — engine-stamped event variants', () => {
  it('SessionRecovered carries optional origin', () => {
    const e: ThreadEvent = {
      type: 'SessionRecovered',
      branch: 'claude-code/x',
      origin: { kind: 'engine', reason: { kind: 'session_recovered' } },
    };
    if (e.type === 'SessionRecovered') expect(e.origin?.kind).toBe('engine');
  });

  it('ChangeProposed carries optional origin', () => {
    const e: ThreadEvent = {
      type: 'ChangeProposed',
      change_id: 'x',
      origin: { kind: 'engine', reason: { kind: 'stale_session' } },
    };
    if (e.type === 'ChangeProposed') expect(e.origin?.kind).toBe('engine');
  });

  it('CodingAgentPromptSent carries optional origin', () => {
    const e: ThreadEvent = {
      type: 'CodingAgentPromptSent',
      text: '/harden',
      origin: { kind: 'engine', reason: { kind: 'harden_retrigger' } },
    };
    if (e.type === 'CodingAgentPromptSent') expect(e.origin?.kind).toBe('engine');
  });

  it('TriggerStarted carries optional origin', () => {
    const e: ThreadEvent = {
      type: 'TriggerStarted',
      trigger_id: 'abc',
      origin: { kind: 'engine', reason: { kind: 'scheduler', trigger_id: 'abc' } },
    };
    if (e.type === 'TriggerStarted') expect(e.origin?.kind).toBe('engine');
  });
});

describe('ActorMode type', () => {
  it('accepts the three mode strings', () => {
    const a: ActorMode = 'human';
    const b: ActorMode = 'agent';
    const c: ActorMode = 'engine';
    expect([a, b, c]).toEqual(['human', 'agent', 'engine']);
  });
});
