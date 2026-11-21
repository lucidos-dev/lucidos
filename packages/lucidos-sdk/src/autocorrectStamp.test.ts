// @vitest-environment jsdom
/**
 * The Autocorrect switch inside an app frame (ADR 0262). The host's stamp never
 * reaches this document. The SDK's stamp is what stands between an app's notes
 * field and the dead tap on the Save below it.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import {
  _resetAutocorrectStampForTesting, applyAutocorrectPreference, initialAutocorrect,
  installAutocorrectStamp, setAppAutocorrect,
} from './autocorrectStamp';

const IPHONE = 'Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15';
const DESKTOP = 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 Chrome/140';

function stubAgent(userAgent: string): void {
  vi.stubGlobal('navigator', { userAgent, platform: 'x', maxTouchPoints: 0 });
}

/** Let the MutationObserver deliver, as it does before any user input. */
const observed = () => new Promise<void>((resolve) => queueMicrotask(resolve));

function mount(html: string): HTMLElement {
  const host = document.createElement('div');
  host.innerHTML = html;
  document.body.appendChild(host);
  return host;
}

beforeEach(() => {
  document.body.innerHTML = '';
  delete (globalThis as { __lucidosPrefs?: unknown }).__lucidosPrefs;
  stubAgent(DESKTOP);
});

afterEach(() => {
  _resetAutocorrectStampForTesting();
  vi.unstubAllGlobals();
});

describe('with the switch off', () => {
  it('stamps a field that was there at install', () => {
    const host = mount('<textarea></textarea>');
    installAutocorrectStamp(false);
    expect(host.querySelector('textarea')!.getAttribute('autocorrect')).toBe('off');
  });

  it('stamps a field that mounts later, nested or not, before anyone can focus it', async () => {
    installAutocorrectStamp(false);
    const host = mount('<form><label><input type="text"></label></form><textarea></textarea>');
    await observed();
    expect(host.querySelector('input')!.getAttribute('autocorrect')).toBe('off');
    expect(host.querySelector('textarea')!.getAttribute('autocorrect')).toBe('off');
  });

  it('stamps a field focused in the task that mounted it, before the observer runs', () => {
    installAutocorrectStamp(false);
    const input = document.createElement('input');
    document.body.appendChild(input);
    let atFocus: string | null = null;
    input.addEventListener('focus', () => { atFocus = input.getAttribute('autocorrect'); });
    input.focus();
    expect(atFocus).toBe('off');
  });

  it('writes autocorrect only, leaving capitals and spell-check to the app', () => {
    const host = mount('<input type="text">');
    installAutocorrectStamp(false);
    const input = host.querySelector('input')!;
    expect(input.hasAttribute('autocapitalize')).toBe(false);
    expect(input.hasAttribute('spellcheck')).toBe(false);
  });

  it('keeps an autocorrect the app author set', () => {
    const host = mount('<textarea autocorrect="on"></textarea>');
    installAutocorrectStamp(false);
    expect(host.querySelector('textarea')!.getAttribute('autocorrect')).toBe('on');
  });

  it('leaves inputs that take no typing alone', () => {
    const host = mount('<input type="checkbox"><input type="submit"><button>Save</button>');
    installAutocorrectStamp(false);
    for (const el of host.querySelectorAll('input, button')) {
      expect(el.hasAttribute('autocorrect')).toBe(false);
    }
  });
});

describe('a flip of the switch', () => {
  it('stamps every mounted field when it turns off', () => {
    const host = mount('<textarea></textarea>');
    installAutocorrectStamp(true);
    expect(host.querySelector('textarea')!.hasAttribute('autocorrect')).toBe(false);
    setAppAutocorrect(false);
    expect(host.querySelector('textarea')!.getAttribute('autocorrect')).toBe('off');
  });

  it('takes back only its own stamps when it turns on', () => {
    const host = mount('<textarea id="ours"></textarea><textarea id="author" autocorrect="off"></textarea>');
    installAutocorrectStamp(false);
    setAppAutocorrect(true);
    expect(host.querySelector('#ours')!.hasAttribute('autocorrect')).toBe(false);
    expect(host.querySelector('#author')!.getAttribute('autocorrect')).toBe('off');
  });

  it('leaves a value the app changed after the stamp', () => {
    const host = mount('<textarea></textarea>');
    installAutocorrectStamp(false);
    const field = host.querySelector('textarea')!;
    field.setAttribute('autocorrect', 'on');
    setAppAutocorrect(true);
    expect(field.getAttribute('autocorrect')).toBe('on');
  });

  it('stamps nothing before install, so the host shell importing the SDK is untouched', () => {
    const host = mount('<textarea></textarea>');
    setAppAutocorrect(false);
    expect(host.querySelector('textarea')!.hasAttribute('autocorrect')).toBe(false);
  });

  it('follows a fetched preferences map', () => {
    const host = mount('<textarea></textarea>');
    installAutocorrectStamp(true);
    applyAutocorrectPreference({ autocorrect: 'false' });
    expect(host.querySelector('textarea')!.getAttribute('autocorrect')).toBe('off');
  });

  it('reads an unset switch in a fetched map as on, an iPhone included', () => {
    stubAgent(IPHONE);
    const host = mount('<textarea></textarea>');
    installAutocorrectStamp(false);
    applyAutocorrectPreference({});
    expect(host.querySelector('textarea')!.hasAttribute('autocorrect')).toBe(false);
  });
});

describe('initialAutocorrect', () => {
  it('is on when nothing is stored, on an iPhone and on a desktop alike', () => {
    expect(initialAutocorrect()).toBe(true);
    stubAgent(IPHONE);
    expect(initialAutocorrect()).toBe(true);
  });

  it('takes the first-paint seed over the default', () => {
    stubAgent(IPHONE);
    (globalThis as { __lucidosPrefs?: unknown }).__lucidosPrefs = { autocorrect: 'false' };
    expect(initialAutocorrect()).toBe(false);
  });
});
