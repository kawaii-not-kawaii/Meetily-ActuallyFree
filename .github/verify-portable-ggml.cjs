const { existsSync, readdirSync, readFileSync } = require('node:fs');
const { join, resolve } = require('node:path');

// Each GGML consumer builds its own ggml through a -sys crate's CMake build.
// whisper: the app's Whisper engine (whisper-rs-sys).
// llama: the llama-helper summary sidecar (llama-cpp-sys-2).
const COMPONENTS = {
  whisper: { label: 'Whisper', cratePrefix: 'whisper-rs-sys-' },
  llama: { label: 'llama-helper', cratePrefix: 'llama-cpp-sys-2-' },
};

// Run before bundling so unverified native code cannot become a release asset.
try {
  const [profileDirectory, componentName = 'whisper', ...extra] = process.argv.slice(2);
  const component = COMPONENTS[componentName];
  if (!profileDirectory || !component || extra.length > 0) {
    throw new Error(
      `Usage: node verify-portable-ggml.cjs <target/profile directory> [${Object.keys(COMPONENTS).join('|')}]`,
    );
  }
  const buildDirectory = resolve(profileDirectory, 'build');
  const caches = existsSync(buildDirectory)
    ? readdirSync(buildDirectory, { withFileTypes: true })
      .filter((entry) => entry.isDirectory() && entry.name.startsWith(component.cratePrefix))
      .map((entry) => join(buildDirectory, entry.name, 'out/build/CMakeCache.txt'))
      .filter(existsSync)
    : [];
  if (caches.length === 0) {
    throw new Error(`No ${component.label} CMake cache found in ${buildDirectory}`);
  }

  for (const cache of caches) {
    const contents = readFileSync(cache, 'utf8');
    console.log(`Checking ${cache}`);
    for (const line of contents.split(/\r?\n/)) {
      if (/^(GGML_(NATIVE|AVX[^:]*|FMA|F16C)|CMAKE_C(?:XX)?_FLAGS[^:]*):/.test(line)) {
        console.log(line);
      }
    }
    for (const flag of ['GGML_NATIVE', 'GGML_AVX512', 'GGML_AVX512_VBMI', 'GGML_AVX512_VNNI', 'GGML_AVX512_BF16']) {
      if (!new RegExp(`^${flag}:BOOL=OFF\\r?$`, 'm').test(contents)) {
        throw new Error(`${flag} must be explicitly OFF in ${cache}`);
      }
    }
  }
  console.log(`Verified ${component.label} CPU portability before bundling.`);
} catch (error) {
  console.error(error.message);
  process.exitCode = 1;
}
