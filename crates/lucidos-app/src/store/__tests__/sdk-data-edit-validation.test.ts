import { describe, it, expect } from 'vitest';
import { lucidos } from '@lucidos/sdk';

describe('SDK runtime input validation', () => {
  describe('lucidos.data.edit()', () => {
    it('throws TypeError when operations is a string', () => {
      expect(() => {
        (lucidos.data.edit as Function)('artifacts/test.json', 'some.path', 'new value');
      }).toThrow(TypeError);
    });

    it('throws TypeError when operations is undefined', () => {
      expect(() => {
        (lucidos.data.edit as Function)('artifacts/test.json');
      }).toThrow(TypeError);
    });

    it('throws TypeError when operations is an object (not array)', () => {
      expect(() => {
        (lucidos.data.edit as Function)('artifacts/test.json', { json_path: 'x', json_value: 1 });
      }).toThrow(TypeError);
    });

    it('error message mentions "array"', () => {
      expect(() => {
        (lucidos.data.edit as Function)('artifacts/test.json', 'some.path');
      }).toThrow(/array/i);
    });
  });

  describe('lucidos.events.emit()', () => {
    it('throws TypeError when payload is a string', () => {
      expect(() => {
        (lucidos.events.emit as Function)('MyEvent', 'not an object');
      }).toThrow(TypeError);
    });

    it('throws TypeError when payload is an array', () => {
      expect(() => {
        (lucidos.events.emit as Function)('MyEvent', ['a', 'b']);
      }).toThrow(TypeError);
    });

    it('throws TypeError when type is not a string', () => {
      expect(() => {
        (lucidos.events.emit as Function)(123, { key: 'value' });
      }).toThrow(TypeError);
    });
  });

  describe('lucidos.triggers.create()', () => {
    it('throws TypeError when trigger is a string', () => {
      expect(() => {
        (lucidos.triggers.create as Function)('not an object');
      }).toThrow(TypeError);
    });

    it('throws TypeError when cron_expressions is a string', () => {
      expect(() => {
        (lucidos.triggers.create as Function)({
          name: 'test',
          run: { type: 'intent', text: 'test', knowhow: [] },
          cron_expressions: '0 0 8 * * *',
        });
      }).toThrow(TypeError);
    });
  });

  describe('lucidos.triggers.update()', () => {
    it('throws TypeError when trigger is a string', () => {
      expect(() => {
        (lucidos.triggers.update as Function)('some-id', 'not an object');
      }).toThrow(TypeError);
    });

    it('throws TypeError when cron_expressions is a string', () => {
      expect(() => {
        (lucidos.triggers.update as Function)('some-id', {
          cron_expressions: '0 0 8 * * *',
        });
      }).toThrow(TypeError);
    });

    it('allows update without cron_expressions', () => {
      expect(() => {
        (lucidos.triggers.update as Function)('some-id', { name: 'new name' }).catch(() => {});
      }).not.toThrow(TypeError);
    });
  });

  describe('lucidos.ui.navigate()', () => {
    it('throws TypeError when params is a string', () => {
      expect(() => {
        (lucidos.ui.navigate as Function)('thread', 'thread-123');
      }).toThrow(TypeError);
    });

    it('throws TypeError when params is an array', () => {
      expect(() => {
        (lucidos.ui.navigate as Function)('thread', ['a']);
      }).toThrow(TypeError);
    });
  });

  describe('lucidos.notifications.list()', () => {
    it('throws TypeError when params is a string', () => {
      expect(() => {
        (lucidos.notifications.list as Function)('unread');
      }).toThrow(TypeError);
    });
  });

  describe('lucidos.preferences.set()', () => {
    it('throws TypeError when key is not a string', () => {
      expect(() => {
        (lucidos.preferences.set as Function)(123, 'value');
      }).toThrow(TypeError);
    });

    it('throws TypeError when value is not a string', () => {
      expect(() => {
        (lucidos.preferences.set as Function)('key', { nested: true });
      }).toThrow(TypeError);
    });
  });
});
