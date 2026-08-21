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

## Decision

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
