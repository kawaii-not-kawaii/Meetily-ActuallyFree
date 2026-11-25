import catalog from './claude-output-limits.json';

/** Suggested Claude models shown when the live model list is unavailable. */
export const CLAUDE_FALLBACK_MODELS: readonly { id: string; name: string }[] = catalog.fallbackModels;

/** A model's limits as resolved by Rust (`get_claude_model_limit`): live
 * Models API data, else the bundled catalog, else a conservative fallback. */
export interface ClaudeLimit {
  maxOutputTokens: number;
  maxInputTokens: number | null;
  source: 'live' | 'catalog' | 'unknown';
}

/** The user's optional summary cap; null means the model maximum. `limit` is
 * null while it loads, and the backend still checks the cap on save. */
export function parseClaudeOutputTokens(value: string, limit: ClaudeLimit | null): number | null {
  const text = value.trim();
  if (!text) return null;
  const maximum = limit?.maxOutputTokens ?? Number.MAX_SAFE_INTEGER;
  if (!/^\d+$/.test(text) || !Number.isSafeInteger(Number(text)) || Number(text) < 1 || Number(text) > maximum) {
    const range = limit ? `between 1 and ${maximum.toLocaleString('en-US')}` : 'of at least 1';
    throw new Error(`Maximum summary length must be a whole number ${range}, or left empty.`);
  }
  return Number(text);
}
