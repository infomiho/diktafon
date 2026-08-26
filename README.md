<p align="center">
  <img src="assets/diktafon-mark.svg" width="96" height="96" alt="diktafon logo">
</p>

<h1 align="center">diktafon</h1>

<p align="center">
  Local-only dictation for macOS: hold Option+Space, speak, release,<br>
  and polished text is pasted into the frontmost app.
</p>

- Speech-to-text: [Canary 1B Flash](https://huggingface.co/nvidia/canary-1b-flash) by default, or Cohere Transcribe, running on-device via transcribe.cpp.
- Cleanup pass: [S1-mini by Superwhisper](https://huggingface.co/superwhisper/s1-mini) removes fillers and false starts, fixes punctuation, and normalizes numbers, dates, and emails.
- Licenses and model attribution: [third-party notices](THIRD_PARTY_NOTICES.md).
