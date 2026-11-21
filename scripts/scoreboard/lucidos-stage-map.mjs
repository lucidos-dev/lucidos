// Lucidos's own stage map: the engine stages plus Hardening, which `/harden`
// posts as domain events. Another factory writes a map like this one.
import { ENGINE_STAGE_MAP, extendStageMap } from './stage-rules.mjs';

export const LUCIDOS_STAGE_MAP = extendStageMap(ENGINE_STAGE_MAP, {
  stages: [{ name: 'hardening', after: 'implementing', expected: true }],
  rules: [
    { on: 'HardeningStarted', to: 'hardening' },
    { on: 'HardenRunCompleted', from: ['hardening'], when: (d) => d.verdict === 'fail', to: 'implementing' },
    // Code is ready to land only once hardened. Apply hardens the rest first.
    // An unhardened proposal after a hardened one is new work on the change.
    { on: 'ChangeProposed', from: ['ready-to-land'], when: unhardenedCode, to: 'implementing' },
    { on: 'ChangeProposed', when: unhardenedCode, to: null },
    { on: 'ChangeHardened', to: 'ready-to-land' },
  ],
  // A handoff re-emits a round's evidence, possibly under a new iteration.
  reemitted: {
    HardeningStarted: ['head_sha', 'iteration', 'tier'],
    HardenRunCompleted: ['head_sha', 'suites'],
  },
  evidence: {
    HardeningStarted: ['head_sha', 'iteration', 'tier'],
    HardenRunCompleted: ['head_sha', 'iteration', 'tier', 'verdict', 'failed_phase', 'suites'],
    ChangeProposed: ['hardened'],
    LucidosReleased: ['commits'],
    ChangeHardened: ['change_id'],
  },
});

function unhardenedCode(d) {
  return d.content !== 'plan-only' && d.hardened !== true;
}
