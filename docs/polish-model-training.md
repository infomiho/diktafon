# Training our own polish model

Goal: a polish model that matches or beats S1-mini on the five-clip eval for the one style diktafon uses, then shrink it below S1-mini's 462 MB and 0.35 s warm polish latency.

## What S1-mini is

- A supervised fine-tune of Qwen3-0.6B with thinking disabled, which is why the ChatML prompt with an empty think block is mandatory.
- Steered by a control line with three axes (styling 4 values, structure 2, context 2). Every combination was trained, so each source transcript became up to 16 pairs.
- Test set is 7,519 cases from 104 transcripts, so the corpus is mostly synthetic expansion of a small real core. Vendor-reported 94.8% token accuracy on the Q4_K_M build.
- Known failures on our eval: kept the sentence after "scratch the last sentence", turned Canary's "appeal nine thousand one" into "9,110", changed "we or I" to "we". These are data-coverage holes, not capacity limits.

Sources: [model card](https://huggingface.co/superwhisper/s1-mini), [S1 announcement](https://superwhisper.com/blog/s1).

## Why we can win

The task is a constrained rewrite that mostly copies its input. We need one control line, not sixteen, and we can train on our own ASR engines' error distribution rather than a generic one. Both make a smaller model viable.

## Compute

Modal. An L4 or A10G run of the 0.6B model takes about 20 minutes and a dozen runs fit inside the $30 monthly credit. Both cards support bf16, which Qwen3 needs (free T4 tiers do not). One Python file, GPU type as a decorator, a Modal volume for data and checkpoints, per-second billing, nothing to tear down. Move to Vast.ai spot only for hour-long sweeps.

## Phases

### 0. Baseline and harness

- Run S1-mini on the five confirmed clips and the reviewed history pairs. Record normalized WER and a manual diff list. This is the bar.
- Make the eval a script that takes any GGUF and prints the same numbers, so every later run is one command.

### 1. Dataset (no GPU)

- Real core: relabel the raw side of every `history.jsonl` row with a frontier model, using the S1 failure list as a checklist. Judge with a second model so the labeler does not grade its own work. Drop failures. Spot-check 30 by hand once.
- Synthetic bulk: collect clean English text (emails, notes, chat, encyclopedia sentences) and run a deterministic verbalizer that turns it into fake ASR output: strip punctuation and casing, spell out numbers, dates, currency, and emails, inject fillers, repeats, false starts, and "scratch that" corrections that resolve to the final value. Sample substitutions from real Canary and Cohere errors.
- Target around 20k pairs, one control line (semi-formal prose). Hold out a slice plus the five clips.

### 2. Match S1

- Full fine-tune Qwen3-0.6B, thinking off, two to three epochs, Unsloth or TRL. Export to GGUF Q4_K_M.
- Add it as a catalog entry and run the harness. Success is no new meaning changes and the known S1 failures fixed.
- If it falls short, the fix is data, not the model. Iterate on the verbalizer and the failure checklist.

### 3. Shrink

- Same dataset on Qwen2.5-0.5B and a 0.3B-class decoder.
- One encoder-decoder candidate such as Flan-T5-small, exported through ONNX or Core ML since it will not run through llama.cpp.
- Keep whichever passes the harness at the smallest download and lowest warm polish latency, under the 0.5 s bar in `benchmarks.md`.

### 4. Ship

- Publish the winner to Hugging Face, add it to the catalog, make it the default polisher, record method and numbers in `benchmarks.md`.

## Croatian

Croatian dictation pastes raw Canary 1B v2 output because S1-mini and Apple Intelligence are English-only. The same pipeline can cover it, and real Croatian audio also makes an ASR fine-tune possible.

Pairs come from running our ASR over audio that already has a reference transcript. The raw ASR output is the polish input and the reference is the target, so the model learns the ASR's real Croatian errors (truncations, merged words, broken case endings) instead of simulated ones.

Sources:

- ParlaSpeech-HR (CLASSLA): about 1,800 hours of Sabor speech aligned to the official records. The records are edited by stenographers (fillers and false starts removed), which suits a polish target better than an ASR target. Formal register only, no self-corrections. Verify license and size before use.
- Croatian-language films and series with same-language subtitles (domestic productions, HRT subtitles for the deaf and hard of hearing). Foreign films are useless because their Croatian subtitles are translations. Subtitles condense fast speech to meet reading-speed limits and cue timing is only accurate to a few hundred milliseconds, so cut at cue boundaries and keep only cues whose ASR output closely matches the subtitle. Filtering on ASR agreement keeps mostly what the ASR already gets right, so use a lenient threshold or an independent aligner and measure the filter's precision on a hand-checked sample. This is the conversational, fast, emotional speech parliament lacks.
- VoxPopuli (European Parliament, some Croatian), FLEURS and Common Voice (hr): small read-speech sets, better as extra evaluation than as training data.
- Synthetic dictation: a frontier model writes Croatian dictation with fillers, "ne, zapravo" corrections, and English loanwords with Croatian endings ("deployao", "pushati"), paired with the clean text. Neither parliament nor film covers self-corrections.
- Our own Croatian `history.jsonl` rows once daily use builds them up, relabeled like the English core.

ASR fine-tune: the same audio and references can fine-tune Canary 1B v2 itself through NeMo, fixing errors at the source instead of in the polisher, followed by conversion to GGUF with Handy's transcribe.cpp converter. A Croatian Canary fine-tune is unlikely to exist, but check for one on ParlaSpeech first, since conversion may then be all that is needed. Decide between ASR fine-tune, polisher, or both from the meaning-changing errors recorded in the Croatian section of `benchmarks.md`.

`eval-own-hr` stays held out from every training set. Five clips are too few to separate candidates, so record 10 to 20 more before comparing Croatian models.

## Decision points

- After phase 2: stop if the 0.6B fine-tune cannot beat S1. Shrinking a worse model is pointless.
- After phase 3: skip the encoder-decoder branch if a 0.3B decoder already passes, to avoid a second inference runtime in the daemon.

## Needs from the owner

- Modal account with a payment method attached, which unlocks the monthly credit.
- A frontier-model API key for labeling and judging.
- A quick look at 30 relabeled dictations before training, and at the five-clip diffs after.
