#!/usr/bin/env node
// Replays the same scripted turns against the real Claude Code and the fake,
// then diffs what the engine would parse from each. Run by the workspace
// trigger claude-code-fake-drift when the installed version changes.
//
//   node drift-check.mjs --real <path to claude>
//
// Exit 0: in step. 1: drift. 2: could not run, which is unknown, not a pass.
// The real side spends two short Haiku turns.

import { spawn } from 'node:child_process';
import { existsSync, mkdtempSync, readdirSync, realpathSync, rmSync } from 'node:fs';
import { homedir, tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

export const EXIT_IN_STEP = 0;
export const EXIT_DRIFT = 1;
export const EXIT_UNKNOWN = 2;

const FAKE = join(dirname(fileURLToPath(import.meta.url)), 'claude.mjs');
const TURN_TIMEOUT_MS = 120_000;

// The engine's session spawn flags (build_command_with_settings in
// runtime/claude_code.rs), split three ways. fake-claude-code.test.ts pins the
// union to that function, so a new engine flag fails until it is placed here.
// Flags that shape the stream, passed on every spawn:
export const SESSION_FLAGS = [
  '--input-format', 'stream-json', '--output-format', 'stream-json', '--verbose',
  '--replay-user-messages', '--include-partial-messages',
];
// Flags each scenario sets itself:
export const SCENARIO_FLAGS = ['--print', '--resume', '--model'];
// Flags left out, because none changes a line the check diffs:
export const UNREPLAYED_FLAGS = [
  '--permission-mode', '--permission-prompt-tool', '--mcp-config', '--settings', '--add-dir', '--append-system-prompt-file',
];

// Lines the engine reads, and the fields it reads from each
// (runtime/claude_code_parse.rs, cc_model_discovery.rs).
const PARSED_MODEL_FIELDS = ['value', 'resolvedModel', 'displayName', 'description', 'supportsEffort', 'supportedEffortLevels'];

// Lines the engine skips today. A real-only line outside this list and the
// parsed set is reported as a notice: Claude Code grew something new.
const KNOWN_IGNORED = new Set([
  'system/hook_started', 'system/hook_progress', 'system/hook_response', 'system/status',
  'system/thinking_tokens', 'command_lifecycle', 'rate_limit_event', 'assistant',
  'stream_event/content_block_start', 'stream_event/content_block_delta',
  'stream_event/content_block_stop', 'stream_event/message_stop',
]);

const keysOf = (obj, wanted) => wanted.filter((k) => obj != null && obj[k] !== undefined).join(',');

/** The engine's view of one line, or null when the engine skips it. */
export function signature(line) {
  const kind = line.subtype ? `${line.type}/${line.subtype}` : line.type;
  switch (line.type) {
    case 'control_response': {
      const r = line.response ?? {};
      // Which models a machine lists depends on its settings, so only the
      // distinct field shapes count.
      const models = r.response?.models;
      const shapes = Array.isArray(models) ? [...new Set(models.map((m) => keysOf(m, PARSED_MODEL_FIELDS)))].sort() : null;
      const modelFields = shapes ? `models[${shapes.join('|')}]` : '';
      // The engine reads success or error off `subtype`, so its value counts.
      return `control_response/${r.subtype}{${keysOf(r, ['request_id'])}}${modelFields}`;
    }
    case 'system':
      return line.subtype === 'init'
        ? `system/init{${keysOf(line, ['session_id', 'model', 'slash_commands', 'skills', 'claude_code_version'])}}`
        : null;
    case 'user':
      return line.isReplay === true ? `user/replay{${keysOf(line.message, ['content'])}}` : 'user/tool_result';
    case 'assistant': {
      // Thinking frames are model behaviour, and the engine renders them only
      // for relayed models, so only text and tool_use blocks count.
      const blocks = (line.message?.content ?? []).map((b) => b.type).filter((t) => t === 'text' || t === 'tool_use');
      if (blocks.length === 0) return null;
      return `assistant{${keysOf(line.message, ['id', 'model', 'usage'])}}[${blocks.join(',')}]`;
    }
    case 'stream_event': {
      const event = line.event ?? {};
      if (event.type === 'message_start') return `stream_event/message_start{${keysOf(event.message, ['id', 'model', 'usage'])}}`;
      if (event.type === 'message_delta') return `stream_event/message_delta{${keysOf(event, ['usage'])}}`;
      return null;
    }
    case 'result':
      return `${kind}{${keysOf(line, ['result', 'duration_ms', 'is_error'])}}`;
    default:
      return null;
  }
}

/** The ordered signatures of a stream. The engine parses every frame, so repeats count. */
export function shapeOf(lines) {
  return lines.map(signature).filter((sig) => sig !== null);
}

/** Line kinds only the real side sent that the engine has never seen. */
export function unknownKinds(lines) {
  const kinds = new Set();
  for (const line of lines) {
    if (signature(line) !== null) continue;
    const sub = line.subtype ?? line.event?.type;
    const kind = sub ? `${line.type}/${sub}` : line.type;
    if (!KNOWN_IGNORED.has(kind)) kinds.add(kind);
  }
  return [...kinds];
}

/** Every difference between two shapes, as readable lines. Empty when in step. */
export function diffShapes(real, fake) {
  const diffs = [];
  const length = Math.max(real.length, fake.length);
  for (let i = 0; i < length; i++) {
    if (real[i] !== fake[i]) diffs.push(`#${i}: real ${real[i] ?? '(none)'} | fake ${fake[i] ?? '(none)'}`);
  }
  return diffs;
}

/** Run one scenario: write `inputs` in turn, each after the reply to the last. */
function runScenario(binary, args, cwd, inputs) {
  return new Promise((resolve, reject) => {
    const child = spawn(binary, args, { cwd, stdio: ['pipe', 'pipe', 'pipe'] });
    const lines = [];
    let buffer = '';
    let next = 0;
    let stderr = '';
    const timer = setTimeout(() => {
      child.kill('SIGKILL');
      reject(new Error(`no reply within ${TURN_TIMEOUT_MS / 1000}s. stderr: ${stderr.slice(-300)}`));
    }, TURN_TIMEOUT_MS);
    const send = () => {
      if (next < inputs.length) child.stdin.write(`${JSON.stringify(inputs[next++].line)}\n`);
      else child.stdin.end();
    };
    child.stderr.on('data', (d) => { stderr += d; });
    child.stdout.on('data', (chunk) => {
      buffer += chunk;
      let at;
      while ((at = buffer.indexOf('\n')) >= 0) {
        const raw = buffer.slice(0, at);
        buffer = buffer.slice(at + 1);
        let line;
        try { line = JSON.parse(raw); } catch { continue; }
        lines.push(line);
        if (inputs[next - 1]?.doneWhen(line)) send();
      }
    });
    child.on('error', (e) => { clearTimeout(timer); reject(e); });
    child.on('close', () => { clearTimeout(timer); resolve(lines); });
    send();
  });
}

const userTurn = (text) => ({
  line: { type: 'user', message: { role: 'user', content: text }, session_id: 'default', parent_tool_use_id: null, uuid: crypto.randomUUID() },
  doneWhen: (l) => l.type === 'result',
});
const controlTurn = (subtype, extra = {}) => {
  const requestId = `drift-${subtype}`;
  return {
    line: { type: 'control_request', request_id: requestId, request: { subtype, ...extra } },
    doneWhen: (l) => l.type === 'control_response' && l.response?.request_id === requestId,
  };
};

/** Every scenario against one CLI, `command` plus its leading `prefix` args. */
async function runAll([command, ...prefix], cwd) {
  const run = (args, inputs) => runScenario(command, [...prefix, ...SESSION_FLAGS, ...args], cwd, inputs);
  const probe = await run(['--no-session-persistence'], [controlTurn('initialize')]);
  const first = await run(['--model', 'haiku'], [userTurn('Say exactly: "drift-one" and nothing else. Do not create any files.')]);
  const sessionId = first.find((l) => l.type === 'system' && l.subtype === 'init')?.session_id;
  if (!sessionId) throw new Error('the first turn reported no session id');
  const resumed = await run(['--model', 'haiku', '--print', '--resume', sessionId], [
    userTurn('Say exactly: "drift-two" and nothing else. Do not create any files.'),
    // The engine never sends this one (CodingAgent::control_reach). It stays as
    // a canary: the day the real CLI accepts it, the shapes differ.
    controlTurn('set_reasoning_effort', { effort: 'low' }),
    controlTurn('set_model', { model: 'haiku' }),
    controlTurn('interrupt'),
  ]);
  const resumedId = resumed.find((l) => l.type === 'system' && l.subtype === 'init')?.session_id;
  return { probe, first, resumed, resumeKeptId: resumedId === sessionId };
}

function projectsDir() {
  return join(process.env.CLAUDE_CONFIG_DIR ?? join(homedir(), '.claude'), 'projects');
}

const listDir = (dir) => (existsSync(dir) ? readdirSync(dir) : []);

async function main(argv) {
  const at = argv.indexOf('--real');
  const real = at >= 0 ? argv[at + 1] : undefined;
  if (!real || !existsSync(real)) {
    console.log(`UNKNOWN: pass --real <path to claude>; got ${real ?? 'nothing'}`);
    return EXIT_UNKNOWN;
  }
  if (realpathSync(real) === realpathSync(FAKE)) {
    console.log(`UNKNOWN: ${real} is the fake itself`);
    return EXIT_UNKNOWN;
  }
  const work = mkdtempSync(join(tmpdir(), 'cc-drift-'));
  const projectsBefore = new Set(listDir(projectsDir()));
  try {
    let realRuns;
    try {
      realRuns = await runAll([real], work);
    } catch (e) {
      console.log(`UNKNOWN: the real claude did not complete the script: ${e.message}`);
      return EXIT_UNKNOWN;
    }
    let fakeRuns;
    try {
      fakeRuns = await runAll([process.execPath, FAKE], work);
    } catch (e) {
      // The real CLI completed the script and the fake did not: the fake is what broke.
      console.log(`DRIFT fake: the fake did not complete the script: ${e.message}`);
      return EXIT_DRIFT;
    }
    console.log(`real: ${realpathSync(real)}`);
    let drift = false;
    for (const name of ['probe', 'first', 'resumed']) {
      const diffs = diffShapes(shapeOf(realRuns[name]), shapeOf(fakeRuns[name]));
      for (const d of diffs) console.log(`DRIFT ${name} ${d}`);
      drift ||= diffs.length > 0;
      for (const kind of unknownKinds(realRuns[name])) console.log(`NOTICE ${name}: real sends ${kind}, which the engine ignores`);
    }
    for (const [side, runs] of [['real', realRuns], ['fake', fakeRuns]]) {
      if (!runs.resumeKeptId) {
        console.log(`DRIFT resumed: ${side} reported a new session id on --resume`);
        drift = true;
      }
    }
    console.log(drift ? 'DRIFT: the fake no longer matches the real Claude Code' : 'IN STEP');
    return drift ? EXIT_DRIFT : EXIT_IN_STEP;
  } finally {
    rmSync(work, { recursive: true, force: true });
    // Only the session files this run created: new entries naming its temp dir.
    const tag = work.split('/').pop();
    for (const entry of listDir(projectsDir())) {
      if (!projectsBefore.has(entry) && entry.includes(tag)) rmSync(join(projectsDir(), entry), { recursive: true, force: true });
    }
  }
}

const invokedAs = process.argv[1] && pathToFileURL(realpathSync(process.argv[1])).href;
// An escaped error would exit 1, which reads as drift. It is unknown instead.
if (import.meta.url === invokedAs) {
  main(process.argv.slice(2)).then(
    (code) => process.exit(code),
    (e) => {
      console.log(`UNKNOWN: the drift check failed: ${e.message}`);
      process.exit(EXIT_UNKNOWN);
    },
  );
}
