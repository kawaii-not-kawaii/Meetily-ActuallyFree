import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import type { ClaudeLimit } from '@/lib/claude-output-tokens';

/** The selected Claude model's limits, or null until known for that model. */
export function useClaudeModelLimit(model: string, apiKey: string | null | undefined, enabled: boolean): ClaudeLimit | null {
  const [resolved, setResolved] = useState<{ model: string; limit: ClaudeLimit } | null>(null);

  useEffect(() => {
    if (!enabled || !model) return;
    let stale = false;
    invoke<ClaudeLimit>('get_claude_model_limit', { model, apiKey: apiKey?.trim() || null })
      .then((limit) => { if (!stale) setResolved({ model, limit }); })
      .catch((error) => console.error('Error loading Claude model limit:', error));
    return () => { stale = true; };
  }, [model, apiKey, enabled]);

  // Tagged with its model so a previous model's limit is never shown.
  return enabled && resolved?.model === model ? resolved.limit : null;
}

function helpText(limit: ClaudeLimit | null): string {
  if (!limit) return "Leave empty to use the model's maximum output.";
  const maximum = limit.maxOutputTokens.toLocaleString('en-US');
  const range = limit.source === 'unknown'
    ? `The output limit for this model is unknown, so the app uses ${maximum} output tokens. Leave empty or set a lower cap.`
    : `Leave empty to use this model's maximum of ${maximum} output tokens, or set a lower cap.`;
  return `${range} A higher limit allows longer summaries and may increase API usage. Incomplete responses are reported as errors.`;
}

export function ClaudeSummaryLengthField({ limit, value, onChange }: {
  limit: ClaudeLimit | null;
  value: string;
  onChange: (value: string) => void;
}) {
  return (
    <div>
      <Label htmlFor="summary-max-tokens">Maximum summary length (optional)</Label>
      <Input
        id="summary-max-tokens"
        type="number"
        min="1"
        max={limit?.maxOutputTokens}
        step="1"
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder={limit ? String(limit.maxOutputTokens) : 'Model maximum'}
        aria-describedby="summary-max-tokens-help"
        className="mt-1"
      />
      <p id="summary-max-tokens-help" className="text-xs text-muted-foreground mt-1">
        {helpText(limit)}
      </p>
    </div>
  );
}
