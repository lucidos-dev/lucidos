import { describe, it, expect } from 'vitest';
import { actorInitiator, originMode } from './thread-events';
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

  it('thread_link carries optional mode field', () => {
    const o: MessageOrigin = {
      kind: 'thread_link', thread_id: 'x', mode: 'engine',
    };
    if (o.kind === 'thread_link') expect(o.mode).toBe('engine');
  });

  it('workspace carries optional mode field', () => {
    const o: MessageOrigin = {
      kind: 'workspace', workspace: 'personal', mode: 'agent',
    };
    if (o.kind === 'workspace') expect(o.mode).toBe('agent');
  });
});

describe('ThreadEvent — engine-stamped event variants', () => {
  it('ContinuationStarted carries optional origin', () => {
    const e: ThreadEvent = {
      type: 'ContinuationStarted',
      branch: 'claude-code/x',
      origin: { kind: 'engine', reason: { kind: 'continuation_started' } },
    };
    if (e.type === 'ContinuationStarted') expect(e.origin?.kind).toBe('engine');
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

describe('originMode', () => {
  it('device → human', () => {
    expect(originMode({ kind: 'device', device_id: 'd', label: 'L' })).toBe('human');
  });
  it('api defaults to human', () => {
    expect(originMode({ kind: 'api' })).toBe('human');
  });
  it('api respects explicit mode', () => {
    expect(originMode({ kind: 'api', mode: 'agent' })).toBe('agent');
    expect(originMode({ kind: 'api', mode: 'engine' })).toBe('engine');
  });
  it('workspace defaults to human', () => {
    expect(originMode({ kind: 'workspace', workspace: 'p' })).toBe('human');
  });
  it('workspace respects explicit mode', () => {
    expect(originMode({ kind: 'workspace', workspace: 'p', mode: 'agent' })).toBe('agent');
    expect(originMode({ kind: 'workspace', workspace: 'p', mode: 'engine' })).toBe('engine');
  });
  it('parent_thread defaults to agent', () => {
    expect(originMode({ kind: 'thread_link', thread_id: 't' })).toBe('agent');
  });
  it('parent_thread respects explicit mode', () => {
    expect(originMode({ kind: 'thread_link', thread_id: 't', mode: 'engine' })).toBe('engine');
  });
  it('engine → engine', () => {
    expect(originMode({ kind: 'engine', reason: { kind: 'session_recovered' } })).toBe('engine');
  });
  it('undefined → engine', () => {
    expect(originMode(undefined)).toBe('engine');
  });
});

describe('actorInitiator (closed set: You / Lucidos Agent / Lucidos Engine / System / API caller)', () => {
  it('device → You (the only origin that is unambiguously the user)', () => {
    expect(actorInitiator({ kind: 'device', device_id: 'd', label: 'L' }))
      .toEqual({ icon: '\u{1F464}', label: 'You' });
  });
  it('api with default mode → API caller (anonymous HTTP, never impersonates the user)', () => {
    // Regression: a Lucidos agent that POSTed via raw urllib without forwarding
    // x-lucidos-agent-origin-token used to land as Api{Human} and the chip
    // rendered "You". The chip now refuses to call any non-device origin
    // "You"; the popover still discloses the User-Agent.
    expect(actorInitiator({ kind: 'api' })).toEqual({ icon: '🔌', label: 'API caller' });
  });
  it('api with explicit mode=human → API caller', () => {
    expect(actorInitiator({ kind: 'api', mode: 'human', user_agent: 'curl/8' }))
      .toEqual({ icon: '🔌', label: 'API caller' });
  });
  it('api with mode=agent → Lucidos Agent', () => {
    expect(actorInitiator({ kind: 'api', mode: 'agent' }))
      .toEqual({ icon: '✨', label: 'Lucidos Agent' });
  });
  it('api with mode=engine → Lucidos Engine', () => {
    expect(actorInitiator({ kind: 'api', mode: 'engine' }))
      .toEqual({ icon: '⚙', label: 'Lucidos Engine' });
  });
  it('workspace with mode=human → API caller (a human in another workspace is not "You" here)', () => {
    expect(actorInitiator({ kind: 'workspace', workspace: 'p', mode: 'human' }))
      .toEqual({ icon: '🔌', label: 'API caller' });
  });
  it('workspace with mode=agent → Lucidos Agent', () => {
    expect(actorInitiator({ kind: 'workspace', workspace: 'p', mode: 'agent' }))
      .toEqual({ icon: '✨', label: 'Lucidos Agent' });
  });
  it('workspace with mode=engine → Lucidos Engine', () => {
    expect(actorInitiator({ kind: 'workspace', workspace: 'p', mode: 'engine' }))
      .toEqual({ icon: '⚙', label: 'Lucidos Engine' });
  });
  it('parent_thread (default mode=agent) → Lucidos Agent', () => {
    expect(actorInitiator({ kind: 'thread_link', thread_id: 't' }))
      .toEqual({ icon: '✨', label: 'Lucidos Agent' });
  });
  it('parent_thread with mode=engine → Lucidos Engine', () => {
    expect(actorInitiator({ kind: 'thread_link', thread_id: 't', mode: 'engine' }))
      .toEqual({ icon: '⚙', label: 'Lucidos Engine' });
  });
  it('engine origin → Lucidos Engine', () => {
    expect(actorInitiator({ kind: 'engine', reason: { kind: 'session_recovered' } }))
      .toEqual({ icon: '⚙', label: 'Lucidos Engine' });
  });
  it('system origin → System (distinct from engine — process killed by host, not engine-deliberate)', () => {
    expect(actorInitiator({ kind: 'system' })).toEqual({ icon: '⚙', label: 'System' });
  });
  it('undefined origin → Lucidos Engine', () => {
    expect(actorInitiator(undefined)).toEqual({ icon: '⚙', label: 'Lucidos Engine' });
  });
});
