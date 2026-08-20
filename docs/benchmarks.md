# Model benchmarks

Method: 12 LibriSpeech clips (130s) plus 5 real dictation recordings (127s, verbatim ground truth confirmed by the speaker). WER with normalization that canonicalizes number formats and brand compounds. Peak RSS via `time -l`, dev build, CPU int8 ONNX unless noted. Harness: `bench` binary in the spike workspace.

## ASR engines

| Engine | LibriSpeech WER | Dictation WER | Load | Speed | Peak RSS |
|---|---|---|---|---|---|
| Cohere Transcribe int8 | 2.64% | 3.98% | 3.0s | 10x RT | 4.28GB |
| Cohere Transcribe int4 | - | 5.31% | 1.3s | 5.8x RT | 3.26GB |
| Canary 180M Flash int8 | 3.30% | 3.54% | 0.3s | 21x RT | 0.97GB |
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
