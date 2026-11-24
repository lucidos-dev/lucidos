import { compareSortKeys, happenedAt, isUningestedMessage, isUnsentExchange } from './exchange-grouping';
import { isTurnlessBoundary } from './thread-event-types';
import type { Exchange } from './exchange';
import type { StoredEvent } from './thread-event-types';



export interface RenderRow {
  exchangeIndex: number;
  kind: 'boundary' | 'step';
  type: string;
  seq: number;
  micros: number | null;
}

export interface RenderOrderViolation {
  above: RenderRow;
  below: RenderRow;
  summary: string;
}

function rowOf(
  exchangeIndex: number,
  kind: 'boundary' | 'step',
  seq: number,
  event: StoredEvent,
): RenderRow {
  return { exchangeIndex, kind, type: event.type, seq, micros: happenedAt(event) };
}

function engagedKey(exchange: Exchange, index: number): RenderRow | null {
  if (isUningestedMessage(exchange)) return null;
  if (isTurnlessBoundary(exchange.userEvent)) return null;
  if (isUnsentExchange(exchange)) return null;
  const pickedUp = ingestionStep(exchange);
  if (pickedUp) return rowOf(index, 'step', pickedUp.seq, pickedUp.event);
  return rowOf(index, 'boundary', exchange.userSeq, exchange.userEvent);
}

function ingestionStep(exchange: Exchange) {
  const ownId = exchange.userEvent._eventId;
  if (exchange.userEvent.type !== 'MessageReceived' || !ownId) return undefined;
  return exchange.steps.find(step =>
    step.event.type === 'PromptInjected'
    && (step.event as { injected_message_id?: string }).injected_message_id === ownId,
  );
}

function resultRejoinsItsCall(exchange: Exchange, event: StoredEvent): boolean {
  if (event.type === 'CodingAgentToolResult') {
    const id = (event as { tool_use_id?: string }).tool_use_id;
    return !!id && exchange.steps.some(step =>
      step.event.type === 'CodingAgentToolCalled'
      && (step.event as { tool_use_id?: string }).tool_use_id === id,
    );
  }
  if (event.type !== 'ToolResult') return false;
  const calledId = (event as { tool_called_event_id?: string }).tool_called_event_id;
  return !!calledId && exchange.steps.some(step =>
    step.event.type === 'ToolCalled' && step.event._eventId === calledId,
  );
}

export function renderOrderViolations(exchanges: Exchange[]): RenderOrderViolation[] {
  const violations: RenderOrderViolation[] = [];
  const walls: (RenderRow | null)[] = exchanges.map(engagedKey);
  for (let i = 0; i < exchanges.length; i++) {
    const exchange = exchanges[i];
    let nextEngaged = i + 1;
    while (nextEngaged < exchanges.length && !walls[nextEngaged]) nextEngaged += 1;
    const rows = [
      rowOf(i, 'boundary', exchange.userSeq, exchange.userEvent),
      ...exchange.steps.map(step => rowOf(i, 'step', step.seq, step.event)),
    ];
    for (let below = i + 1; below < exchanges.length; below++) {
      const wall = walls[below];
      if (!wall) continue;
      for (const row of rows) {
        if (compareSortKeys(row.micros, row.seq, wall.micros, wall.seq) <= 0) continue;
        const step = below === nextEngaged && row.kind === 'step'
          ? exchange.steps.find(s => s.seq === row.seq)
          : undefined;
        if (step && resultRejoinsItsCall(exchange, step.event)) continue;
        violations.push({
          above: row,
          below: wall,
          summary: `${row.kind} ${row.type} (seq ${row.seq}) renders in exchange ${i}, `
            + `above exchange ${below}'s ${wall.kind} ${wall.type} (seq ${wall.seq}), `
            + 'which the clock puts first',
        });
        break;
      }
    }
  }
  return violations;
}
