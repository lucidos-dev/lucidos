// @vitest-environment jsdom
// A `repo:` link can name an image that is not in the repository at that ref.
// The preview must say so, rather than draw a broken image.
import { afterEach, beforeEach, expect, it } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { RepoFileContent } from '../RepoFilePreview';

let host: HTMLDivElement;

beforeEach(() => {
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  render(null, host);
  host.remove();
});

function show(path: string) {
  act(() => { render(<RepoFileContent repoId="repo-1" path={path} gitRef={null} />, host); });
}

it('replaces an image that fails to load with a load error naming the file', () => {
  show('docs/missing.png');
  const img = host.querySelector('img.preview-image')!;
  expect(img).not.toBeNull();
  act(() => { img.dispatchEvent(new Event('error')); });
  expect(host.querySelector('img')).toBeNull();
  expect(host.querySelector('.error-text')!.textContent)
    .toBe('Failed to load image: docs/missing.png could not be read from the repository');
});

it('starts the next file without the last one\'s failure', () => {
  show('docs/missing.png');
  act(() => { host.querySelector('img')!.dispatchEvent(new Event('error')); });
  show('docs/present.png');
  expect(host.querySelector('img.preview-image')).not.toBeNull();
  expect(host.querySelector('.error-text')).toBeNull();
});
