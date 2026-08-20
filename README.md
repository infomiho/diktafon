# diktafon

- Local-only dictation for macOS: hold Option+Space, speak, release, and polished text is pasted into the frontmost app.
- Speech-to-text: [Cohere Transcribe](https://huggingface.co/CohereLabs/cohere-transcribe-03-2026) running on-device via [transcribe-rs](https://github.com/cjpais/transcribe-rs).
- Cleanup pass: [S1-mini by Superwhisper](https://huggingface.co/superwhisper/s1-mini) removes fillers and false starts, fixes punctuation, and normalizes numbers, dates, and emails.
