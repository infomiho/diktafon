# Handy's Canary 1B Flash path

Research snapshot: Handy `c6fa60da2f13a5af660fba17f37af548855119c5` (2026-08-25), transcribe.cpp `c6a9257cdf8e9c6918c0f8f876246db048a22103` (2026-08-24). Only the requested primary sources were used.

## Conclusions

- Handy runs the displayed **Canary 1B Flash (733 MB)** as one **Q5_K_M GGUF** through **transcribe.cpp/ggml**, using the `transcribe-cpp` Rust crate. It does not run this entry through NeMo, ONNX Runtime, or `transcribe-rs`. NeMo is used upstream and by the conversion/validation tooling, not in Handy inference.
- `733 MB` is Handy's integer MiB display of `769,563,424` bytes. The exact file is `canary-1b-flash-Q5_K_M.gguf`, SHA-256 `7eed3cac92f255a4adbd518c58663d3fbf65984d2619189e593f2d374b05c601`, from `handy-computer/canary-1b-flash-gguf` at revision `b427664769b93c021df108a2fa8bfb858ae236c1`.
- Q5_K_M is the catalog entry's **default quantization**, not Handy's application-wide default model. Canary 1B Flash is neither `recommended` nor ranked. The generator selects Q5_K_M because its GGUF size label is `1B` and the policy is Q5_K_M for models `>=1B`; Handy otherwise starts with no selected model and onboarding presents ranked recommendations.
- On Apple Silicon, Handy statically builds transcribe.cpp with Metal and `Backend::Auto` selects the best available device with CPU fallback. The published transcribe.cpp M4 Max result is about 96-103x realtime on Metal for Q8/Q4; no Q5-specific latency is published. Handy's UI scores are **accuracy 90** and **speed 83**, derived from Q8 WER 1.62% and Ryzen Vulkan 14.4x realtime, not measured in Handy itself.
- For diktafon, the existing 16 kHz mono `f32` PCM, language setting, worker/model residency, panic isolation, pinned model manifest, resumed download, SHA-256 verification, and downstream text/polish flow are reusable. The added backend would be `transcribe-cpp` plus its native C++/ggml build and Metal feature. The existing `transcribe-rs` ONNX Cohere engine and model directory are not reusable for this GGUF.

## Catalog identity and selection

The compiled catalog entry records:

| Field | Value |
| --- | --- |
| Repository | `handy-computer/canary-1b-flash-gguf` |
| Pinned catalog revision | `b427664769b93c021df108a2fa8bfb858ae236c1` |
| Base model | `nvidia/canary-1b-flash` |
| Architecture / parameters | `canary` / catalog label `1B` (upstream actual count: 883M) |
| Default file | `canary-1b-flash-Q5_K_M.gguf` |
| Exact size | `769,563,424` bytes = 733 MiB (HF card rounds to 734 MB) |
| SHA-256 | `7eed3cac92f255a4adbd518c58663d3fbf65984d2619189e593f2d374b05c601` |
| Languages | `en`, `de`, `es`, `fr` |
| Catalog capabilities | translation yes; streaming, language detection, timestamps no |
| Catalog/UI scores | speed 83, accuracy 90 |
| License | CC-BY-4.0 |

The repository also contains Q4_K_M, Q6_K, Q8_0, F16, and F32 files with individual sizes and LFS SHA-256 values. Handy exposes the Q5 file because `scripts/gen_catalog.py` reads `general.size_label`, chooses Q5_K_M for `>=1B`, and falls back through Q8_0, any Q8, Q5_K_M, then the smallest file. `ModelDescriptor::default_file` resolves that quant; `to_model_info` computes `size_mb` with integer division by `1024 * 1024` and builds the identity as `<repo>/<filename>`.

This is not a global recommendation: the entry has `recommended: false` and no rank. Handy's settings default to an empty model ID; onboarding controls the initial choice, and after onboarding an empty selection is filled by the first already-downloaded model in catalog/UI sort order.

Sources:

- Handy [`src-tauri/src/catalog/catalog.json`, lines 339-369](https://github.com/cjpais/Handy/blob/c6fa60da2f13a5af660fba17f37af548855119c5/src-tauri/src/catalog/catalog.json#L339-L369)
- Handy [`scripts/gen_catalog.py`, lines 167-201 and 203-260](https://github.com/cjpais/Handy/blob/c6fa60da2f13a5af660fba17f37af548855119c5/scripts/gen_catalog.py#L167-L260)
- Handy [`src-tauri/src/managers/model.rs`, lines 132-143 and 182-251](https://github.com/cjpais/Handy/blob/c6fa60da2f13a5af660fba17f37af548855119c5/src-tauri/src/managers/model.rs#L132-L251)
- Handy [`src-tauri/src/settings.rs`, lines 497-499](https://github.com/cjpais/Handy/blob/c6fa60da2f13a5af660fba17f37af548855119c5/src-tauri/src/settings.rs#L497-L499) and [`model.rs`, lines 1479-1523](https://github.com/cjpais/Handy/blob/c6fa60da2f13a5af660fba17f37af548855119c5/src-tauri/src/managers/model.rs#L1479-L1523)
- Hugging Face [repository files/API at revision `b427664...`](https://huggingface.co/api/models/handy-computer/canary-1b-flash-gguf/tree/b427664769b93c021df108a2fa8bfb858ae236c1) and [model card](https://huggingface.co/handy-computer/canary-1b-flash-gguf/blob/b427664769b93c021df108a2fa8bfb858ae236c1/README.md)

## Download and verification

The catalog is generated from HF metadata, requires every GGUF to have a positive size and LFS/Xet SHA-256, pins the repository commit, and is compiled into Handy with `include_str!`. New network acquisition uses the immutable identity `resolve/<revision>/<filename>`.

For the primary source, Handy's fork of `hf-hub` downloads the one file into the shared HF cache. It retries with four streams and then sequential streams, supports cancellation/resume, and switches to the catalog mirror after HF failure. The current Handy code does **not** call `ModelManager::verify_sha256` after a successful primary `hf-hub` download; it relies on the pinned HF revision and the downloader/cache behavior. Cache lookup also accepts an older `refs/main` entry when the pinned ref is absent, explicitly preserving pre-pinning files without revalidation.

The mirror URL is `https://blob.handy.computer/<repo>/<revision>/<filename>`. Mirror bytes go to `<filename>.partial`, enforce expected size and range offsets, resume safely, and are SHA-256 checked before atomic rename. A mismatch/read error deletes the partial. The catalog hash is explicitly the mirror trust anchor.

Sources:

- Handy [`src-tauri/src/catalog/mod.rs`, lines 1-14, 73-110, and 125-173](https://github.com/cjpais/Handy/blob/c6fa60da2f13a5af660fba17f37af548855119c5/src-tauri/src/catalog/mod.rs#L1-L173)
- Handy [`src-tauri/src/managers/model.rs`, cache lookup at lines 313-332](https://github.com/cjpais/Handy/blob/c6fa60da2f13a5af660fba17f37af548855119c5/src-tauri/src/managers/model.rs#L313-L332), [download at lines 1849-2109](https://github.com/cjpais/Handy/blob/c6fa60da2f13a5af660fba17f37af548855119c5/src-tauri/src/managers/model.rs#L1849-L2109), and [mirror finalization at lines 2112-2150](https://github.com/cjpais/Handy/blob/c6fa60da2f13a5af660fba17f37af548855119c5/src-tauri/src/managers/model.rs#L2112-L2150)
- Handy [`src-tauri/src/managers/model/download.rs`, lines 52-119 and 159-379](https://github.com/cjpais/Handy/blob/c6fa60da2f13a5af660fba17f37af548855119c5/src-tauri/src/managers/model/download.rs#L52-L379)

## Engine, binding, and model load

Handy 0.9.6 depends on `transcribe-cpp = 0.2.0` and `transcribe-cpp-sys = 0.2.0`. Catalog descriptors always map to `EngineType::TranscribeCpp`. During load Handy:

1. Resolves the downloaded GGUF path and accelerator/device setting.
2. Calls Rust `Model::load_with(path, ModelOptions { backend, device })`.
3. The binding initializes `transcribe_model_load_params` and calls C `transcribe_model_load_file`.
4. Calls `model.session()`; the session retains the model and is cached across dictations until unload/replacement.
5. Re-reads capabilities from loaded GGUF metadata and updates Handy's runtime model record.

The native Canary implementation is a FastConformer encoder plus autoregressive Transformer decoder in `src/arch/canary/`; it computes its own mel features and executes ggml graphs. The converter is the only NeMo-dependent part: it loads NVIDIA's `.nemo` `EncDecMultiTaskModel`, emits one F32 GGUF with tokenizer/config/tensors, then transcribe.cpp quantizes it. NVIDIA's HF `model.safetensors` is explicitly an encoder-only shim and cannot generate transcripts; Handy does not use it.

Sources:

- Handy [`src-tauri/Cargo.toml`, lines 77-82 and 126-165](https://github.com/cjpais/Handy/blob/c6fa60da2f13a5af660fba17f37af548855119c5/src-tauri/Cargo.toml#L77-L165) and [`Cargo.lock`, `transcribe-cpp` entries](https://github.com/cjpais/Handy/blob/c6fa60da2f13a5af660fba17f37af548855119c5/src-tauri/Cargo.lock#L7278-L7297)
- Handy [`src-tauri/src/managers/transcription.rs`, lines 179-190](https://github.com/cjpais/Handy/blob/c6fa60da2f13a5af660fba17f37af548855119c5/src-tauri/src/managers/transcription.rs#L179-L190) and [model load at lines 471-627](https://github.com/cjpais/Handy/blob/c6fa60da2f13a5af660fba17f37af548855119c5/src-tauri/src/managers/transcription.rs#L471-L627)
- transcribe.cpp Rust binding [`model.rs`, lines 108-150](https://github.com/handy-computer/transcribe.cpp/blob/c6a9257cdf8e9c6918c0f8f876246db048a22103/bindings/rust/transcribe-cpp/src/model.rs#L108-L150) and [`session.rs`, lines 28-63 and 94-180](https://github.com/handy-computer/transcribe.cpp/blob/c6a9257cdf8e9c6918c0f8f876246db048a22103/bindings/rust/transcribe-cpp/src/session.rs#L28-L180)
- transcribe.cpp [`scripts/convert-canary.py`, lines 1-53 and 118-126](https://github.com/handy-computer/transcribe.cpp/blob/c6a9257cdf8e9c6918c0f8f876246db048a22103/scripts/convert-canary.py#L1-L53) and [`src/arch/canary/model.cpp`](https://github.com/handy-computer/transcribe.cpp/blob/c6a9257cdf8e9c6918c0f8f876246db048a22103/src/arch/canary/model.cpp)
- transcribe.cpp [Canary port notes, lines 18-28](https://github.com/handy-computer/transcribe.cpp/blob/c6a9257cdf8e9c6918c0f8f876246db048a22103/docs/porting/families/canary.md#L18-L28)

## Audio and inference contract

Handy's recorder converts device audio to mono `f32` PCM at 16,000 Hz with `rubato`; the Rust binding's `Session::run` contract is 16 kHz mono floats in `[-1, 1]`. Handy passes the complete stopped recording to this non-streaming model. transcribe.cpp then computes the model-configured NeMo-compatible log-mel frontend (128 mels, 512 FFT, 400-sample/25 ms window, 160-sample/10 ms hop, pre-emphasis 0.97, per-feature normalization, zero inference dither) before FastConformer encoding. The runtime advertises an approximately 400-second positional-table limit and rejects longer input; this is more permissive than upstream NeMo's documented direct-inference recommendation below 40 seconds.

Canary has no language detection. Handy's effective-language logic resolves a matching `en/de/es/fr` intent and coerces `auto` or an unsupported intent to English; the run plan then passes that concrete language. For normal dictation Handy sets:

- `task = Transcribe`
- `language =` validated supported language
- `target_language = None`
- `pnc = Default`, which Canary maps to punctuation/capitalization **on**
- timestamps `Auto`, but the port advertises/returns none
- ITN/diarization defaults; special tags stripped
- greedy autoregressive decode, up to 512 generated tokens; no external LM

If "translate to English" is enabled and the source is non-English, Handy uses `Task::Translate` and target `en`. English input stays ASR. Although transcribe.cpp supports EN to DE/ES/FR too, Handy's UI path only requests translation **to English**. The Canary prompt embeds source, target, task, PNC, and disabled timestamp controls. This model is batch-only: no live partials, VAD, language detection, or timestamps in the v1 transcribe.cpp port.

Sources:

- Handy recorder [`audio_toolkit/audio/recorder.rs`, lines 740-759](https://github.com/cjpais/Handy/blob/c6fa60da2f13a5af660fba17f37af548855119c5/src-tauri/src/audio_toolkit/audio/recorder.rs#L740-L759)
- Handy language coercion [`managers/model.rs`, lines 269-303](https://github.com/cjpais/Handy/blob/c6fa60da2f13a5af660fba17f37af548855119c5/src-tauri/src/managers/model.rs#L269-L303)
- Handy run path [`transcription.rs`, lines 1261-1343](https://github.com/cjpais/Handy/blob/c6fa60da2f13a5af660fba17f37af548855119c5/src-tauri/src/managers/transcription.rs#L1261-L1343) and [task plan, lines 1726-1759 and 1830-1852](https://github.com/cjpais/Handy/blob/c6fa60da2f13a5af660fba17f37af548855119c5/src-tauri/src/managers/transcription.rs#L1726-L1852)
- transcribe.cpp binding [`session.rs`, lines 28-63 and 146-169](https://github.com/handy-computer/transcribe.cpp/blob/c6a9257cdf8e9c6918c0f8f876246db048a22103/bindings/rust/transcribe-cpp/src/session.rs#L28-L63)
- transcribe.cpp Canary [`model.cpp`, mel/run and prompt handling](https://github.com/handy-computer/transcribe.cpp/blob/c6a9257cdf8e9c6918c0f8f876246db048a22103/src/arch/canary/model.cpp#L548-L732), [`capabilities.cpp`](https://github.com/handy-computer/transcribe.cpp/blob/c6a9257cdf8e9c6918c0f8f876246db048a22103/src/arch/canary/capabilities.cpp), and [input limits](https://github.com/handy-computer/transcribe.cpp/blob/c6a9257cdf8e9c6918c0f8f876246db048a22103/docs/models/canary.md#L45-L51)
- NVIDIA pinned [model card: architecture, inputs, task manifest, long-form behavior, and output](https://huggingface.co/nvidia/canary-1b-flash/blob/a9a55e0295e7dd50d0c8c2a19491900a0daf24f3/README.md)

## Output handling

The Rust binding materializes `Transcript.text`; Handy ignores timestamp/token structures and takes only `t.text`. transcribe.cpp strips Canary special tags by default. Handy then optionally applies fuzzy custom-word correction (Canary cannot accept Whisper's initial prompt), detects output language from constrained transcript text only when needed for filler filtering, removes configured filler words, collapses repeated stutters/whitespace, trims, and returns the string to the normal action/paste path. Optional cleanup is fail-open: a panic returns the raw model text.

Sources:

- Handy [`transcription.rs`, lines 1292-1343 and 1467-1492](https://github.com/cjpais/Handy/blob/c6fa60da2f13a5af660fba17f37af548855119c5/src-tauri/src/managers/transcription.rs#L1292-L1492) and [post-processing, lines 1762-1826](https://github.com/cjpais/Handy/blob/c6fa60da2f13a5af660fba17f37af548855119c5/src-tauri/src/managers/transcription.rs#L1762-L1826)
- Handy [`audio_toolkit/text.rs`, lines 383-433](https://github.com/cjpais/Handy/blob/c6fa60da2f13a5af660fba17f37af548855119c5/src-tauri/src/audio_toolkit/text.rs#L383-L433)
- transcribe.cpp C API [`include/transcribe.h`, lines 1041-1064](https://github.com/handy-computer/transcribe.cpp/blob/c6a9257cdf8e9c6918c0f8f876246db048a22103/include/transcribe.h#L1041-L1064)

## Platforms and acceleration

Handy builds macOS arm64 and x86_64, Linux x86_64 and arm64, and Windows x86_64 and arm64. For this transcribe.cpp engine:

- **macOS:** static `metal` feature. Metal is the GPU path on Apple Silicon; Auto falls back to CPU. transcribe.cpp skips pre-Apple7 Metal GPUs without simdgroup matrix multiply in Auto, which is relevant to Intel-era Mac GPUs. Intel macOS remains build-supported but should be treated as CPU for this path unless a compatible Metal device is actually registered.
- **Linux/Windows x86_64:** dynamic CPU backends plus Vulkan; Auto can select Vulkan and falls back to CPU.
- **Windows arm64:** statically linked CPU-only in Handy due to its Vulkan/toolchain constraints.
- transcribe.cpp itself also supports CUDA and ROCm, but Handy 0.9.6 does not enable those crate features.

Published transcribe.cpp Canary 1B Flash measurements (not Handy end-to-end) are: M4 Max Metal 103.2x realtime on 11 s and 95.9x on 35.3 s for Q8_0; M4 Max CPU 21.2x/19.7x. The HF card's headline `rtf_m4_max.metal` is 99.6. No Q5_K_M speed row is published, so a 733 MB performance claim would require measurement.

Sources:

- Handy [platform build matrix](https://github.com/cjpais/Handy/blob/c6fa60da2f13a5af660fba17f37af548855119c5/.github/workflows/main-build.yml#L19-L45), [`Cargo.toml` backend features](https://github.com/cjpais/Handy/blob/c6fa60da2f13a5af660fba17f37af548855119c5/src-tauri/Cargo.toml#L126-L165), and [backend selection](https://github.com/cjpais/Handy/blob/c6fa60da2f13a5af660fba17f37af548855119c5/src-tauri/src/managers/transcription.rs#L1955-L1973)
- transcribe.cpp [README build/backends](https://github.com/handy-computer/transcribe.cpp/blob/c6a9257cdf8e9c6918c0f8f876246db048a22103/README.md#L34-L80), [Metal compatibility guard](https://github.com/handy-computer/transcribe.cpp/blob/c6a9257cdf8e9c6918c0f8f876246db048a22103/src/transcribe-load-common.cpp#L32-L51), and [Canary benchmark](https://github.com/handy-computer/transcribe.cpp/blob/c6a9257cdf8e9c6918c0f8f876246db048a22103/docs/models/canary-1b-flash.md#L72-L99)

## Scores and quality evidence

Handy's generator computes `accuracy = round(100 * exp(-WER/15))` and `speed = round(100 * (1 - exp(-RTF/8)))`. Its headline-WER preference chooses Q8_0 before Q5_K_M, producing 90 from 1.62%; speed uses Ryzen 4750U Vulkan 14.4x, producing 83. These are catalog display scores, not a Handy product benchmark.

The model repository reports full LibriSpeech test-clean greedy/no-LM WER: F32/F16/Q8 1.62%, Q6 1.65%, **Q5 1.64%**, Q4 1.59%. NVIDIA reports 1.48% for its NeMo reference under its own evaluation. transcribe.cpp reports tensor-by-tensor validation against NeMo and matching F32 transcript on its validation sample. Quantized WER being slightly lower than F32 is possible evaluation variation, not evidence that Q4 is intrinsically more accurate.

Sources:

- Handy [`scripts/gen_catalog.py`, lines 30-37 and 95-103](https://github.com/cjpais/Handy/blob/c6fa60da2f13a5af660fba17f37af548855119c5/scripts/gen_catalog.py#L30-L103)
- handy-computer [model card WER/RTF block](https://huggingface.co/handy-computer/canary-1b-flash-gguf/blob/b427664769b93c021df108a2fa8bfb858ae236c1/README.md)
- transcribe.cpp [validation and reproduction](https://github.com/handy-computer/transcribe.cpp/blob/c6a9257cdf8e9c6918c0f8f876246db048a22103/docs/models/canary-1b-flash.md#L113-L150)
- NVIDIA pinned [performance tables](https://huggingface.co/nvidia/canary-1b-flash/blob/a9a55e0295e7dd50d0c8c2a19491900a0daf24f3/README.md#performance)

## Licensing and attribution

The base model and GGUF conversion are **CC-BY-4.0**, which the NVIDIA card explicitly allows for commercial use. Distribution of the GGUF therefore needs CC-BY attribution and license information, and adaptations should be identified as such. A practical notice should name NVIDIA's `canary-1b-flash`, link the upstream model/card and CC-BY-4.0, identify the `handy-computer` GGUF conversion/quantization and transcribe.cpp, and note any further changes. This is distinct from the older `canary-1b`, which is CC-BY-NC-4.0.

Handy and transcribe.cpp are MIT-licensed. transcribe.cpp vendors ggml and miniz under MIT and documents their notices. Shipping the native backend requires retaining those MIT notices as well as the model's separate CC-BY attribution. Handy's runtime catalog currently deserializes only operational fields and explicitly ignores the catalog `license` field, so downstream attribution should not rely on the UI/catalog alone.

Sources:

- NVIDIA pinned [license statement](https://huggingface.co/nvidia/canary-1b-flash/blob/a9a55e0295e7dd50d0c8c2a19491900a0daf24f3/README.md#description)
- handy-computer [model card license and provenance](https://huggingface.co/handy-computer/canary-1b-flash-gguf/blob/b427664769b93c021df108a2fa8bfb858ae236c1/README.md)
- transcribe.cpp [Canary license distinction](https://github.com/handy-computer/transcribe.cpp/blob/c6a9257cdf8e9c6918c0f8f876246db048a22103/docs/porting/families/canary.md#L13-L20), [MIT license](https://github.com/handy-computer/transcribe.cpp/blob/c6a9257cdf8e9c6918c0f8f876246db048a22103/LICENSE), and [third-party notices](https://github.com/handy-computer/transcribe.cpp/blob/c6a9257cdf8e9c6918c0f8f876246db048a22103/THIRD-PARTY-LICENSES.md)
- Handy [`catalog/mod.rs`, lines 36-38](https://github.com/cjpais/Handy/blob/c6fa60da2f13a5af660fba17f37af548855119c5/src-tauri/src/catalog/mod.rs#L36-L38)

## Minimal diktafon integration delta

Reusable from the current Cohere path:

- Client-side capture already supplies 16 kHz mono float PCM (`TARGET_RATE`), and daemon protocol chunks are already `Vec<f32>`.
- `Inference` already owns resident models on one worker thread, catches native panics, passes a configured language, emits partial/final events, and feeds the same S1-mini polish/history/paste pipeline.
- `manifest.rs` already uses immutable HF revisions, exact sizes, SHA-256 verification, resumable staging, and atomic completion. Canary would simplify the ASR payload from four ONNX/token files to one GGUF.

Added or changed:

- Add `transcribe-cpp` with `default-features = false, features = ["metal"]` on macOS, including its CMake/C++/ggml native build. This is independent of the existing `transcribe-rs = 0.3.11` ONNX dependency and `llama-cpp-2`; the fact both use GGUF/ggml does not make their model/session APIs interchangeable.
- Replace `CohereModel::load(directory, Int8)` with `transcribe_cpp::Model::load_with(gguf, Backend::Auto)` plus a retained `Session`, then call `session.run(samples, RunOptions { language, ..Default::default() })` and consume `Transcript.text`.
- Canary requires one of four explicit languages and is not streaming. Diktafon can keep its existing silence chunking and transcribe each completed chunk, but should not expect model-native partials, auto language detection, or timestamps. Translation needs explicit `Task::Translate` and target `en` (or another valid English-pivot target).
- Add CC-BY-4.0 model attribution and transcribe.cpp/ggml MIT notices to distributed artifacts/documentation.

Current diktafon sources: `crates/diktafond/Cargo.toml:8-9`, `crates/diktafond/src/inference.rs:35-60,192-220`, `crates/diktafond/src/manifest.rs:1-55`, and `crates/protocol/src/lib.rs` (`TARGET_RATE`).
