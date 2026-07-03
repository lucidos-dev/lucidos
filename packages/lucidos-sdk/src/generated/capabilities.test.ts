import { describe, it, expect } from 'vitest';
import { CAPABILITIES } from './capabilities';
import { notifications } from '../notifications';
import { preferences } from '../preferences';
import { triggers } from '../triggers';
import { apps } from '../apps';

/** The hand-written SDK facade is NOT generated (it carries rich JSDoc + typed
 *  return shapes the codegen can't express), but it must stay in parity with the
 *  capability manifest. `capabilities.ts` is generated from the manifest
 *  (`capability_manifest::DOMAINS`); this test fails if a domain declared
 *  `sdk = true` gains an operation the SDK facade doesn't expose. Add the
 *  namespace to `FACADES` when a new `sdk = true` domain lands. */
const FACADES: Record<string, Record<string, unknown>> = {
  notifications: notifications as unknown as Record<string, unknown>,
  preferences: preferences as unknown as Record<string, unknown>,
  triggers: triggers as unknown as Record<string, unknown>,
  apps: apps as unknown as Record<string, unknown>,
};

describe('SDK parity with the capability manifest', () => {
  for (const domain of CAPABILITIES) {
    it(`${domain.name} facade exposes every manifest operation`, () => {
      const facade = FACADES[domain.name];
      expect(
        facade,
        `No SDK facade registered for manifest domain '${domain.name}'. ` +
          `Add it to FACADES in capabilities.test.ts (and implement the methods).`,
      ).toBeDefined();
      for (const op of domain.ops) {
        expect(
          typeof facade[op.sdkName],
          `lucidos.${domain.name}.${op.sdkName}() is in the manifest but missing ` +
            `from the SDK facade (action '${op.action}').`,
        ).toBe('function');
      }
    });
  }
});
