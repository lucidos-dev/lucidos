// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { act } from 'preact/test-utils';
import { ModelPicker, type ModelPickerInstance } from './modelPicker';
import { request } from './request';
vi.mock('./request', () => ({ request: vi.fn() }));
const instances: ModelPickerInstance[] = [];
afterEach(() => { for (const picker of instances.splice(0)) picker.destroy(); vi.clearAllMocks(); });
const value = { model: 'example', reasoning_effort: null, provider: null };
const registry = { models: [{ id: 'example', label: 'Example', successor: null, on_chat_ladder: true,
  preferred_provider: null, default_effort: null, routes: [{ provider: 'local', id: 'wire', configured: true,
    reasoning_efforts: [], proxy: { name: 'local', protocol: 'openai-chat-completions' } }] }] };
function create() { const picker = ModelPicker.create({ value, onChange: vi.fn() }); instances.push(picker); return picker; }
describe('ModelPicker lifecycle', () => {
  it('loads through the app request path and resolves the actual default without emitting a user change', async () => {
    vi.mocked(request).mockResolvedValue(registry);
    const onChange = vi.fn();
    const picker = ModelPicker.create({ value, onChange }); instances.push(picker);
    await act(async () => { await picker.ready; });
    expect(request).toHaveBeenCalledWith('/models');
    expect(picker.getValue()).toEqual({ ...value, provider: 'local' });
    picker.setValue(value);
    expect(onChange).not.toHaveBeenCalled();
    expect(picker.element.textContent).toContain('Example');
  });
  it('surfaces loading failures and supports explicit refresh', async () => {
    vi.mocked(request).mockRejectedValueOnce(new Error('offline')).mockResolvedValueOnce(registry);
    const picker = create();
    await expect(picker.ready).rejects.toThrow('offline');
    await act(async () => { await picker.refresh(); });
    expect(picker.getValue().provider).toBe('local');
  });
  it('ignores late loads after destroy and refuses further operations', async () => {
    let finish!: (data: unknown) => void;
    vi.mocked(request).mockImplementation(() => new Promise(resolve => { finish = resolve; }));
    const picker = create();
    picker.destroy();
    finish(registry);
    await expect(picker.ready).rejects.toThrow('destroyed');
    expect(picker.element.childNodes.length).toBe(0);
    expect(() => picker.setValue(value)).toThrow('destroyed');
  });
});

describe('ModelPicker interactions and concurrent reads', () => {
  it('commits a keyboard choice and cancels Escape without a callback', async () => {
    vi.mocked(request).mockResolvedValue(registry);
    const onChange = vi.fn();
    const picker = ModelPicker.create({ value, onChange }); instances.push(picker);
    document.body.append(picker.element);
    await act(async () => { await picker.ready; picker.open(); });
    const list = document.querySelector('.control-list')!;
    expect(list).not.toBeNull();
    await act(async () => { list.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true })); });
    expect(onChange).toHaveBeenCalledWith({ ...value, provider: 'local' });
    await act(async () => { picker.open(); });
    await act(async () => { window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape' })); });
    expect(document.querySelector('.dropdown-menu')).toBeNull();
    expect(onChange).toHaveBeenCalledTimes(1);
    picker.element.remove();
  });
  it('ignores an obsolete load and validates setValue against the winning registry', async () => {
    let finish!: (data: unknown) => void;
    vi.mocked(request).mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }))
      .mockResolvedValueOnce(registry);
    const picker = create();
    picker.setValue({ ...value, provider: 'local' });
    await act(async () => { await picker.refresh(); });
    finish({ models: [] });
    await act(async () => { await picker.ready; });
    expect(picker.getValue().provider).toBe('local');
    expect(picker.element.textContent).toContain('Example');
  });
  it('removes only its own panel when another picker remains open', async () => {
    vi.mocked(request).mockResolvedValue(registry);
    const a = create(), b = create();
    await act(async () => { await Promise.all([a.ready, b.ready]); a.open(); b.open(); });
    expect(document.querySelectorAll('.dropdown-menu')).toHaveLength(2);
    await act(async () => { b.destroy(); });
    expect(document.querySelectorAll('.dropdown-menu')).toHaveLength(1);
    await act(async () => { window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape' })); });
    expect(document.querySelectorAll('.dropdown-menu')).toHaveLength(0);
  });
});

describe('ModelPicker focus navigation', () => {
  it('opens from ArrowDown and returns Tab to the field before native traversal', async () => {
    vi.mocked(request).mockResolvedValue(registry);
    const picker = create(); document.body.append(picker.element);
    await act(async () => { await picker.ready; });
    const trigger = picker.element.querySelector('button')!;
    await act(async () => { trigger.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true })); });
    expect(document.querySelector('.control-list')).not.toBeNull();
    await act(async () => { document.querySelector('.control-list')!.dispatchEvent(new KeyboardEvent('keydown', { key: 'Tab', bubbles: true })); });
    expect(document.querySelector('.dropdown-menu')).toBeNull();
    expect(document.activeElement).toBe(trigger);
    picker.element.remove();
  });
});
