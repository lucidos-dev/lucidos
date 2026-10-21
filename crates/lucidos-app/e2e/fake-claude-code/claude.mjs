#!/usr/bin/env node
// A fake Claude Code CLI for e2e runs on GitHub runners, which have no
// logged-in Claude Code. It speaks the stream-json lines the engine parses
// (runtime/claude_code_parse.rs) and answers `Say exactly: "X"` with X.
// `claude auth status` answers as a first-party login.
// `Fail to start: "X"` makes the first turn die the way a CLI that cannot reach
// its gateway does: X on stderr, exit code 1, and no `system/init` line.
// `Name your session's branch` answers with the branch the message's session
// context names (ADR 0491), or NO_SESSION_CONTEXT_REPLY.
// A turn naming `your reasoning effort` answers `effort: X`, X being the
// CLAUDE_CODE_EFFORT_LEVEL it was spawned with. The effort restart's resume
// note names it too, so a spec sees what the restarted process runs at.
// `Take N seconds` makes that turn last N seconds. An interrupt ends the
// turn in flight at once, with an error result and no reply.
// A fresh session's first message carries the thread history ahead of the
// turn, so the other rules read only what follows the session context.
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
export const NO_SESSION_CONTEXT_REPLY = 'fake-claude-code: no session context';
export const NO_EFFORT = 'none';

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

// What `claude auth status` prints. The engine reads `apiProvider` at spawn to
// learn whether the agent's progress notes reach the user.
export const AUTH_STATUS = { loggedIn: true, authMethod: 'claude.ai', apiProvider: 'firstParty' };

const SLASH_COMMANDS = ['compact', 'context', 'cost', 'init', 'review'];

export function resolveModel(alias) {
  return MODELS.find((m) => m.value === alias)?.resolvedModel ?? alias;
}

// Closes the session context block (`SessionContext::render`).
const SESSION_CONTEXT_END = '---\nEnd of session context.';

/** The prompt without its session context block: what this turn says. */
function turnText(prompt) {
  return prompt.split(SESSION_CONTEXT_END).at(-1);
}

export function replyTo(prompt, effort = NO_EFFORT) {
  const turn = turnText(prompt);
  if (turn.includes('your reasoning effort')) return `effort: ${effort}`;
  if (turn.includes("Name your session's branch")) {
    const branch = /BRANCH: Your worktree is on branch `([^`]+)`/.exec(prompt)?.[1];
    return prompt.includes(SESSION_CONTEXT_END) && branch ? branch : NO_SESSION_CONTEXT_REPLY;
  }
  const match = /Say exactly:\s*"([^"]*)"/.exec(turn);
  return match ? match[1] : NO_RULE_REPLY;
}

/** How long a turn lasts: its own `Take N seconds`, else `turnMs`. */
export function turnDurationMs(prompt, turnMs) {
  const seconds = /Take (\d+) seconds/.exec(turnText(prompt))?.[1];
  return seconds === undefined ? turnMs : Number(seconds) * 1000;
}

export function failToStartText(prompt) {
  return /[Ff]ail to start:\s*"([^"]*)"/.exec(turnText(prompt))?.[1];
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
export function createSession(argv, cwd, turnMs = DEFAULT_TURN_MS, effort = NO_EFFORT) {
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
    const failure = initSent ? undefined : failToStartText(promptText(line.message?.content));
    if (failure !== undefined) return { now: [], later: [], isTurn: false, failToStart: failure };
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
    const prompt = promptText(line.message?.content);
    const text = replyTo(prompt, effort);
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
    const interrupted = [{
      type: 'result', subtype: 'error_during_execution', is_error: true, session_id: sessionId,
      duration_ms: 0, num_turns: 1, stop_reason: null, usage, uuid: randomUUID(),
    }];
    return { now: out, later: reply, interrupted, isTurn: true, delayMs: turnDurationMs(prompt, turnMs) };
  }

  return {
    sessionId,
    /** `now` prints at once; `later` prints `delayMs` after, once earlier turns
     *  finished, or `interrupted` does if an interrupt lands first. */
    handle(raw) {
      let line;
      try {
        line = JSON.parse(raw);
      } catch {
        return { now: [], later: [], isTurn: false };
      }
      if (line.type === 'control_request') {
        return { now: control(line), later: [], isTurn: false, interrupt: line.request?.subtype === 'interrupt' };
      }
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
  if (argv[0] === 'auth' && argv[1] === 'status') {
    process.stdout.write(`${JSON.stringify(AUTH_STATUS)}\n`);
    return;
  }
  const turnMs = Number(process.env.FAKE_CLAUDE_CODE_TURN_MS ?? DEFAULT_TURN_MS);
  const session = createSession(argv, process.cwd(), turnMs, process.env.CLAUDE_CODE_EFFORT_LEVEL ?? NO_EFFORT);
  const print = (lines) => { for (const line of lines) process.stdout.write(`${JSON.stringify(line)}\n`); };
  let turns = Promise.resolve();
  let interruptTurn = () => {};
  const input = createInterface({ input: process.stdin });
  input.on('line', (raw) => {
    const { now, later, interrupted, isTurn, failToStart, interrupt, delayMs } = session.handle(raw);
    if (failToStart !== undefined) {
      process.stderr.write(`${failToStart}\n`, () => process.exit(1));
      return;
    }
    if (!isTurn) {
      print(now);
      if (interrupt) interruptTurn();
      return;
    }
    turns = turns.then(async () => {
      print(now);
      const wasInterrupted = await new Promise((done) => {
        const timer = setTimeout(() => done(false), delayMs);
        interruptTurn = () => { clearTimeout(timer); done(true); };
      });
      interruptTurn = () => {};
      print(wasInterrupted ? interrupted : later);
    });
  });
}

// The runner starts this through a symlink, so compare real paths.
const invokedAs = process.argv[1] && pathToFileURL(realpathSync(process.argv[1])).href;
if (import.meta.url === invokedAs) main(process.argv.slice(2));
