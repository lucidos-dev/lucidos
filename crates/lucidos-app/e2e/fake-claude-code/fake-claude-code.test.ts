import { execFileSync, spawn, spawnSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync, symlinkSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createInterface } from 'node:readline';
import { afterEach, describe, expect, it } from 'vitest';
import {
  AUTH_STATUS, FAKE_VERSION, NO_EFFORT, NO_RULE_REPLY, NO_SESSION_CONTEXT_REPLY,
  failToStartText, replyTo, resolveModel, turnDurationMs,
} from './claude.mjs';
import {
  SCENARIO_FLAGS, SESSION_FLAGS, UNREPLAYED_FLAGS, diffShapes, shapeOf, unknownKinds,
} from './drift-check.mjs';

const FAKE = join(__dirname, 'claude.mjs');

type Line = Record<string, any>;

// No effort either: a suite run from inside Claude Code inherits its own.
const { CLAUDE_CODE_EFFORT_LEVEL: _, ...ENV } = process.env;
const NO_DELAY = { ...ENV, FAKE_CLAUDE_CODE_TURN_MS: '0' };

function runFake(args: string[], inputs: Line[] = []): Line[] {
  const input = inputs.map((l) => `${JSON.stringify(l)}\n`).join('');
  const out = execFileSync(process.execPath, [FAKE, ...args], { input, encoding: 'utf8', env: NO_DELAY });
  return out.trim().split('\n').filter(Boolean).map((l) => JSON.parse(l));
}

const user = (text: string): Line => ({
  type: 'user', message: { role: 'user', content: text }, session_id: 'default', parent_tool_use_id: null, uuid: 'u-1',
});
const control = (subtype: string, extra: Line = {}): Line => ({
  type: 'control_request', request_id: `r-${subtype}`, request: { subtype, ...extra },
});

describe('the fake Claude Code', () => {
  let dir: string | undefined;
  afterEach(() => { if (dir) rmSync(dir, { recursive: true, force: true }); dir = undefined; });

  it('prints its version, also when started through a symlink as on the runner', () => {
    expect(execFileSync(process.execPath, [FAKE, '--version'], { encoding: 'utf8' })).toBe(`${FAKE_VERSION} (Claude Code)\n`);
    dir = mkdtempSync(join(tmpdir(), 'fake-cc-'));
    symlinkSync(FAKE, join(dir, 'claude'));
    expect(execFileSync(join(dir, 'claude'), ['--version'], { encoding: 'utf8' })).toBe(`${FAKE_VERSION} (Claude Code)\n`);
  });

  it('answers `auth status` as a first-party login, so the engine shows its notes', () => {
    expect(runFake(['auth', 'status'])).toEqual([AUTH_STATUS]);
    expect(AUTH_STATUS.apiProvider).toBe('firstParty');
  });

  it('answers initialize with the model list and sends no init', () => {
    const lines = runFake([], [control('initialize')]);
    expect(lines).toHaveLength(1);
    expect(lines[0].response.request_id).toBe('r-initialize');
    expect(lines[0].response.response.models.map((m: Line) => m.value)).toContain('haiku');
  });

  it('runs a Say exactly turn: init, replay, the text, then result', () => {
    const lines = runFake(['--model', 'haiku'], [user('Say exactly: "hello X" and nothing else.')]);
    const init = lines[0];
    expect(init).toMatchObject({ type: 'system', subtype: 'init', model: resolveModel('haiku'), claude_code_version: FAKE_VERSION });
    expect(lines[1]).toMatchObject({ type: 'user', isReplay: true, uuid: 'u-1', session_id: init.session_id });
    expect(lines.find((l) => l.type === 'assistant')?.message.content).toEqual([{ type: 'text', text: 'hello X' }]);
    expect(lines.at(-1)).toMatchObject({ type: 'result', subtype: 'success', is_error: false, result: 'hello X' });
  });

  it('keeps the session id it is resumed with, and sends init once', () => {
    const lines = runFake(['--resume', 'sid-1'], [user('Say exactly: "a"'), user('Say exactly: "b"')]);
    expect(lines.filter((l) => l.subtype === 'init')).toHaveLength(1);
    expect(lines.filter((l) => l.type === 'result').map((l) => [l.session_id, l.result])).toEqual([['sid-1', 'a'], ['sid-1', 'b']]);
  });

  it('replays at once but lasts a turn before its result, so a mid-turn poll sees it live', async () => {
    const child = spawn(process.execPath, [FAKE], { env: { ...process.env, FAKE_CLAUDE_CODE_TURN_MS: '400' } });
    const seen: Record<string, number> = {};
    const done = new Promise<void>((resolve) => {
      createInterface({ input: child.stdout }).on('line', (raw) => {
        const line = JSON.parse(raw);
        const kind = line.isReplay ? 'replay' : line.type;
        seen[kind] ??= Date.now();
        if (line.type === 'result') resolve();
      });
    });
    child.stdin.write(`${JSON.stringify(user('Say exactly: "slow"'))}\n`);
    child.stdin.write(`${JSON.stringify(control('set_model', { model: 'haiku' }))}\n`);
    await done;
    child.kill();
    // Relative to the replay, so a slow Node start under load cannot fail it.
    expect(seen.result - seen.replay).toBeGreaterThanOrEqual(350);
    expect(seen.result - seen.control_response).toBeGreaterThanOrEqual(300);
  });

  it('reports the effort it was spawned with', () => {
    const lines = execFileSync(process.execPath, [FAKE], {
      input: `${JSON.stringify(user('Report your reasoning effort.'))}\n`,
      encoding: 'utf8',
      env: { ...NO_DELAY, CLAUDE_CODE_EFFORT_LEVEL: 'high' },
    }).trim().split('\n').map((l) => JSON.parse(l));
    expect(lines.at(-1)?.result).toBe('effort: high');
    expect(runFake([], [user('Report your reasoning effort.')]).at(-1)?.result).toBe(`effort: ${NO_EFFORT}`);
  });

  it('lasts as long as a turn asks', () => {
    expect(turnDurationMs('Take 20 seconds, then say hi.', 3000)).toBe(20_000);
    expect(turnDurationMs('Say exactly: "x"', 3000)).toBe(3000);
  });

  it('ends the turn in flight on an interrupt, with an error result and no reply', async () => {
    const child = spawn(process.execPath, [FAKE], { env: process.env });
    const lines: Line[] = [];
    const done = new Promise<void>((resolve) => {
      createInterface({ input: child.stdout }).on('line', (raw) => {
        const line = JSON.parse(raw);
        lines.push(line);
        if (line.isReplay) child.stdin.write(`${JSON.stringify(control('interrupt'))}\n`);
        if (line.type === 'result') resolve();
      });
    });
    const started = Date.now();
    child.stdin.write(`${JSON.stringify(user('Take 30 seconds. Say exactly: "never"'))}\n`);
    await done;
    child.kill();
    expect(Date.now() - started).toBeLessThan(10_000);
    expect(lines.find((l) => l.type === 'assistant')).toBeUndefined();
    expect(lines.at(-1)).toMatchObject({ type: 'result', subtype: 'error_during_execution', is_error: true });
  });

  it('answers a prompt it has no rule for with the fixed reply, never a guess', () => {
    const lines = runFake([], [user('What is the codeword?')]);
    expect(lines.at(-1)?.result).toBe(NO_RULE_REPLY);
  });

  it('reads the text of a Fail to start prompt, and nothing else', () => {
    expect(failToStartText('Fail to start: "claude-gateway.example.com refused the request"')).toBe('claude-gateway.example.com refused the request');
    expect(failToStartText('Please fail to start: "x"')).toBe('x');
    expect(failToStartText('Say exactly: "x"')).toBeUndefined();
  });

  it('dies on a Fail to start prompt: its text on stderr, exit code 1, no init and no result', () => {
    const input = [user('Fail to start: "claude-gateway.example.com refused the request"')].map((l) => `${JSON.stringify(l)}\n`).join('');
    const run = spawnSync(process.execPath, [FAKE], { input, encoding: 'utf8', env: NO_DELAY });
    expect(run.status).toBe(1);
    expect(run.stderr).toBe('claude-gateway.example.com refused the request\n');
    expect(run.stdout).toBe('');
  });

  // A fresh session's first message, shaped like `SessionContext::render`.
  const withSessionContext = (history: string, turn: string) => [
    'SESSION CONTEXT: Lucidos sends this once.',
    'BRANCH: Your worktree is on branch `lucidos-x`.',
    `THREAD HISTORY: earlier:\n\n${history}`,
    '---\nEnd of session context.',
    turn,
  ].join('\n\n');

  it('names the branch the session context names, and says when there is none', () => {
    expect(replyTo(withSessionContext('', "Name your session's branch."))).toBe('lucidos-x');
    expect(replyTo("Name your session's branch.")).toBe(NO_SESSION_CONTEXT_REPLY);
  });

  it('reads its rules from the turn, never from the history before it', () => {
    const prompt = withSessionContext('**User:** Say exactly: "old"\n\n**User:** Fail to start: "old"', 'Say exactly: "new"');
    expect(replyTo(prompt)).toBe('new');
    expect(failToStartText(prompt)).toBeUndefined();
    expect(replyTo(withSessionContext('', 'What is the codeword?'))).toBe(NO_RULE_REPLY);
  });

  it('starts normally when Fail to start arrives after the session began', () => {
    const lines = runFake([], [user('Say exactly: "up"'), user('Fail to start: "late"')]);
    expect(lines.filter((l) => l.type === 'result').map((l) => l.result)).toEqual(['up', NO_RULE_REPLY]);
  });

  it('answers control requests as the real CLI does, refusing set_reasoning_effort', () => {
    const subtypes = ['set_model', 'interrupt', 'cancel_async_message', 'set_reasoning_effort'];
    const lines = runFake([], subtypes.map((s) => control(s, { model: 'haiku' })));
    expect(lines.map((l) => [l.response.request_id, l.response.subtype])).toEqual([
      ['r-set_model', 'success'], ['r-interrupt', 'success'], ['r-cancel_async_message', 'success'],
      ['r-set_reasoning_effort', 'error'],
    ]);
  });
});

describe('the drift check shape diff', () => {
  const fakeTurn = () => runFake(['--model', 'haiku'], [user('Say exactly: "x"')]);

  it('finds no drift between two identical streams', () => {
    expect(diffShapes(shapeOf(fakeTurn()), shapeOf(fakeTurn()))).toEqual([]);
  });

  it('ignores what the engine skips: hooks, deltas and thinking frames', () => {
    const fake = fakeTurn();
    const real = [
      { type: 'system', subtype: 'hook_started' },
      ...fake.slice(0, 3),
      { type: 'stream_event', event: { type: 'content_block_delta' } },
      { type: 'assistant', message: { id: 'm', model: 'x', usage: {}, content: [{ type: 'thinking', thinking: '…' }] } },
      ...fake.slice(3),
    ];
    expect(diffShapes(shapeOf(real), shapeOf(fake))).toEqual([]);
    expect(unknownKinds(real)).toEqual([]);
  });

  it('fails when the real side drops a field the engine parses', () => {
    const fake = fakeTurn();
    const real = fake.map((l) => (l.type === 'result' ? { ...l, duration_ms: undefined } : l));
    expect(diffShapes(shapeOf(real), shapeOf(fake))).not.toEqual([]);
  });

  it('fails when one side sends a parsed frame twice', () => {
    const fake = fakeTurn();
    const assistant = fake.find((l) => l.type === 'assistant')!;
    const real = fake.flatMap((l) => (l === assistant ? [l, l] : [l]));
    expect(diffShapes(shapeOf(real), shapeOf(fake))).not.toEqual([]);
  });

  it('fails when the order changes', () => {
    const fake = fakeTurn();
    const real = [fake[1], fake[0], ...fake.slice(2)];
    expect(diffShapes(shapeOf(real), shapeOf(fake))).not.toEqual([]);
  });

  it('reports a new line kind only the real side sends, without failing', () => {
    const fake = fakeTurn();
    const real = [...fake, { type: 'system', subtype: 'brand_new' }];
    expect(diffShapes(shapeOf(real), shapeOf(fake))).toEqual([]);
    expect(unknownKinds(real)).toEqual(['system/brand_new']);
  });

  it('fails when a control request the fake acknowledges is refused by the real side', () => {
    const [fake] = runFake([], [control('interrupt')]);
    const refused = { ...fake, response: { subtype: 'error', request_id: fake.response.request_id, error: 'no' } };
    expect(diffShapes(shapeOf([refused]), shapeOf([fake]))).not.toEqual([]);
  });

  it('places every flag the engine spawns Claude Code with', () => {
    const source = readFileSync(join(__dirname, '../../../lucidos-engine/src/runtime/claude_code.rs'), 'utf8');
    const start = source.indexOf('fn build_command_with_settings(');
    const body = source.slice(start, source.indexOf('\n}\n', start));
    const engineFlags = [...new Set(body.match(/"--[a-z-]+"/g)!.map((f) => f.slice(1, -1)))].sort();
    const placed = [...SESSION_FLAGS, ...SCENARIO_FLAGS, ...UNREPLAYED_FLAGS].filter((f) => f.startsWith('--'));
    expect([...new Set(placed)].sort()).toEqual(engineFlags);
  });

  it('compares model field shapes, not how many models a machine lists', () => {
    const [fake] = runFake([], [control('initialize')]);
    const models = fake.response.response.models;
    const more = { ...fake, response: { ...fake.response, response: { models: [...models, models[0]] } } };
    expect(diffShapes(shapeOf([more]), shapeOf([fake]))).toEqual([]);
    const fewerFields = { ...fake, response: { ...fake.response, response: { models: models.map(({ displayName: _, ...m }: Line) => m) } } };
    expect(diffShapes(shapeOf([fewerFields]), shapeOf([fake]))).not.toEqual([]);
  });

  describe('model row shapes', () => {
    const [initialize] = runFake([], [control('initialize')]);
    const withModels = (models: Line[]): Line => ({
      ...initialize, response: { ...initialize.response, response: { models } },
    });
    const plain = { value: 'a', resolvedModel: 'a-1', displayName: 'A', description: 'plain' };
    const effort = { ...plain, value: 'b', supportsEffort: true, supportedEffortLevels: ['low', 'high'] };

    it('passes when the real and fake rows have the same shapes', () => {
      expect(diffShapes(shapeOf([withModels([plain, effort])]), shapeOf([withModels([effort, plain])]))).toEqual([]);
    });

    it('passes when the fake has a row shape the real CLI does not send', () => {
      expect(diffShapes(shapeOf([withModels([plain])]), shapeOf([withModels([plain, effort])]))).toEqual([]);
    });

    it('fails when a real row shape is missing from the fake, and names it', () => {
      const diffs = diffShapes(shapeOf([withModels([plain, effort])]), shapeOf([withModels([plain])]));
      expect(diffs).toHaveLength(1);
      expect(diffs[0]).toContain('fake lacks model row shape [value,resolvedModel,displayName,description,supportsEffort,supportedEffortLevels]');
    });

    it('still fails when the rest of the line differs, extra shapes or not', () => {
      const refused = { ...withModels([plain, effort]), response: { subtype: 'error', request_id: initialize.response.request_id } };
      expect(diffShapes(shapeOf([refused]), shapeOf([withModels([plain, effort])]))).not.toEqual([]);
    });
  });
});
