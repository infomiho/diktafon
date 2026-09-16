# Memory replay

Build the client and daemon, then replay a local 16 kHz mono signed 16-bit WAV:

```sh
cargo build --release -p diktafon -p diktafond
python3 scripts/memory-replay.py "/path/to/recording.wav" --repeat 20
```

The loop keeps one client and one isolated daemon alive across all iterations. It records client `footprint` and `vmmap -summary` before replay and after each completed transcription. Results and a CSV go into a private temporary directory. History recording is disabled. Treat logs as private.

This measures the headless transport and transcription path. It does not exercise microphone capture, voice activity detection, the pill, or clipboard insertion. Daemon memory is not included. Stack logging is disabled for the replay client to avoid profiler overhead.

Compare live heap and total footprint separately. Stable live allocations with a fluctuating footprint suggest allocator retention. Repeat with short and long clips and compare later cycles after initialization. Do not interpret the headless footprint as the app's idle footprint.

For growth found only in the interactive app, profile the capture, window, and paste paths separately. Validate any fix in the full app after the isolated regression passes.

## Keyboard event sources

These probes create and release native keyboard events without posting them:

```sh
python3 scripts/memory-event-sources.py --repeat 1000
python3 scripts/memory-event-sources.py --repeat 1000 --reuse
```

Creating an event from a fresh private source per cycle reproduced roughly 16 KB of growth per cycle on the tested macOS system. Reusing one source stayed flat after initialization. Creating sources without creating events did not reproduce that growth.

The paste path retains one private event source on the dictation thread. It builds the complete Command+V chord before posting any events, so event creation failures cannot leave a modifier pressed. Accessibility is checked on every insertion.

Run native regressions separately from other tests:

```sh
cargo test -p diktafon paste::tests::repeated_keyboard_sessions_have_bounded_memory -- --ignored --exact --nocapture
cargo test -p diktafon paste::tests::repeated_image_pastes_preserve_clipboard_without_memory_growth -- --ignored --exact --nocapture
cargo test -p diktafon capture::silero_tests::repeated_vad_sessions_release_memory -- --ignored --exact --nocapture
cargo run -p diktafon --example idle_memory
```

The keyboard test creates real paste chords without posting them. It checks changing layout keycodes, modifier flags and releases, and bounds footprint growth across 1,000 cycles. The clipboard test temporarily uses and restores the real clipboard. The VAD test loads the local model and evaluation audio for 30 sessions. The window test checks native view deallocation across 20 closures. These checks do not cover microphone device churn or end-to-end event delivery to another application.

## Device metadata

```sh
cargo run -p diktafon --example device_name_memory
```

This regression queries the default input device description 10,000 times without opening the microphone and asserts that live heap growth stays below 128 KB. CPAL 0.17.3 uses coreaudio-rs 0.14.2, which releases the returned CoreAudio strings.
