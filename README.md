<p align="center">
  <img src="assets/diktafon-mark.svg" width="96" height="96" alt="diktafon logo">
</p>

<h1 align="center">diktafon</h1>

<p align="center">
  Local-only dictation for macOS: hold Option+Space, speak, release,<br>
  and polished text is pasted into the frontmost app.
</p>

- Speech-to-text: [Cohere Transcribe](https://huggingface.co/CohereLabs/cohere-transcribe-03-2026) running on-device via [transcribe-rs](https://github.com/cjpais/transcribe-rs).
- Cleanup pass: [S1-mini by Superwhisper](https://huggingface.co/superwhisper/s1-mini) removes fillers and false starts, fixes punctuation, and normalizes numbers, dates, and emails.
