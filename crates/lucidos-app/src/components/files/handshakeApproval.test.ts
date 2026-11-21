import { describe, it, expect } from 'vitest';
import { handshakeWarningFor, type HandshakeScriptState } from './handshakeApproval';

const scripts: HandshakeScriptState[] = [
  { path: 'data/scripts/auth/comfort-cloud.py', exists: true, approved: false },
  { path: 'data/scripts/auth/firebase.py', exists: true, approved: true },
  { path: 'data/scripts/auth/gone.py', exists: false, approved: false },
];

describe('handshakeWarningFor', () => {
  it('warns about a configured script whose content is not approved', () => {
    expect(handshakeWarningFor('scripts/auth/comfort-cloud.py', scripts))
      .toBe('data/scripts/auth/comfort-cloud.py');
  });

  it('accepts the workspace-relative spelling too', () => {
    // The Files panel passes a `data/`-relative path; a caller holding the
    // API's own spelling must not be told the file is unknown.
    expect(handshakeWarningFor('data/scripts/auth/comfort-cloud.py', scripts))
      .toBe('data/scripts/auth/comfort-cloud.py');
  });

  it('says nothing about an approved script', () => {
    expect(handshakeWarningFor('scripts/auth/firebase.py', scripts)).toBeNull();
  });

  it('says nothing about a script that is not on disk', () => {
    // apis.json names it and the file is missing. That is a 404 from the
    // runner with its own message, not an approval problem.
    expect(handshakeWarningFor('scripts/auth/gone.py', scripts)).toBeNull();
  });

  it('says nothing about an ordinary file', () => {
    for (const path of [
      'artifacts/notes.md',
      'config/apis.json',
      // Under scripts/, but no apis.json entry names it, so nothing runs it.
      'scripts/helpers/tidy.py',
    ]) {
      expect(handshakeWarningFor(path, scripts), path).toBeNull();
    }
  });

  it('says nothing while the list has not loaded', () => {
    expect(handshakeWarningFor('scripts/auth/comfort-cloud.py', [])).toBeNull();
  });
});
