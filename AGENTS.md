# Contributor and coding-agent orientation

Meetily - Actually Free is a local-first desktop meeting recorder (Tauri 2): it
captures microphone and system audio, transcribes on-device (Parakeet/Whisper),
labels speakers (Pyannote/WeSpeaker or Nemotron diarization), and keeps notes and
summaries. This repo is our fork, `kawaii-not-kawaii/Meetily-ActuallyFree`
(remote `origin`); `upstream` is `TylerBuza/Meetily-ActuallyFree`. Never open PRs,
issues, or pushes against upstream.

Start with [docs/DEVELOPMENT_MAP.md](docs/DEVELOPMENT_MAP.md). It maps the recording,
transcription, speaker-labeling, model-download, and release paths to their owning
files. Feature qualification notes are linked there; read the relevant note before
changing that subsystem. [ARCHITECTURE.md](ARCHITECTURE.md) covers the overall design.

## Layout

- `frontend/` — Next.js UI (`src/app`, `src/components`, `src/hooks`, …), Bun
  tests in `frontend/tests/`, build/packaging scripts in `frontend/scripts/`.
- `frontend/src-tauri/` — Rust app crate `meetily`: `audio/` (capture, pipeline,
  transcription worker), `diarization/`, `whisper_engine/`, `parakeet_engine/`,
  `database/`, `summary/`, crash diagnostics.
- `llama-helper/` — Rust sidecar for local LLM summaries.
- `vendor/tauri-runtime-wry/` — vendored Tauri runtime patch.
- `docs/` — development map, feature/qualification and release notes;
  `docs/agents/` — agent-skill configuration (issue tracker, labels, domain docs).

## Build and test

From `frontend/` (pnpm; set `PNPM_CONFIG_ENABLE_GLOBAL_VIRTUAL_STORE=false`):

```sh
pnpm install --frozen-lockfile
pnpm dlx bun@1.3.10 scripts/test-isolated.mjs   # every frontend test file, isolated
pnpm run build                                  # Next production build -> frontend/out
pnpm exec tsc --noEmit                          # after the build, never concurrently
pnpm run tauri:dev                              # run the app (auto-selects GPU features)
```

Native tests: `cargo test -p meetily --lib <filter>` with the platform's native
prerequisites and feature flags (see DEVELOPMENT_MAP §6). CI: `.github/workflows/`
(`pr-checks.yml` runs the frontend tests, build, and typecheck).

## Working conventions

- Inspect the current diff before editing; preserve unrelated local work.
- Keep explanations close to non-obvious code: ownership, timestamp units,
  concurrency, lifecycle ordering, and the reason for a constraint. Avoid comments
  that merely restate the next line.
- When changing behavior, update the architecture map or its linked feature note
  with the new data flow, affected interfaces, test coverage, and remaining limits.
- Distinguish implemented, tested, installed locally, and published. A local build
  or installer is not a GitHub release. Check the latest published version before
  choosing a release number; do not increment it for every development rebuild.
- Do not infer identity from a model's speaker number. Preserve microphone/source
  provenance and transcript text/timestamps when changing speaker labels.
- Keep capture nonblocking. Heavy inference belongs on a worker; queues, model
  state, and shutdown behavior need explicit owners and bounded behavior.
- Do not silently substitute another diarization engine after a selected-engine
  failure. Report the failure and retain source labels where appropriate.
- Keep optional-model activation tied to successful completion. Native operations
  can outlive the WebView that invoked them; a JS promise alone is not durable
  ownership of an operation's required native side effects.

## Verification pointers

- Run targeted frontend/native tests for the affected behavior. Mock-heavy Bun
  groups must run as separate invocations; see the map for known groups.
- Serialize Next production builds with native builds/tests embedding
  `frontend/out`. Do not run standalone TypeScript checking during a Next build.
- Windows release packaging is owned by
  `frontend/scripts/build-universal-windows.ps1`; verify the resulting payload with
  `node frontend/scripts/verify-windows-release.mjs`.
- Model/audio-dependent tests are explicitly ignored by default. Report whether
  they actually ran, which type of fixture was used, and what they establish.
- Never commit private recordings, databases, model binaries, crash dumps, local
  signing keys, or diagnostic artifacts. `.build-tools/` contains local helpers,
  not a portable or authoritative public test environment. `.vera/` (the local
  search index) is git-excluded automatically and must not be committed.

## Workflow

Project-scoped skills (Matt Pocock's `mattpocock/skills`, pinned in
`skills-lock.json`) live in `.claude/skills/` (Claude Code) and `.agents/skills/`
(Codex and other agents). For a change:

1. `/grill-me` (or `/grill-with-docs` to also grow `GLOSSARY.md`/ADRs) — align on
   the plan before touching code.
2. `/to-spec` — publish the agreed plan as a GitHub issue on our fork.
3. `/to-tickets` — split the spec into tracer-bullet issues with blocking edges.
4. `/implement` (one ticket) or `/implement-spec` (whole spec), test-first with
   `tdd`; use `diagnosing-bugs` for hard bugs; close out with `code-review`.
5. `pr` — write the PR body, then `gh pr create -R kawaii-not-kawaii/Meetily-ActuallyFree --base main`.

Also: `/triage` for incoming issues, `/wayfinder` for work bigger than one
session, `/improve-codebase-architecture`, `research`, `prototype`, `/handoff`.
`/ask-matt` routes to the right skill. Refresh the skills with
`npx skills@latest update -p`.

## Agent skills

### Issue tracker

GitHub Issues on our fork `kawaii-not-kawaii/Meetily-ActuallyFree`; always pass
`-R kawaii-not-kawaii/Meetily-ActuallyFree`. See `docs/agents/issue-tracker.md`.

### Triage labels

Default vocabulary: `needs-triage`, `needs-info`, `ready-for-agent`,
`ready-for-human`, `wontfix` (plus `wayfinder:*`). See `docs/agents/triage-labels.md`.

### Domain docs

Single-context: root `GLOSSARY.md` and `docs/adr/`, created lazily. See
`docs/agents/domain.md`.

## Code Search

Vera is a machine-local tool, not a repo dependency. On a new machine:
`npx -y @vera-ai/cli install`, `vera setup --potion-code` (offline CPU backend, no
API keys), then `vera index .` in the main checkout; linked worktrees seed their
index from it. The section between the markers is managed by `vera agent sync`.

<!-- vera:begin -->

In a repository with a Vera index, use Vera when the answer is not a string you already know: how or where something works, unfamiliar code, definitions and callers, and enumerating routes, env reads, or implementations. Run `vera overview` when you have not searched this repository yet, then `vera search "<what the code does>"`. For known exact strings, counts, file names, find-and-replace, or unindexed trees, rg or a direct read is the better tool.

If Vera reports no index and this is a code repository you will work in, run `vera index .` from its root. A first index can take several minutes, so give the command a long timeout or run it in the background, and use rg until it finishes; an interrupted API-mode run reuses its finished embeddings. Otherwise, or if you cannot run shell commands, use your usual search tools.

Route by question type:

- How or where something works: `vera search "request teardown ordering"`. Describe behavior ("JWT validation"), not nouns ("auth"). If a phrasing misses, add `--intent "<goal>"` or use `--deep`.
- The question names a function, class, or config key: `vera structural definitions <symbol>` for the definition, `vera references <symbol>` for callers (`--callees` for callees). One call returns ranked definitions or call sites, not raw matching lines.
- Exact text you already know: rg. Use `vera grep "pattern"` when you want Vera's `--lang`, `--path`, or `--changed` filters on indexed files.
- Enumerate every X (signals, routes, env reads, implementations): `vera structural routes`, `vera structural env <NAME>`, `vera structural impls <symbol>`, or `vera grep "X" --lang <lang>`.

Working with results:

- Hits are `path:start-end kind:name` plus the code. Cite from the hit; open a file only for lines the hit did not include.
- If the top hit is a usage site, re-run with the symbol name or `vera structural definitions <symbol>` instead of trying more phrasings.
- `--path` is relative to the repository root (`--path src/flask`, not an absolute path). Narrow with `--lang`, `--path`, `--type`, or `--scope docs`; widen with `--limit 8`.
- Vera indexes this repository only. For dependency sources (site-packages, uv or pip caches) use rg, then return to Vera for repository code.
- A stale-index warning does not invalidate hits. After editing files, run `vera update .` from any directory (it refreshes the repository root's index).
- `vera explain-path <file>`, `vera stats --json`, and detailed usage are in the Vera skill installed by `vera agent install`.
<!-- vera:end -->
