#!/usr/bin/env node

import { randomUUID } from 'node:crypto';
import { spawnSync } from 'node:child_process';
import { realpathSync } from 'node:fs';
import { createInterface } from 'node:readline';
import { fileURLToPath, pathToFileURL } from 'node:url';

export const FAKE_VERSION = '2.1.311';
export const NO_RULE_REPLY = 'fake-claude-code: no rule for this prompt';
export const NO_SESSION_CONTEXT_REPLY = 'fake-claude-code: no session context';
export const NO_EFFORT = 'none';

export const DEFAULT_TURN_MS = 3000;

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

const SESSION_CONTEXT_END = '---\nEnd of session context.';

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

export function turnDurationMs(prompt, turnMs) {
  const seconds = /Take (\d+) seconds/.exec(turnText(prompt))?.[1];
  return seconds === undefined ? turnMs : Number(seconds) * 1000;
}

export function failToStartText(prompt) {
  return /[Ff]ail to start:\s*"([^"]*)"/.exec(turnText(prompt))?.[1];
}

export function modelBase(id) {
  return id.replace(/\[1m\]$/, '').split('@')[0];
}

export function restrictedModels(list = '') {
  return new Set(list.split(',').map((id) => id.trim()).filter(Boolean).map(modelBase));
}

export function restrictionWarning(requested, served) {
  return {
    type: 'system', subtype: 'informational', level: 'warning',
    content: `Model "${requested}" is restricted by your organization's settings. Using ${served} instead.`,
  };
}

function promptText(content) {
  if (typeof content === 'string') return content;
  return (content ?? []).filter((b) => b.type === 'text').map((b) => b.text).join('\n');
}

function flagValue(argv, name) {
  const at = argv.indexOf(name);
  return at >= 0 ? argv[at + 1] : undefined;
}

export function createSession(argv, cwd, turnMs = DEFAULT_TURN_MS, effort = NO_EFFORT, answer = replyTo, restricted = new Set(), mcpServers = [], toolCallFor = () => undefined) {
  const sessionId = flagValue(argv, '--resume') ?? randomUUID();
  let model;
  let warning;
  const pick = (requested) => {
    if (restricted.has(modelBase(requested))) {
      model = resolveModel('default');
      warning = restrictionWarning(requested, model);
    } else {
      model = resolveModel(requested);
      warning = undefined;
    }
  };
  pick(flagValue(argv, '--model') ?? 'default');
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
        pick(request.model);
        return [controlResponse(id)];
      case 'cancel_async_message':
        return [controlResponse(id, { cancelled: false })];
      case 'interrupt':
        return [controlResponse(id, { still_queued: [] })];
      default:
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
        tools: [], mcp_servers: mcpServers, permissionMode: flagValue(argv, '--permission-mode') ?? 'default',
        apiKeySource: 'none', uuid: randomUUID(),
      });
    }
    if (warning) {
      out.push(warning);
      warning = undefined;
    }
    out.push({ ...line, session_id: sessionId, timestamp: new Date().toISOString(), isReplay: true });
    const prompt = promptText(line.message?.content);
    const text = answer(prompt, effort);
    const id = `msg_fake_${randomUUID().replaceAll('-', '')}`;
    const message = { model, id, type: 'message', role: 'assistant', usage };
    const tool = toolCallFor(prompt);
    const toolUseId = `toolu_fake_${randomUUID().replaceAll('-', '')}`;
    const toolCall = tool === undefined ? [] : [
      { type: 'assistant', session_id: sessionId, parent_tool_use_id: null, message: { ...message, id: `${id}_tool`, content: [{ type: 'tool_use', id: toolUseId, name: tool, input: {} }] } },
      { type: 'user', session_id: sessionId, parent_tool_use_id: null, message: { role: 'user', content: [{ type: 'tool_result', tool_use_id: toolUseId, content: 'Authentication required', is_error: true }] } },
    ];
    const reply = [
      ...toolCall,
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

export function main(argv, options = {}) {
  if (argv[0] === 'auth' && argv[1] === 'status' && argv.includes('--json')) {
    const loggedIn = process.env.FAKE_CLAUDE_LOGGED_OUT !== '1';
    console.log(JSON.stringify({ loggedIn, apiProvider: 'firstParty' }));
    process.exitCode = loggedIn ? 0 : 1;
    return;
  }
  if (argv.includes('--version')) {
    process.stdout.write(`${FAKE_VERSION} (Claude Code)\n`);
    return;
  }
  const turnMs = Number(process.env.FAKE_CLAUDE_CODE_TURN_MS ?? DEFAULT_TURN_MS);
  const answer = (prompt, effort) => {
    const override = options.answer?.(prompt);
    if (override !== undefined) return override;
    const fitRenderer = turnText(prompt).match(/Probe fit renderer: (\{[^\n]*\})/);
    if (fitRenderer) {
      const request = JSON.parse(fitRenderer[1]);
      const result = spawnSync(process.execPath, [fileURLToPath(new URL('./fit-renderer-probe.mjs', import.meta.url)), JSON.stringify(request.widgets), request.browser], {
        encoding: 'utf8', timeout: 180000, maxBuffer: 131072,
      });
      const proof = result.status === 0 ? result.stdout.trim() : '{"verified":false,"stage":"launcher"}';
      return '```json\n' + proof + '\n```';
    }
    const routingCli = turnText(prompt).match(/Probe routing CLI: (\{[^\n]*\})/);
    if (routingCli) {
      const request = JSON.parse(routingCli[1]);
      const result = spawnSync(request.cli, request.args, {
        encoding: 'utf8', timeout: 45000, maxBuffer: 1048576,
      });
      let data = null;
      try { data = JSON.parse(result.stdout); } catch {  }
      const link = result.stdout?.match(/thread:[a-zA-Z0-9_@.-]+\/[a-f0-9-]{36}/)?.[0] ?? null;
      return JSON.stringify({ code: result.status ?? 4, data, link,
        liveness: result.stderr?.includes('# thread:') ?? false });
    }
    const restoreCli = turnText(prompt).match(/Probe snapshot CLI restore: (\{[^\n]*\})/);
    if (restoreCli) {
      const request = JSON.parse(restoreCli[1]);
      const result = spawnSync(request.cli, ['snapshot', 'restore', request.handle,
        '--workspace', request.workspace, '--expected-workspace-id', request.workspace,
        '--port', String(request.port), '--wait', '--json'], {
        encoding: 'utf8', timeout: 120000, maxBuffer: 65536,
      });
      let status = null;
      try { status = JSON.parse(result.stdout); } catch {  }
      return JSON.stringify({ code: result.status ?? 4, status });
    }
    const selfRestore = turnText(prompt).includes('Probe snapshot self-restore refusal.');
    const gateway = turnText(prompt).match(/Probe snapshot gateway restore origin: (http:\/\/127\.0\.0\.1:[0-9]+)\./)?.[1];
    if (!gateway && !selfRestore && !turnText(prompt).includes('Probe snapshot restore origin.')) return replyTo(prompt, effort);
    const result = spawnSync(process.execPath, [fileURLToPath(new URL('./restore-origin-probe.mjs', import.meta.url)), ...(gateway ? ['--gateway', gateway] : selfRestore ? ['--self-restore'] : [])], {
      encoding: 'utf8', timeout: 55000, maxBuffer: 8192,
    });
    return result.status === 0 ? result.stdout.trim() : '{"verified":false}';
  };
  const mcpServers = options.mcpServers ?? JSON.parse(process.env.FAKE_CLAUDE_MCP_SERVERS ?? '[]');
  const session = createSession(argv, process.cwd(), turnMs, process.env.CLAUDE_CODE_EFFORT_LEVEL ?? NO_EFFORT, answer,
    restrictedModels(process.env.FAKE_CLAUDE_CODE_RESTRICTED_MODELS), mcpServers, options.toolCall);
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

const invokedAs = process.argv[1] && pathToFileURL(realpathSync(process.argv[1])).href;
if (import.meta.url === invokedAs) main(process.argv.slice(2));
