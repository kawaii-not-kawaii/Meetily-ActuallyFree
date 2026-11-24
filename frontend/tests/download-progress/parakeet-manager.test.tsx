import React from 'react';
import { act, create, ReactTestRenderer } from 'react-test-renderer';
import { afterEach, beforeEach, expect, mock, test } from 'bun:test';

// The native ParakeetEngine serializes ModelStatus::Downloading { progress } as
// { Downloading: { progress: N } }; the manager must read the nested value both
// when it restores a native download and when it applies progress events.
const MODEL = 'parakeet-tdt-0.6b-v3-int8';
const listeners = new Map<string, (event: { payload: any }) => void>();
let nativeStatus: unknown;
mock.module('sonner', () => ({ toast: { success: () => {}, error: () => {}, info: () => {} } }));
mock.module('framer-motion', () => ({
  motion: new Proxy({}, { get: (_target, tag: string) => ({ children, initial, animate, exit, transition, ...props }: any) => React.createElement(tag, props, children) }),
  AnimatePresence: ({ children }: any) => children,
}));
mock.module('@tauri-apps/api/event', () => ({ listen: async (name: string, callback: any) => {
  listeners.set(name, callback); return () => listeners.delete(name);
} }));
mock.module('@tauri-apps/api/core', () => ({ invoke: async (command: string) => {
  if (command === 'parakeet_init') return;
  if (command === 'parakeet_get_available_models') {
    return [{ name: MODEL, path: '', size_mb: 670, accuracy: 'High', speed: 'Ultra Fast', status: nativeStatus, description: '', quantization: 'Int8' }];
  }
  throw new Error(command);
} }));
const { ParakeetModelManager } = await import('../../src/components/ParakeetModelManager');

let root: ReactTestRenderer;
const text = () => {
  const out: string[] = [];
  const walk = (node: any): void => {
    if (node === null || node === undefined) return;
    if (typeof node === 'string') { out.push(node); return; }
    if (Array.isArray(node)) { node.forEach(walk); return; }
    walk(node.children);
  };
  walk(root.toJSON());
  return out.join('');
};

beforeEach(() => {
  Object.defineProperty(globalThis, 'localStorage', { configurable: true, value: { getItem: () => null, setItem: () => {} } });
});
afterEach(async () => { await act(async () => root?.unmount()); listeners.clear(); });

test('restored native Parakeet download shows its nested progress', async () => {
  nativeStatus = { Downloading: { progress: 62 } };
  await act(async () => { root = create(<ParakeetModelManager />); });
  expect(text()).toContain('62%');
  expect(text()).not.toContain('NaN');
});

test('Parakeet progress events update the nested progress', async () => {
  nativeStatus = 'Missing';
  await act(async () => { root = create(<ParakeetModelManager />); });
  await act(async () => listeners.get('parakeet-model-download-progress')!({ payload: { modelName: MODEL, progress: 41 } }));
  expect(text()).toContain('41%');
});
