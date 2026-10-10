/**
 * The hand-written TS copies of the coding-agent change state agree with the
 * types generated from the engine (ADR 0400): the app's own union, and the
 * SDK's standalone one. The pins are types, so `tsc` is what fails on a drift.
 */
import { describe, it, expect } from 'vitest';
import type { CodingAgentChangeState } from './threads';
import type { ChangeStateKind } from '../generated/thread-lifecycle';
import type { UnproposedReason } from '../generated/thread-event-wire';
import type {
  CodingAgentChangeState as SdkChangeState,
  UnproposedReason as SdkUnproposedReason,
} from '../../../../packages/lucidos-sdk/src/threads';

type Same<A, B> = [A] extends [B] ? ([B] extends [A] ? true : false) : false;

const pins: [
  Same<CodingAgentChangeState['kind'], ChangeStateKind>,
  Same<Extract<CodingAgentChangeState, { kind: 'unproposed' }>['reason'], UnproposedReason | null>,
  Same<SdkUnproposedReason, UnproposedReason>,
  Same<SdkChangeState, CodingAgentChangeState>,
] = [true, true, true, true];

describe('coding-agent change state wire types', () => {
  it('match the engine-generated kind and reason, in the app and the SDK', () => {
    expect(pins).toEqual([true, true, true, true]);
  });
});
