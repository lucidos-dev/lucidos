/**
 * The credential prompt links the page it names.
 *
 * The engine (or an agent) writes a prompt telling the user where to create the
 * token. That URL used to render as flat text, so the one actionable thing on
 * the card had to be selected and copied.
 *
 * The anchor's missing `onClick` is asserted too, and it is not an omission:
 * `onGlobalClick` claims the click at the document and routes it through
 * `openUrl`. A handler that stopped propagation would leave the link dead in
 * the packaged client, where WKWebView drops a bare `_blank` navigation.
 */
import { describe, it, expect } from 'vitest';
import { CredentialInstructions } from '../CredentialModal';
import { findByType, textOf } from '../../layout/__tests__/vnodeWalk';

const CONSOLE_URL = 'https://github.com/settings/personal-access-tokens';

const PROMPT = [
  'GitHub personal access token for the private repo example-org/example-repo.',
  '',
  `Create one at ${CONSOLE_URL} (Fine-grained tokens, then Generate new token).`,
  '',
  'Paste the token below. It stays out of the chat and the event log.',
].join('\n');

describe('the credential prompt links its URL', () => {
  it('renders the URL as an anchor that opens away from the workspace', () => {
    const anchors = findByType(CredentialInstructions({ text: PROMPT }), 'a');
    expect(anchors).toHaveLength(1);
    expect(anchors[0].props.href).toBe(CONSOLE_URL);
    expect(anchors[0].props.target).toBe('_blank');
    expect(anchors[0].props.rel).toBe('noopener noreferrer');
    expect(anchors[0].props.class).toBe('accent-link');
  });

  it('leaves the click to the delegated anchor route', () => {
    const anchors = findByType(CredentialInstructions({ text: PROMPT }), 'a');
    expect(anchors[0].props.onClick).toBeUndefined();
  });

  it('keeps the rest of the prompt as text, newlines included', () => {
    // `.credential-instructions` is `white-space: pre-wrap`, so the blank lines
    // between the paragraphs are the prompt's own layout.
    expect(textOf(CredentialInstructions({ text: PROMPT }))).toBe(PROMPT);
  });
});
