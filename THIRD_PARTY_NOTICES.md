# Third-party notices

This file covers the downloadable models and native inference components used by diktafon. Model files are downloaded separately, but remain subject to these notices.

## NVIDIA Canary 1B Flash and Handy GGUF

NVIDIA Canary 1B Flash (`nvidia/canary-1b-flash`) by NVIDIA is licensed under Creative Commons Attribution 4.0 International. Diktafon uses the GGUF conversion and Q5_K_M quantization published by handy-computer for transcribe.cpp. The weights were converted and quantized from NVIDIA's original model; diktafon did not modify the model weights.

- NVIDIA source: https://huggingface.co/nvidia/canary-1b-flash/tree/a9a55e0295e7dd50d0c8c2a19491900a0daf24f3
- Handy conversion: https://huggingface.co/handy-computer/canary-1b-flash-gguf/tree/b427664769b93c021df108a2fa8bfb858ae236c1
- License: https://creativecommons.org/licenses/by/4.0/legalcode

The conversion and quantization are adaptations licensed under CC BY 4.0 by handy-computer. No endorsement by NVIDIA or handy-computer is implied.

## Cohere Transcribe and Handy GGUF

Cohere Transcribe (`CohereLabs/cohere-transcribe-03-2026`) was developed by Cohere and Cohere Labs and is licensed under the Apache License, Version 2.0. Diktafon uses the GGUF conversion and Q5_K_M quantization published by handy-computer for transcribe.cpp.

- Cohere source: https://huggingface.co/CohereLabs/cohere-transcribe-03-2026/tree/76b8b23e8607f35f0265a23d481b338fb0e26aea
- Handy conversion: https://huggingface.co/handy-computer/cohere-transcribe-03-2026-gguf/tree/dfa4adebb64f3076b7b6b90b721275cc069cb421

## "S1-mini" by "Superwhisper"

This product identifies the model by its required original name, "S1-mini" by "Superwhisper". The model is distributed under the Apache License, Version 2.0, with this additional term from its pinned upstream license:

> Any use, distribution, or integration of this model, whether unmodified or as part of a derivative work or product, must continue to identify it by its original name, "S1-mini" by "Superwhisper", using that exact capitalization, regardless of any other name under which the model or a product incorporating it is marketed or distributed.

- Model and license: https://huggingface.co/superwhisper/s1-mini-GGUF/tree/8eab4779866f477ae6e7f237ca45fc2c65153f50

S1-mini is based on Qwen3-0.6B by Qwen. Qwen3-0.6B is licensed under the Apache License, Version 2.0.

Copyright 2024 Alibaba Cloud

- Qwen source and license reference: https://huggingface.co/Qwen/Qwen3-0.6B/tree/c1899de289a04d12100db370d81485cdf75e47ca

## transcribe.cpp

transcribe.cpp, revision `c6a9257cdf8e9c6918c0f8f876246db048a22103`, is licensed under the MIT License.

Copyright (c) 2026 The transcribe.cpp authors

Its vendored ggml revision `8c63e70982c95ceb862e3a1073a2c1beef75d60a` is MIT licensed.

Copyright (c) 2023-2026 The ggml authors

Its vendored miniz 3.1.1 revision `d10b03cc73475af673df40f06e5cefd1d5f940d9` is MIT licensed.

Copyright 2013-2014 RAD Game Tools and Valve Software

Copyright 2010-2014 Rich Geldreich and Tenacious Software LLC

All Rights Reserved.

- Source and third-party notices: https://github.com/handy-computer/transcribe.cpp/tree/c6a9257cdf8e9c6918c0f8f876246db048a22103

## llama.cpp and Rust bindings

llama-cpp-2 and llama-cpp-sys-2 revision `bed81ad4ab1a6c904b11d425608e50f976d8ea62` are available under MIT or Apache-2.0. Diktafon satisfies their terms under MIT.

Copyright (c) Dial AI

The bindings embed llama.cpp revision `5f55650a78f92aff4d48d671423e888fac0469ff`, licensed under MIT.

Copyright (c) 2023-2026 The ggml authors

- Bindings: https://github.com/utilityai/llama-cpp-rs/tree/bed81ad4ab1a6c904b11d425608e50f976d8ea62
- llama.cpp: https://github.com/ggml-org/llama.cpp/tree/5f55650a78f92aff4d48d671423e888fac0469ff

## Sparkle

Sparkle 2.9.6, the framework embedded in the app bundle for in-app updates, is licensed under the MIT License.

Copyright (c) 2006-2013 Andy Matuschak.
Copyright (c) 2009-2013 Elgato Systems GmbH.
Copyright (c) 2011-2014 Kornel Lesiński.
Copyright (c) 2015-2017 Mayur Pawashe.
Copyright (c) 2014 C.W. Betts.
Copyright (c) 2014 Petroules Corporation.
Copyright (c) 2014 Big Nerd Ranch.
All rights reserved.

Sparkle bundles bsdiff 4.3 (Copyright 2003-2005 Colin Percival, BSD-2-Clause), sais-lite (Copyright (c) 2008-2010 Yuta Mori, MIT), ed25519 (Copyright (c) 2015 Orson Peters, zlib), and SUSignatureVerifier.m (Copyright (c) 2011 Mark Hamlin, BSD-2-Clause). Their full texts are in Sparkle's license file.

- Source and license: https://github.com/sparkle-project/Sparkle/blob/2.9.6/LICENSE

## Apple Foundation Models

Apple Intelligence uses Apple's system-provided Foundation Model. No Apple model weights or framework binaries are redistributed by diktafon, so there is no model redistribution notice. Use remains subject to the Apple Developer Program License Agreement.

- Framework documentation: https://developer.apple.com/documentation/foundationmodels

## Solar Icons

The interface icons in `crates/diktafon/assets/icons/diktafon` are from
[Solar Icons](https://www.figma.com/community/file/1166831539721848736) by
480 Design, in the Linear style, taken from the
[Iconify](https://github.com/iconify/icon-sets) `solar` set. The glyphs are
unmodified; each is wrapped in its own SVG element.

Copyright (c) 480 Design

Licensed under the
[Creative Commons Attribution 4.0 International License](https://creativecommons.org/licenses/by/4.0/).

## Lucide Icons

The `x.svg` dismiss glyph bundled alongside the Solar set in
`crates/diktafon/assets/icons/diktafon` is from
[Lucide](https://lucide.dev/), redrawn at the Solar 1.5px stroke weight;
all other interface icons are Solar as noted above.

Copyright (c) Lucide Contributors

Licensed under the [ISC License](https://opensource.org/licenses/ISC).

## MIT License

The following text applies to each MIT-licensed component listed above, together with its respective copyright notice.

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

## Apache License 2.0

The complete, unmodified text is distributed alongside this file at `licenses/Apache-2.0.txt`.
