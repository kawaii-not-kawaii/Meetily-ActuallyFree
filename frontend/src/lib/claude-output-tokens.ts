import limits from './claude-output-limits.json';

// Shared with Rust (include_str! of the same JSON). Conservative fallback for
// unknown models; these are app budgets, not a promise to use every token a
// model might support.

/** Suggested Claude models shown when the live model list is unavailable. */
export const CLAUDE_FALLBACK_MODELS: readonly { id: string; name: string }[] = limits.fallbackModels;

function matches(name: string, prefix: string) {
  return name === prefix || name.startsWith(prefix.endsWith('-') ? prefix : `${prefix}-`);
}

/** Catalog output limit before the application clamp. The longest matching
 * prefix wins, so `claude-opus-4-5` is not captured by `claude-opus-4`. */
export function claudeCatalogLimit(model: string): number {
  const name = model.toLowerCase();
  let best: { prefix: string; limit: number } | undefined;
  for (const entry of limits.models) {
    if (matches(name, entry.prefix) && (!best || entry.prefix.length > best.prefix.length)) best = entry;
  }
  return best?.limit ?? limits.fallbackLimit;
}

export function claudeOutputBudget(model: string) {
  const maximum = Math.min(limits.applicationLimit, claudeCatalogLimit(model));
  return { maximum, defaultTokens: Math.min(limits.defaultTokens, maximum) };
}

export function parseClaudeOutputTokens(value: string, model: string): number | null {
  const text = value.trim();
  if (!text) return null;
  const { maximum } = claudeOutputBudget(model);
  if (!/^\d+$/.test(text) || !Number.isSafeInteger(Number(text)) || Number(text) < 1 || Number(text) > maximum) {
    throw new Error(`Maximum summary length must be a whole number between 1 and ${maximum.toLocaleString()}, or left empty.`);
  }
  return Number(text);
}
