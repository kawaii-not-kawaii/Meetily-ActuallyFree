# Upstream correctness ports (batch 2, November 2026)

These are selected fixes from upstream `Zackriya-Solutions/meetily` (after the 0.4.1
integration in [UPSTREAM_0_4_1_PORTS.md](UPSTREAM_0_4_1_PORTS.md)). Each one was
adapted to this fork rather than merged wholesale. Each commit cites its upstream
SHA or PR.

## Included

- **Parakeet model switch deadlock** (upstream `254ea02`, #749).
  `ParakeetEngine::load_model` kept the `current_model_name` read guard alive as an
  `if let` temporary. It then called `unload_model`, which waits for the write lock
  on the same tokio `RwLock`. As a result, switching between two loaded models
  (v2-int8 ↔ v3-int8) hung. Load also held the `available_models` read guard during
  the blocking ONNX load. Now:
  - the catalog entry is cloned out of the lock;
  - `model_lifecycle_lock` serializes load and unload;
  - the current name is read into an owned value;
  - `ParakeetModel::new` runs on `spawn_blocking`.

  Tests: `switching_from_a_loaded_model_does_not_deadlock` (new) and
  `loading_releases_available_models_and_serializes_unload` (from upstream).
- **Recording start checks the configured provider's downloads** (#639).
  `useRecordingStart.checkIfModelDownloading` now asks the selected provider's
  catalog (Parakeet or Whisper), using `lib/transcription-model-readiness.ts`.
- **Whisper download hardening** (#737, `1ae3036`…`eeb4566`):
  - a per-download owner with a `CancellationToken`;
  - cancel-before-retry serialization;
  - terminal cleanup on every exit path;
  - rejection of undersized files;
  - `Downloading` state before the first byte;
  - `whisper_cancel_download` returns `cancelled | pending`, and the UI reconciles
    with the backend before it allows a retry.

  Fork adaptations:
  - The frontend status contract is `{ Downloading: { progress } }`, which is what
    the backend always serialized. `OptionalModelDownloadsContext` and a test fixture
    used a bare number and were corrected.
  - **Optional activation:** when `enable_post_call` is set, a cancelled download
    still rejects. `OptionalModelDownloadsContext` treats a resolved promise as
    "downloaded and enabled". Post-call activation still runs only after a validated
    completion ([V0220_OPTIONAL_ACTIVATION.md](V0220_OPTIONAL_ACTIVATION.md)).
  - Upstream's retranscription-metadata test duplicated ours and was dropped.
  - The cancel UI was re-themed with `af-*` tokens.
  - Upstream's `Meetily/<version>` User-Agent on Whisper downloads was removed.
    It is not needed, and it would identify the app.
- **VAD forced-flush bound** (`cda2ff0`, `bc00314`). `ContinuousVadProcessor::flush`
  zero-pads the final partial frame. The forced segment's end timestamp and payload
  now stop at the real (pre-padding) end sample. An empty or reversed interval is
  dropped with a warning. Upstream returns an error instead, which would also drop
  segments already queued. Our calibrated VAD thresholds and redemption values are
  unchanged; #679/#771 were not ported. Test:
  `test_flush_segment_timestamps_stay_within_audio_duration`.
- **Reasoning-tag cleaner** (#744, `e1a7155`/`afcfeb5`, cleaner only). The cleaner
  strips `<think>`/`<thinking>` envelopes in any case and with attributes. If a reply
  starts with reasoning and a leading orphan `</think>` (implicit opener), the text
  before that marker is dropped. A summary is rejected if it still has an unpaired
  marker or is empty after reasoning removal. Not ported: the #744 summary UI,
  lifecycle and persistence changes, Ollama `reasoning_effort`, and `analytics.ts`.
- **Parakeet v3 revision pin** (`d16c04f`). The Hugging Face v3 source is pinned to
  `8f23f0c03c8761650bdb5b40aaf3e40d2c15f1ce`. On 2026-11-24, HEAD requests to that
  revision returned the exact sizes in `download_file_sizes`, and a Range request
  returned 206. No User-Agent was added: Hugging Face serves the files without one,
  and upstream's `Meetily/<version>` UA would disclose the app version.
- **Log scrubbing** (from #755, `beec9b8`). Logs no longer contain:
  - the auth token prefix;
  - endpoint URLs beyond scheme://host:port (`utils::url_origin_for_log`);
  - URLs inside reqwest errors;
  - response or error bodies (only their size is logged);
  - the transcript search query;
  - transcript text (only its character count is logged).

  Logging still goes to stderr. The persistent `<app-data>/logs` file pipeline from
  #755 was deliberately not ported.

## Known follow-ups (not changed here)

- ~~Parakeet has the same `Downloading` payload mismatch that Whisper had.~~ Fixed in
  the follow-up PR: `lib/parakeet.ts` and `ParakeetModelManager` now use
  `{ Downloading: { progress } }`, covered by
  `tests/download-progress/parakeet-manager.test.tsx`.
- ~~The Parakeet v2 Hugging Face source still uses `resolve/main`.~~ Fixed in the
  follow-up PR: pinned to `0bbb45a3365852604aef28b538a8f066f4ccaa85` (the HF API's
  current `sha`, and the same revision upstream pins). On 2026-11-24, HEAD requests
  returned exactly the v2 sizes in `download_file_sizes` (encoder 652184014,
  decoder_joint 8998286, nemo128 139764, vocab 9384) and a Range request returned
  206. No User-Agent was added.
- Every remaining Hugging Face model download is now pinned to a revision as
  well (no `resolve/main` remains in app code). Each revision is the repo's HF API
  `sha` on 2026-11-24. At that revision, HEAD returned the sizes below and a Range
  request returned 206. No User-Agent was added.

  | Download | Repo | Revision | Size check |
  | --- | --- | --- | --- |
  | Summary `qwen3.5:2b` | `unsloth/Qwen3.5-2B-GGUF` | `f6d5376be1edb4d416d56da11e5397a961aca8ae` | 1280835840 = `size_bytes` |
  | Summary `qwen3.5:4b` | `unsloth/Qwen3.5-4B-GGUF` | `e87f176479d0855a907a41277aca2f8ee7a09523` | 2740937888 = `size_bytes` |
  | Summary `gemma3:4b` | `bartowski/google_gemma-3-4b-it-GGUF` | `71506238f970075ca85125cd749c28b1b0eee84e` | 2489758112 = `size_bytes` |
  | Summary `gemma3:1b` | `bartowski/google_gemma-3-1b-it-GGUF` | `116f76234503685a98f572982177b11d44ec8ff1` | 1069306624 = `size_bytes` |
  | All 12 Whisper catalog models | `ggerganov/whisper.cpp` | `5359861c739e955e79d9a303bcbc70fb988958b1` | each byte-identical in size to `main`, above the catalog's 90% minimum |

  Whisper URLs are now derived from `WHISPER_MODEL_CATALOG` filenames, so the
  catalog and the downloadable set cannot drift apart. Tests:
  `every_catalog_model_downloads_from_the_pinned_revision` (whisper_engine) and
  `every_model_download_is_pinned_to_a_hugging_face_revision` (summary models).

  Summary downloads resume only a partial that the same URL produced: the URL is
  recorded in `<model>.gguf.source` before bytes are appended. A partial with no
  record (from a build that used `resolve/main`) or with a different URL is deleted
  and downloaded from the start. Without this, mixed-revision bytes could pass the
  size and GGUF-magic checks. Test:
  `partial_downloads_resume_only_from_their_recorded_source`.
