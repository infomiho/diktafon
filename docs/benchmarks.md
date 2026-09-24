# Model benchmarks

Method: 12 LibriSpeech clips (130s) plus 5 real dictation recordings (127s, verbatim ground truth confirmed by the speaker). WER with normalization that canonicalizes number formats and brand compounds. Peak RSS via `time -l`, dev build, CPU int8 ONNX unless noted. Harness: `bench` binary in the spike workspace.

## ASR engines

| Engine | LibriSpeech WER | Dictation WER | Load | Speed | Peak RSS |
|---|---|---|---|---|---|
| Cohere Transcribe int8 | 2.64% | 3.98% | 3.0s | 10x RT | 4.28GB |
| Cohere Transcribe int4 | - | 5.31% | 1.3s | 5.8x RT | 3.26GB |
| Canary 180M Flash int8 | 3.30% | 3.54% | 0.3s | 21x RT | 0.97GB |
| Canary 1B v2 int8 | - | 7.96%* | 0.8s | 12x RT | 2.67GB |
| Parakeet TDT 0.6B v3 int8 | 3.30% | 4.87% | 0.6s | 33x RT | 1.57GB |
| Moonshine base | 9.57% | 3.98% | 0.4s | 40x RT | 0.52GB |
| SenseVoice int8 | 9.24% | 10.62% | 0.4s | 62x RT | 0.76GB |

## Polish model

s1-mini q4_k_m (462MB) produced identical output to f16 (1.5GB) on all test phrases, and decodes faster. q4 is the default.

## Initial decision

Cohere int8 + s1-mini q4_k_m.

- End-to-end quality decided it, not raw WER: Cohere performs disfluency editing (drops asides, resolves "one thousand, no, ten thousand" into "10,000") that s1-mini does not replicate. Small-ASR pipelines compounded errors after polish (e.g. s1-mini rewrote Canary's spelled-out "nine thousand one" as "91").
- Cohere int4 trades 1GB of RAM for roughly half the inference speed; rejected since latency is per-dictation and memory is addressed by idle unload.
- Memory plan: idle model unload/reload instead of a smaller ASR.
- *Canary 1B v2's WER is inflated by its own ITN and editing being scored against verbatim ground truth. After polish it nearly matches Cohere: better number/compound formatting (SlideMaster, -60°C), slightly worse faithfulness (spelling of rare words, keeps asides). It is the designated runner-up at 1.6GB less RAM if memory ever outweighs fidelity.
- Canary 180M was rejected on polished-output diffs: real content errors a user must hand-fix (PNG 91, PEC ice, Hasko for Haskell).

## Client/daemon loopback overhead (2026-08-21)

Measured with `cargo run --release -p diktafond --example loopback_bench`: eval clip 01 in five ≤5s chunks through the in-process worker versus a spawned diktafond over the Unix socket, identical audio, warmed ASR. The Cancel→Aborted roundtrip does no inference, so it isolates pure transport cost (framing + socket + worker channel + relay).

| Metric | In-process | Unix socket |
| --- | --- | --- |
| chunk → Partial, median of 5 (5s audio, ~320KB frame) | 466.1ms | 447.7ms |
| Flush → Final (single run) | 422.7ms | 770.7ms |
| Cancel → Aborted, median of 200 (no inference) | 3.3µs | 14.6µs |

The split costs ~11µs per roundtrip; per-chunk latency is inside run-to-run noise (the socket run even sampled faster). The Flush→Final gap is first-polish Metal warmup variance in each freshly started process, not transport, as the µs-scale roundtrip shows. Loopback overhead is invisible next to 200-560ms inference.

## CoreML execution provider (2026-08-21)

Evaluated with `cargo run --release -p diktafond --example asr_bench` (per-clip RTF over the eval set).

The example takes `[MODEL_ID] [LANGUAGE] [CLIP_DIR]`, defaulting to the default transcription model, `en`, and `eval-own`. When the clip directory has a `manifest.json` it also prints WER. That WER only lowercases and replaces punctuation with spaces, so it is not comparable to the canonicalized (numbers, brand compounds) WER figures elsewhere in this doc.

- CPU int8 baseline: 126.6s of audio in 13.53s, 9.4x realtime (per clip 6.1-11.0x; model load 5.6s).
- CoreML EP (`--features coreml -- coreml`): fails at session init with onnxruntime's "model_path must not be empty" - the CoreML EP cannot handle our external-data model (the 2.7GB `.onnx.data`) under ort 2.0.0-rc.12 / transcribe-rs 0.3.11.

Decision at the time: stay on CPU int8. This path was removed after the GGUF migration below.

## Cohere transcribe.cpp migration (2026-08-26)

Compared the old Cohere int8 ONNX result above with Handy's pinned `cohere-transcribe-03-2026-Q5_K_M.gguf` on the same five confirmed clips. The GGUF run used Metal on an M2 Pro, explicit English, the model's punctuation-enabled default, greedy decoding, and the existing number/brand-compound normalization.

| Metric | ONNX int8 | GGUF Q5_K_M |
| --- | ---: | ---: |
| Normalized dictation WER | 3.98% | 3.96% |
| Warm five-clip speed | 9.4x realtime | 28.6x realtime |
| Model load | 5.6s | 1.3s warm / 10.6s first Metal initialization |
| Peak RSS | 4.28GB | 1.92GB |
| Download | 2.89GB | 1.77GB |

The normalized GGUF errors were five words in clip 01, two in clip 02, and two in clip 04. Content differences requiring review were `polymer blend` becoming `polymer plant` and `PENG 9001` becoming `PNG 9001`; Cohere also retained its intended editing of false starts and duplicate words. Fixed five-second chunks were a boundary stress test rather than the product's silence-based chunking and produced additional omissions and rare-word substitutions in both Cohere and Canary, confirming that future comparisons must keep chunk boundaries identical.

Decision: accept the GGUF migration as the Cohere default. Aggregate quality is unchanged while inference is 3x faster, resident memory is 55% lower, and the download is 39% smaller. The meaning-changing examples remain part of the four-pipeline transcript review before final product defaults are chosen. The old ONNX files are not deleted by the application.

## Apple Intelligence polish smoke test (2026-08-26)

Apple Foundation Models was available on the M2 Pro test machine. Cohere's fixed-five-second-chunk raw output from all five confirmed clips was polished through `SystemLanguageModel.default`; no request failed, refused, or used the S1 fallback.

Apple preserved the rare product and standard substitutions already present in raw ASR rather than inventing corrections. It correctly applied the explicit “scratch the last sentence” instruction in clip 03. In clip 05 it added an unwanted “Sure,” and changed “we or I never got used to” to “we never got used to,” which alters the speaker's qualification. It also retained “one thousand, no, ten thousand” rather than formatting the corrected number as S1 did. These differences keep Apple Intelligence opt-in until the complete four-pipeline review.

## Four-pipeline evaluation (2026-08-26)

This local measurement used the same five confirmed clips and fixed five-second chunks for every pair. The machine was a 16 GB M2 Pro running macOS 26.5.1. Both ASR models ran through transcribe.cpp on Metal device `MTL0`; S1-mini was Q4_K_M, and Apple used the system-provided Foundation Model. Apple reported `Available`; all ten Apple requests completed without refusal, timeout, or S1 fallback.

Each row is the sum or mean of five release-build runs after model loading. “Total” is batch ASR plus polish, not paced release-to-paste latency.

| Transcription | Polishing | Normalized raw WER | ASR total | Mean polish | Mean total |
| --- | --- | ---: | ---: | ---: | ---: |
| Cohere Q5_K_M | S1-mini Q4_K_M | 6.17% | 5.39s / 23.5x RT | 0.39s | 1.47s |
| Cohere Q5_K_M | Apple Intelligence | 6.17% | 5.51s / 23.0x RT | 1.33s | 2.44s |
| Canary Q5_K_M | S1-mini Q4_K_M | 20.70% | 2.91s / 43.5x RT | 0.35s | 0.93s |
| Canary Q5_K_M | Apple Intelligence | 20.70% | 3.02s / 41.9x RT | 0.96s | 1.56s |

WER lowercases and removes punctuation, then canonicalizes the evaluated number forms and compounds such as `Slide Master`/`SlideMaster`, `three thousand`/`3000`, `minus sixty`/`minus 60`, and `one point two`/`1.2`. It is 14 edits over 227 reference words for Cohere and 47 over 227 for Canary. Fixed chunks are deliberately harsher than whole-clip inference: boundary losses raise Cohere from the earlier 3.96% result and expose larger Canary omissions.

The unnormalized review found:

- Cohere retained the fewest words incorrectly, but fixed boundaries dropped words including `Slide`, `across three Antarctic colonies`, and the start of `we are implementing`; its known entity errors included `Krill Chase` to `Krill Chick` and `PENG 9001` to `NG 9001`.
- Canary was faster and used less memory, but omitted substantial tails or clauses in clips 02 and 03. Its `PENG 9001` to `appeal nine thousand one` became `9,110` under S1, a meaning-changing number edit.
- S1 reliably converted the explicit correction to `10,000`, removed repeated words, and joined many chunk-boundary fragments. It sometimes preserved sentence fragments and changed the qualification `we or I` to `we` for Canary.
- Apple removed false starts and joined fragments, but retained `one thousand, no, ten thousand`. It prefixed clip 05 with `Sure` or an explanatory sentence and changed `we or I` to `we`, both unwanted style or meaning changes.
- Apple honored “scratch the last sentence”; S1 removed the spoken instruction but incorrectly retained the sentence it referred to. Neither repaired ASR entity substitutions, which is preferable to inventing corrections.

ASR-only whole-clip cold-process measurements used `/usr/bin/time -l`. The first process paid about 9.9 seconds to initialize the embedded Metal library. Cohere loaded in 11.31 seconds, ran the five clips at 23.7x realtime, and peaked at 1.91 GB RSS. Canary loaded in 10.61 seconds, ran at 27.0x realtime, and peaked at 0.87 GB RSS. Catalog downloads are 1.77 GB for Cohere, 770 MB for Canary, 484 MB for S1-mini, and zero for system-provided Apple Intelligence. The daemon exits after the configured idle interval to release resident models; changing a pair retires the old daemon before the replacement loads.

Reliability boundaries are deterministic: sessions below one second return no text; empty ASR output skips polishing; Apple input above 12,000 characters is rejected; Apple output is capped at 2,048 tokens and its bridge cancels after 20 seconds. Unavailable Apple Intelligence is not offered; a refused, empty, timed-out, or otherwise failed selected Apple request returns the raw transcript. Unit tests cover short input, Apple availability reasons, daemon restart, model mismatch, active-session deferral, interrupted downloads, and corrupt artifacts.

Run one pair:

```sh
target/release/diktafon --transcribe-file "$WAV" --json \
  --transcription-model cohere-transcribe-q5-k-m \
  --polishing-model s1-mini-q4-k-m
```

Run the full matrix by iterating the two catalog IDs in each category over the five WAV files. JSON output identifies the clip, stable model IDs, warm state, backend/device, audio duration, ASR/polish/total latency, raw transcript, and polished result. Keep that output local because it contains private transcripts; only these aggregate measurements and approved example differences belong in the repository.

## Default decision (2026-08-26)

The human transcript review selected `canary-1b-flash-q5-k-m` with `s1-mini-q4-k-m` as the defaults. Canary's 770 MB download, 0.87 GB ASR peak RSS, and higher throughput were preferred over Cohere's better fixed-chunk WER, accepting Canary's observed entity errors and clause omissions. S1 remains the default because it is always local and available, averaged 0.35 seconds per polish with Canary, handled corrected numbers better than Apple, and did not add explanatory framing.

Apple Intelligence remains an ordinary polishing option only when the system reports it available; unsupported devices do not see it. It is not a default or advanced-only option. A per-request Apple refusal, timeout, empty response, or other failure returns the raw transcript, avoiding an unselected S1 rewrite. Cohere remains selectable for users who prefer transcription fidelity over download size, memory, and speed.

Future default ASR artifacts must run this same five-clip matrix with identical boundaries. They must introduce no new dropped clauses or meaning-changing entity/number errors, stay at or below Canary's 20.70% fixed-chunk normalized WER, and not regress its 0.87 GB ASR peak RSS or 27.0x whole-clip throughput by more than 10%. Future default polishers must add no new meaning-changing rewrites or unsolicited framing, preserve explicit corrections, and keep mean warm polish latency below 0.5 seconds on this machine.

## Croatian transcription candidates (2026-09-23)

Method: the Croatian eval set `eval-own-hr` (five dictations, 120s, 233 reference words, verbatim ground truth confirmed by the speaker), each clip transcribed whole by `asr_bench <model id> hr <clip dir>` from `cargo build --release -p diktafond --example asr_bench`. Three model families list Croatian: Whisper large-v3-turbo, and NVIDIA's Parakeet TDT 0.6B v3 and Canary 1B v2, all from Handy's transcribe.cpp GGUFs at pinned revisions. Each file was checked against its Hugging Face sha256 and loaded from a scratch `DIKTAFON_DATA_DIR` under a catalog model's file name. transcribe.cpp picks the architecture from GGUF metadata, and every load reported the expected family. Whisper ran with its family decode defaults: no initial prompt and the default temperature fallback. Canary v2 takes `hr` as its source-language prompt. Parakeet v3 accepts `hr` because its GGUF lists it, but it has no language prompt and detects the language itself, so an `en` hint gave identical output. Everything ran on Metal (`MTL0`) on a 16 GB M2 Pro (`Mac14,9`) with macOS 27.0.

A copy of clip 01 sorts first as an unscored warmup, and warm speed is scored audio over scored inference time. Each model ran once cold and three more times under `/usr/bin/time -l`, with the dev app's daemon confirmed idle before each run. Speed and warm load are medians of the three, peak RSS is the maximum of all four. Earlier runs had already initialized Metal and read each file, so cold load is a first process for the file, not the ~10s first Metal initialization noted above.

Raw WER is the `asr_bench` figure. Canonicalized WER uses a closed rule list, applied identically to reference and hypothesis and fixed before any run beyond Whisper Q5_K_M: times written `10 i 30`, `10.30`, or `10:30` become one form, `%` becomes `posto`, a hyphen between letters is removed, `tri`/`šest`/`osam`/`devet` become digits, `kilometara` becomes `km`, and the open compounds `web socket` and `stand upa` are joined. It also drops the fillers `hm`, `hmm`, `hmmm`, and `mhm`, a deviation from the English method that removes 3 reference-only edits for every model. Canary v2 applies its own number formatting and punctuation, and the same rules decide which of its formats count as matches.

| Model | Raw WER (edits/233) | Canonicalized WER | Warm speed | Load cold/warm | Peak RSS | Download |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Whisper turbo Q5_K_M | 19.7% (46) | 12.7% (29/228) | 20.9x RT | 0.40s / 0.26s | 0.82GB | 620MB |
| Whisper turbo Q8_0 | 22.3% (52) | 15.8% (36/228) | 23.3x RT | 0.66s / 0.34s | 1.10GB | 886MB |
| Parakeet TDT 0.6B v3 Q5_K_M | 27.0% (63) | 23.2% (53/228) | 80.6x RT | 0.25s / 0.22s | 0.77GB | 549MB |
| Canary 1B v2 Q5_K_M | 18.0% (42) | 11.0% (25/228) | 46.7x RT | 0.35s / 0.30s | 1.00GB | 837MB |

With 233 words from one speaker every WER here carries roughly ±3 pp of sampling uncertainty, so both Whisper quantizations and Canary v2 overlap and only Parakeet is clearly worse. Manual review sorted the remaining canonicalized edits. An error counts as meaning-changing when the output is a real word or phrase that says something different, or when a fact is dropped. Obvious non-words are not counted.

| Model | Segmentation | Omission | Morphology | Truncation | Lexical | Phonetic spelling | Proper noun | Number format | Meaning-changing |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Whisper turbo Q5_K_M | 8 | 7 | 4 | 4 | 3 | 2 | 1 | 0 | 4 |
| Whisper turbo Q8_0 | 10 | 5 | 4 | 4 | 8 | 2 | 1 | 2 | 4 |
| Parakeet TDT 0.6B v3 | 9 | 4 | 7 | 3 | 13 | 9 | 7 | 1 | 5 |
| Canary 1B v2 | 4 | 7 | 3 | 0 | 5 | 3 | 3 | 0 | 2 |

Whisper Q5_K_M loses a weekday and a time of day and changes a verb and a subject noun. Q8_0 loses the weekday, replaces a deadline date with a number, changes a verb, and turns another verb into a place name. Parakeet loses the weekday, reverses a spoken time correction, garbles a clause, and changes a verb's person and a product name. Canary v2 drops a weekday and changes a product name.

- No Cyrillic or other wrong-language output from any model in any run.
- Transcripts were identical across all runs of each model, so Whisper's temperature fallback never sampled differently between runs.
- The only run of three or more deleted reference words is in both Whisper quantizations: a spoken time self-correction that Whisper resolved, next to a weekday merged into the following word. Review counts that as omitted words plus one meaning-changing error, not a dropped clause. Whisper Q8_0 replaced a four-word deadline phrase with a number and Parakeet garbled the subject and object of a clause, which score as substitutions but lose the clause's content.
- Zero padding (each clip plus 5s of digital silence): both Whisper quantizations appended the same invented sign-off sentence to clip 05. The NVIDIA models added no text, but the padding degraded Parakeet's recognition throughout the clips (raw WER 33.0%) and changed a few words for Canary v2 (17.2%). With 0.5s of zero padding, about the VAD chunker's 0.45s hangover, no model added text.
- Room-tone padding (5s of pink noise at -60 dBFS RMS, informational since VAD keeps silence from the ASR): Whisper invented tails on clips 03 and 05, the NVIDIA models added none, and Parakeet again degraded throughout (33.0%).
- Parakeet has no token for one capital Croatian letter and emitted a literal `<unk>` inside a place name.
- The fixed 5s chunk stress test was not run: `--transcribe-file` starts every session with default settings and so always requests English.

Croatian is opt-in, so the gates are absolute rather than relative to Canary 1B Flash: no wrong-language output, no dropped clause in whole-clip runs, no text on zero padding, at least 10x realtime warm, and peak RSS at or below Cohere's 1.91 GB. WER has no threshold. Both Whisper quantizations pass the language, speed, and memory gates and fail the zero-padding gate, and Q8_0 also fails the dropped-clause gate on review. Parakeet fails the dropped-clause gate on review. Canary v2 passes all five.

Decision: Canary 1B v2 Q5_K_M is the catalog's Croatian model. Whisper turbo is not shipped. Canary v2 is the only candidate that passes every gate, it has the fewest meaning-changing errors (2 against Whisper Q5_K_M's 4) and the lowest WER, though within Whisper's uncertainty, and it runs 2.2x faster than Whisper Q5_K_M for 0.18 GB more peak RSS and a 217 MB larger download. Its `hr` hint is a real prompt token, so the product's per-session language needs no daemon change. Parakeet is the fastest and smallest but has twice the edits and the most meaning-changing errors, ignores the language hint, and degrades badly when trailing audio changes. Croatian still pastes raw ASR because no polisher supports `hr`, so the remaining errors reach the user unedited.
