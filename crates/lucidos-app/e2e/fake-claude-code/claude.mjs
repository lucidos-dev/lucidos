#!/usr/bin/env node
// A fake Claude Code CLI for e2e runs on GitHub runners, which have no
// logged-in Claude Code. It speaks the stream-json lines the engine parses
// (runtime/claude_code_parse.rs) and answers `Say exactly: "X"` with X.
// Any other prompt gets NO_RULE_REPLY, so a spec that needs a real model
// fails loudly instead of passing by luck. Such a spec carries the
// @real-claude-code tag and stays on the local leg.
//
// drift-check.mjs keeps these lines in step with the real CLI.

import { randomUUID } from 'node:crypto';
import { realpathSync } from 'node:fs';
import { createInterface } from 'node:readline';
import { pathToFileURL } from 'node:url';

export const FAKE_VERSION = '0.0.0-fake';
export const NO_RULE_REPLY = 'fake-claude-code: no rule for this prompt';

// A real turn takes seconds, and the engine stops the process at its result.
// Specs that poll for a live session mid-turn need the turn to last that long.
// FAKE_CLAUDE_CODE_TURN_MS overrides it.
export const DEFAULT_TURN_MS = 3000;

// What `initialize` lists, in Claude Code's order. `resolvedModel` is what
// `system/init` and each assistant frame report for the alias.
export const MODELS = [
  { value: 'default', resolvedModel: 'claude-opus-5-5', displayName: 'Default', description: 'Use the default model', supportsEffort: true, supportedEffortLevels: ['low', 'medium', 'high', 'xhigh', 'max'] },
  { value: 'sonnet', resolvedModel: 'claude-sonnet-5', displayName: 'Sonnet 5', description: 'Sonnet 5', supportsEffort: true, supportedEffortLevels: ['low', 'medium', 'high', 'xhigh', 'max'] },
  { value: 'opus', resolvedModel: 'claude-opus-5-5', displayName: 'Opus 5.5', description: 'Opus 5.5', supportsEffort: true, supportedEffortLevels: ['low', 'medium', 'high', 'xhigh', 'max'] },
  { value: 'haiku', resolvedModel: 'claude-haiku-4-5', displayName: 'Haiku 4.5', description: 'Haiku 4.5' },
];

const SLASH_COMMANDS = ['compact', 'context', 'cost', 'init', 'review'];

export function resolveModel(alias) {
  return MODELS.find((m) => m.value === alias)?.resolvedModel ?? alias;
}

export function replyTo(prompt) {
  const match = /Say exactly:\s*"([^"]*)"/.exec(prompt);
  return match ? match[1] : NO_RULE_REPLY;
}

function promptText(content) {
  if (typeof content === 'string') return content;
  return (content ?? []).filter((b) => b.type === 'text').map((b) => b.text).join('\n');
}

function flagValue(argv, name) {
  const at = argv.indexOf(name);
  return at >= 0 ? argv[at + 1] : undefined;
}

/** One session: turns each stdin line into the lines Claude Code would print. */
export function createSession(argv, cwd, turnMs = DEFAULT_TURN_MS) {
  const sessionId = flagValue(argv, '--resume') ?? randomUUID();
  let model = resolveModel(flagValue(argv, '--model') ?? 'default');
  let initSent = false;
  const usage = { input_tokens: 10, cache_creation_input_tokens: 0, cache_read_input_tokens: 0, output_tokens: 3 };

  const controlResponse = (requestId, response) => ({
    type: 'control_response',
    response: { subtype: 'success', request_id: requestId, ...(response && { response }) },
  });

  function control(line) {
    const { request_id: id, request } = line;
    switch (request?.subtype) {
      case 'initialize':
        return [controlResponse(id, { models: MODELS, commands: [] })];
      case 'set_model':
        model = resolveModel(request.model);
        return [controlResponse(id)];
      case 'cancel_async_message':
        return [controlResponse(id, { cancelled: false })];
      case 'interrupt':
        return [controlResponse(id, { still_queued: [] })];
      default:
        // The real CLI refuses set_reasoning_effort this way too: effort
        // reaches it only through CLAUDE_CODE_EFFORT_LEVEL at spawn.
        return [{ type: 'control_response', response: { subtype: 'error', request_id: id, error: `Unsupported control request subtype: ${request?.subtype}` } }];
    }
  }

  function turn(line) {
    const out = [];
    if (!initSent) {
      initSent = true;
      out.push({
        type: 'system', subtype: 'init', session_id: sessionId, model, cwd,
        claude_code_version: FAKE_VERSION, slash_commands: SLASH_COMMANDS, skills: [],
        tools: [], mcp_servers: [], permissionMode: flagValue(argv, '--permission-mode') ?? 'default',
        apiKeySource: 'none', uuid: randomUUID(),
      });
    }
    out.push({ ...line, session_id: sessionId, timestamp: new Date().toISOString(), isReplay: true });
    const text = replyTo(promptText(line.message?.content));
    const id = `msg_fake_${randomUUID().replaceAll('-', '')}`;
    const message = { model, id, type: 'message', role: 'assistant', usage };
    const reply = [
      { type: 'stream_event', session_id: sessionId, parent_tool_use_id: null, event: { type: 'message_start', message: { ...message, content: [] } } },
      { type: 'assistant', session_id: sessionId, parent_tool_use_id: null, message: { ...message, content: [{ type: 'text', text }] } },
      { type: 'stream_event', session_id: sessionId, parent_tool_use_id: null, event: { type: 'message_delta', delta: { stop_reason: 'end_turn' }, usage } },
      { type: 'stream_event', session_id: sessionId, parent_tool_use_id: null, event: { type: 'message_stop' } },
      {
        type: 'result', subtype: 'success', is_error: false, result: text, session_id: sessionId,
        duration_ms: turnMs, num_turns: 1, stop_reason: 'end_turn', usage, uuid: randomUUID(),
      },
    ];
    return { now: out, later: reply, isTurn: true };
  }

  return {
    sessionId,
    /** `now` prints at once; `later` prints `turnMs` after, once earlier turns finished. */
    handle(raw) {
      let line;
      try {
        line = JSON.parse(raw);
      } catch {
        return { now: [], later: [], isTurn: false };
      }
      if (line.type === 'control_request') return { now: control(line), later: [], isTurn: false };
      if (line.type === 'user') return turn(line);
      return { now: [], later: [], isTurn: false };
    },
  };
}

function main(argv) {
  if (argv.includes('--version')) {
    process.stdout.write(`${FAKE_VERSION} (Claude Code)\n`);
    return;
  }
  const turnMs = Number(process.env.FAKE_CLAUDE_CODE_TURN_MS ?? DEFAULT_TURN_MS);
  const session = createSession(argv, process.cwd(), turnMs);
  const print = (lines) => { for (const line of lines) process.stdout.write(`${JSON.stringify(line)}\n`); };
  let turns = Promise.resolve();
  const input = createInterface({ input: process.stdin });
  input.on('line', (raw) => {
    const { now, later, isTurn } = session.handle(raw);
    if (!isTurn) {
      print(now);
      return;
    }
    turns = turns.then(async () => {
      print(now);
      await new Promise((done) => setTimeout(done, turnMs));
      print(later);
    });
  });
}

// The runner starts this through a symlink, so compare real paths.
const invokedAs = process.argv[1] && pathToFileURL(realpathSync(process.argv[1])).href;
if (import.meta.url === invokedAs) main(process.argv.slice(2));
