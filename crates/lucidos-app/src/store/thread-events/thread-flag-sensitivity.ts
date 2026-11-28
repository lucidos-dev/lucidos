import { exchangeResponseEvents, exchangeSteps, exchangeStatus } from './exchange-render';
import type { Exchange } from './exchange';

export interface FlagSensitivityInputs {
  streamingBuffer: string;
  isLast: boolean;
  isQueued: boolean;
  hasPriorActive: boolean;
  threadIsCC: boolean;
  behindOpenQuestion: boolean;
}

const FLAG_COMBINATIONS: readonly (readonly [threadIdle: boolean, threadAwaitingAnswer: boolean])[] = [
  [false, false],
  [true, false],
  [false, true],
  [true, true],
];

interface Verdict {
  key: string;
  sensitive: boolean;
}

const verdicts = new WeakMap<Exchange, Verdict>();

function sameValue(a: unknown, b: unknown): boolean {
  if (Object.is(a, b)) return true;
  if (typeof a !== 'object' || a === null || typeof b !== 'object' || b === null) return false;
  if (Array.isArray(a) !== Array.isArray(b)) return false;
  const left = a as Record<string, unknown>;
  const right = b as Record<string, unknown>;
  const keys = Object.keys(left);
  return keys.length === Object.keys(right).length && keys.every((key) => sameValue(left[key], right[key]));
}

function drawing(exchange: Exchange, inputs: FlagSensitivityInputs, threadIdle: boolean, threadAwaitingAnswer: boolean) {
  const { streamingBuffer, isLast, hasPriorActive, threadIsCC, behindOpenQuestion } = inputs;
  return {
    status: exchangeStatus(exchange, streamingBuffer, isLast, hasPriorActive, threadIsCC, threadIdle, threadAwaitingAnswer, behindOpenQuestion),
    steps: exchangeSteps(exchange, isLast, threadIdle),
    responseEvents: exchangeResponseEvents(exchange, isLast, threadIdle),
  };
}

function computeSensitive(exchange: Exchange, inputs: FlagSensitivityInputs): boolean {
  const [[idle0, awaiting0], ...rest] = FLAG_COMBINATIONS;
  const base = drawing(exchange, inputs, idle0, awaiting0);
  return rest.some(([threadIdle, threadAwaitingAnswer]) => {
    const other = drawing(exchange, inputs, threadIdle, threadAwaitingAnswer);
    return other.status !== base.status
      || !sameValue(other.steps, base.steps)
      || !sameValue(other.responseEvents, base.responseEvents);
  });
}

function verdictKey(exchange: Exchange, inputs: FlagSensitivityInputs): string {
  const lastSeq = exchange.steps[exchange.steps.length - 1]?.seq;
  return [
    exchange.revision ?? 0,
    exchange.steps.length,
    lastSeq,
    inputs.isLast,
    inputs.hasPriorActive,
    inputs.threadIsCC,
    inputs.behindOpenQuestion,
    inputs.streamingBuffer !== '',
  ].join('|');
}

export function threadFlagsChangeDrawing(exchange: Exchange, inputs: FlagSensitivityInputs): boolean {
  if (inputs.isQueued) return true;
  const key = verdictKey(exchange, inputs);
  const cached = verdicts.get(exchange);
  if (cached?.key === key) return cached.sensitive;
  const sensitive = computeSensitive(exchange, inputs);
  verdicts.set(exchange, { key, sensitive });
  return sensitive;
}
