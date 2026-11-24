import { describe, expect, test } from 'bun:test';
import { CLAUDE_FALLBACK_MODELS, claudeCatalogLimit, claudeOutputBudget, parseClaudeOutputTokens } from '../../src/lib/claude-output-tokens';

describe('Claude output budget', () => {
  test('uses an application default, with legacy limits and conservative unknown-model fallback', () => {
    expect(claudeOutputBudget('claude-sonnet-4-5-20250929')).toEqual({ maximum: 64000, defaultTokens: 8192 });
    expect(claudeOutputBudget('claude-3-sonnet-20240229').defaultTokens).toBe(4096);
    expect(claudeOutputBudget('claude-3-5-sonnet-latest').defaultTokens).toBe(8192);
    expect(claudeOutputBudget('claude-unknown').maximum).toBe(8192);
  });
  test('distinguishes blank from invalid input instead of truncating parseInt input', () => {
    const model = 'claude-sonnet-4-5';
    expect(parseClaudeOutputTokens('  ', model)).toBeNull();
    expect(parseClaudeOutputTokens('32000', model)).toBe(32000);
    for (const value of ['0', '-1', '1.5', '1e3', '12abc', '64001', '4294967296', 'NaN']) {
      expect(() => parseClaudeOutputTokens(value, model)).toThrow();
    }
    expect(() => parseClaudeOutputTokens('8192', 'claude-3-opus')).toThrow();
  });
});

describe('Claude model catalog', () => {
  test.each([
    ['claude-3-opus-20240229', 4096], ['claude-3-7-sonnet-latest', 64000],
    ['claude-sonnet-4-20250514', 64000], ['claude-sonnet-4-5-20250929', 64000], ['claude-sonnet-4-6', 128000],
    ['claude-sonnet-5', 128000], ['claude-sonnet-5-5', 128000],
    ['claude-haiku-4-5-20251001', 64000], ['claude-haiku-5', 128000], ['claude-haiku-5-5', 128000],
    ['claude-opus-4-1-20250805', 32000], ['claude-opus-4-20250514', 32000], ['claude-opus-4-5-20251101', 64000],
    ['claude-opus-4-6', 128000], ['claude-opus-4-7', 128000], ['claude-opus-4-8', 128000],
    ['claude-opus-5', 128000], ['claude-opus-5-5', 128000],
    ['claude-fable-5-2', 128000], ['claude-mythos-5-2', 128000], ['claude-unknown', 8192],
  ])('%s resolves to %i (longest prefix wins)', (model, limit) => {
    expect(claudeCatalogLimit(model)).toBe(limit);
  });
  test('Haiku 5.5 and Mythos no longer resolve to 8192', () => {
    expect(claudeCatalogLimit('claude-haiku-5-5')).toBe(128000);
    expect(claudeCatalogLimit('claude-mythos-5-2')).toBe(128000);
  });
  test('fallback list is the unified list without retired models', () => {
    expect(CLAUDE_FALLBACK_MODELS.map(m => m.id)).toEqual(['claude-sonnet-5-5', 'claude-opus-5-5', 'claude-haiku-5-5', 'claude-haiku-4-5']);
  });
});
