/** The unified effort ladder, ascending, in the Lucidos Agent's vocabulary. */
export const REASONING_LEVELS = [
  { value: 'none', label: 'Off' },
  { value: 'low', label: 'Low' },
  { value: 'medium', label: 'Med' },
  { value: 'high', label: 'High' },
  { value: 'xhigh', label: 'X-High' },
  { value: 'max', label: 'Max' },
];

/** The REASONING_LEVELS a model supports, by the engine's own answer.
 *
 *  `supported` is the model's `reasoning_efforts` from the `/models` registry
 *  (`llm::reasoning::supported_efforts`), the same set `RoutingProvider`
 *  clamps a request onto. Get it through `lucidosTiers` in
 *  `store/modelSelection.ts`. With no answer, every level is offered, and the
 *  engine snaps the request onto what the backend accepts. That covers a
 *  registry still loading and an id with no row. The client keeps no copy of the per-family
 *  rules, so it cannot offer a tier the engine has stopped honouring
 *  (ADR 0368). */
export function availableReasoningLevels(
  supported?: readonly string[],
): typeof REASONING_LEVELS {
  const offered = supported ? REASONING_LEVELS.filter(l => supported.includes(l.value)) : [];
  // An empty result would render an empty dropdown, so a registry row that
  // declares nothing we recognise offers the whole ladder instead.
  return offered.length > 0 ? offered : REASONING_LEVELS;
}
