// @vitest-environment jsdom
/** A Settings model field ends on a link to the ladder that decides its list.
 *  The link navigates and never picks a model. */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { ModelSelectionPicker, type PickerLink } from '../ModelSelectionPicker';
import { useModelSelection } from '../../../hooks/useModelSelection';
import { LADDER_LINK_LABEL, LADDER_LINK_ROW } from '../../chat/rungRows';
import type { ModelChoice } from '../../../store/modelSelection';

let host: HTMLDivElement;

beforeEach(() => {
  vi.spyOn(Element.prototype, 'scrollIntoView').mockImplementation(() => {});
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  render(null, host);
  host.remove();
  vi.restoreAllMocks();
});

const MODELS: ModelChoice[] = [
  { value: 'opus', label: 'Opus', reasoningEfforts: [] },
  { value: 'haiku', label: 'Haiku', reasoningEfforts: [] },
];

function Picker({ link, onPick }: { link?: PickerLink; onPick: (encoded: string) => void }) {
  const selection = useModelSelection({
    models: MODELS, vocabulary: [], model: 'opus', effort: null, onChange: () => {},
  });
  return <ModelSelectionPicker label="Chat model" selection={selection} link={link} onPick={onPick} />;
}

const labels = () => [...host.querySelectorAll('.control-option-label')].map((el) => el.textContent?.replace('✓', ''));

function rowNamed(label: string): HTMLElement {
  const row = [...host.querySelectorAll<HTMLElement>('.control-option')].find((el) => el.textContent?.includes(label));
  if (!row) throw new Error(`no row ${label}`);
  return row;
}

describe('the model step link', () => {
  it('is the last row, after every model', () => {
    act(() => {
      render(<Picker link={{ option: LADDER_LINK_ROW, onChoose: () => {} }} onPick={() => {}} />, host);
    });
    expect(labels()).toEqual(['Opus', 'Haiku', LADDER_LINK_LABEL]);
  });

  it('keeps the field label as the heading over the models', () => {
    act(() => {
      render(<Picker link={{ option: LADDER_LINK_ROW, onChoose: () => {} }} onPick={() => {}} />, host);
    });
    const headings = [...host.querySelectorAll('.control-section-label')].map((el) => el.textContent);
    expect(headings).toEqual(['Chat model', LADDER_LINK_ROW.section]);
  });

  it('navigates on a tap and picks no model', () => {
    const onChoose = vi.fn();
    const onPick = vi.fn();
    act(() => {
      render(<Picker link={{ option: LADDER_LINK_ROW, onChoose }} onPick={onPick} />, host);
    });
    act(() => { rowNamed(LADDER_LINK_LABEL).click(); });
    expect(onChoose).toHaveBeenCalledOnce();
    expect(onPick).not.toHaveBeenCalled();
  });

  it('is absent where the host passes none', () => {
    act(() => { render(<Picker onPick={() => {}} />, host); });
    expect(labels()).toEqual(['Opus', 'Haiku']);
  });
});

it('shows no matches while keeping the ladder link available', () => {
  act(() => render(<Picker link={{ option: LADDER_LINK_ROW, onChoose: () => {} }} onPick={() => {}} />, host));
  act(() => {
    host.querySelector('.control-list')!.dispatchEvent(new KeyboardEvent('keydown', { key: 'z', bubbles: true, cancelable: true }));
  });
  expect(host.textContent).toContain('No matches');
  expect(labels()).toEqual([LADDER_LINK_LABEL]);
});
