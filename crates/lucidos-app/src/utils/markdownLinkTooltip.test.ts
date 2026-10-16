// @vitest-environment jsdom
// jsdom, since fragment resolution searches a real DOM tree for a heading id.

import { describe, it, expect } from 'vitest';
import { markdownLinkTooltip } from './markdownLinkTooltip';
import type { MarkdownDocumentLocation } from './markdownImageSource';

const repoDoc: MarkdownDocumentLocation = { kind: 'repo', repoId: 'repo-1', path: 'docs/databricks.md' };
const workspaceDoc: MarkdownDocumentLocation = { kind: 'workspace', path: 'notes/plan.md' };
const noCandidates: Element[] = [];

function headingCandidate(id: string, text: string): Element {
  const heading = document.createElement('h2');
  heading.id = id;
  heading.textContent = text;
  return heading;
}

describe('markdownLinkTooltip', () => {
  it('resolves a relative repo link the same way a click would, keeping a fragment', () => {
    const tooltip = markdownLinkTooltip(
      '../deploy/databricks/README.md#install', '', repoDoc, noCandidates,
    );
    expect(tooltip).toEqual({ text: 'deploy/databricks/README.md#install' });
  });

  it('resolves a relative workspace link the same way a click would', () => {
    const tooltip = markdownLinkTooltip('../other/README.md', '', workspaceDoc, noCandidates);
    expect(tooltip).toEqual({ text: 'other/README.md' });
  });

  it("shows the target heading's own text for a found in-page anchor", () => {
    const candidates = [headingCandidate(
      'user-content-auth-tier-compatibility',
      'Auth tier compatibility (API key vs subscription / OAuth)',
    )];
    const tooltip = markdownLinkTooltip('#auth-tier-compatibility', '', workspaceDoc, candidates);
    expect(tooltip).toEqual({ text: 'Auth tier compatibility (API key vs subscription / OAuth)' });
  });

  it('matches the "No ... section" toast wording when no heading matches', () => {
    const tooltip = markdownLinkTooltip('#missing-section', '', workspaceDoc, noCandidates);
    expect(tooltip).toEqual({ text: 'No "missing-section" section in notes/plan.md' });
  });

  it('stamps no tooltip for a bare "#" back-to-top link', () => {
    expect(markdownLinkTooltip('#', '', workspaceDoc, noCandidates)).toBeNull();
  });

  it('shows the full URL for an external link, unresolved', () => {
    const tooltip = markdownLinkTooltip('https://example.com/docs', '', workspaceDoc, noCandidates);
    expect(tooltip).toEqual({ text: 'https://example.com/docs' });
  });

  it('marks a link climbing above the repo checkout as not reachable', () => {
    const tooltip = markdownLinkTooltip('../../outside.md', '', repoDoc, noCandidates);
    expect(tooltip).toEqual({ text: '../../outside.md (not reachable)' });
  });

  it('marks a link climbing above the workspace data root as not reachable', () => {
    const tooltip = markdownLinkTooltip('../../outside.md', '', workspaceDoc, noCandidates);
    expect(tooltip).toEqual({ text: '../../outside.md (not reachable)' });
  });

  it("keeps an author's markdown title above the resolved target", () => {
    const tooltip = markdownLinkTooltip('../guide.md', 'The guide', repoDoc, noCandidates);
    expect(tooltip).toEqual({ title: 'The guide', text: 'guide.md' });
  });

  it('stamps no tooltip for an empty href', () => {
    expect(markdownLinkTooltip('', '', workspaceDoc, noCandidates)).toBeNull();
  });
});
