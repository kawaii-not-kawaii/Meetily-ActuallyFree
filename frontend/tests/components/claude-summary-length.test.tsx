import React from 'react';
import { act, create, ReactTestRenderer } from 'react-test-renderer';
import { afterEach, beforeEach, expect, mock, test } from 'bun:test';

const limits: Record<string, { maxOutputTokens: number; maxInputTokens: number | null; source: string }> = {
  'claude-sonnet-5-5': { maxOutputTokens: 128000, maxInputTokens: 1000000, source: 'live' },
  'claude-haiku-4-5': { maxOutputTokens: 64000, maxInputTokens: null, source: 'catalog' },
  'claude-next-9': { maxOutputTokens: 8192, maxInputTokens: null, source: 'unknown' },
};
const calls: any[] = [];
let held: Record<string, (value: unknown) => void> = {};
let hold = false;
mock.module('@tauri-apps/api/core', () => ({ invoke: async (command: string, args: any) => {
  if (command !== 'get_claude_model_limit') throw new Error(command);
  calls.push(args);
  if (hold) return new Promise((resolve) => { held[args.model] = resolve; });
  return limits[args.model];
} }));

const { ClaudeSummaryLengthField, useClaudeModelLimit } = await import('../../src/components/ClaudeSummaryLengthField');

let root: ReactTestRenderer;
function Harness({ model, value = '', apiKey = 'sk-test' }: { model: string; value?: string; apiKey?: string }) {
  const limit = useClaudeModelLimit(model, apiKey, true);
  return <ClaudeSummaryLengthField limit={limit} value={value} onChange={() => {}} />;
}
const render = async (element: React.ReactElement) => {
  await act(async () => { if (root) root.update(element); else root = create(element); });
};
const input = () => root.root.findByType('input').props;
const text = () => root.root.findByProps({ id: 'summary-max-tokens-help' }).children.join('');

beforeEach(() => { calls.length = 0; held = {}; hold = false; });
afterEach(async () => { await act(async () => root?.unmount()); root = undefined as any; });

test('an empty field shows the model maximum as the default and maximum', async () => {
  await render(<Harness model="claude-sonnet-5-5" />);
  expect(calls).toEqual([{ model: 'claude-sonnet-5-5', apiKey: 'sk-test' }]);
  expect(input().placeholder).toBe('128000');
  expect(input().max).toBe(128000);
  expect(input().value).toBe('');
  expect(text()).toContain("this model's maximum of 128,000 output tokens");
  expect(text()).not.toContain('unknown');
});

test('a user cap is kept while the placeholder follows the selected model', async () => {
  await render(<Harness model="claude-sonnet-5-5" value="32000" />);
  await render(<Harness model="claude-haiku-4-5" value="32000" />);
  expect(input().value).toBe('32000');
  expect(input().max).toBe(64000);
  expect(input().placeholder).toBe('64000');
});

test('an unknown model shows 8192 and says the limit is unknown', async () => {
  await render(<Harness model="claude-next-9" />);
  expect(input().placeholder).toBe('8192');
  expect(text()).toContain('limit for this model is unknown');
  expect(text()).toContain('8,192');
});

test('a late answer for the previous model is never shown for the new one', async () => {
  hold = true;
  await render(<Harness model="claude-sonnet-5-5" />);
  await render(<Harness model="claude-haiku-4-5" />);
  await act(async () => { held['claude-sonnet-5-5'](limits['claude-sonnet-5-5']); });
  expect(input().max).toBeUndefined();
  expect(input().placeholder).toBe('Model maximum');
  await act(async () => { held['claude-haiku-4-5'](limits['claude-haiku-4-5']); });
  expect(input().max).toBe(64000);
});
