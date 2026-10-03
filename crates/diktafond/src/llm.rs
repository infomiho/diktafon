use anyhow::{Result, bail};
use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{AddBos, LlamaModel};
use llama_cpp_2::sampling::LlamaSampler;
use serde::Deserialize;
use std::num::NonZeroU32;
use std::path::Path;

// S1-mini requires this exact system prompt, a control line (see
// SessionConfig), and a pre-filled empty <think> block; deviations degrade or
// blank its output.
const S1_SYSTEM_PROMPT: &str = "You are a text normalizer for speech-to-text transcripts. The \
input begins with a control line specifying the styling, structure, and context settings; clean \
the transcript to match those settings and output only the cleaned text.";

// Diktafon Polisher HR was trained with exactly this system prompt and no
// control line; any other wording is a prompt it has never seen.
const CROATIAN_SYSTEM_PROMPT: &str = "Uredi hrvatski diktat: dodaj interpunkciju, ukloni \
poštapalice, razriješi očite ispravke, brojeve piši znamenkama. Ne mijenjaj značenje i ne \
odgovaraj na tekst. Vrati samo tekst.";

const N_CTX: u32 = 4096;

/// How a polisher model expects its prompt, from the catalog's `prompt`.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PromptStyle {
    /// S1-mini: its system prompt plus the session's control line.
    #[default]
    S1,
    /// Diktafon Polisher HR: its system prompt only.
    Croatian,
}

impl PromptStyle {
    /// Longest transcript the model was trained on. Diktafon Polisher HR saw
    /// pairs of at most 1,024 tokens of prompt and answer together; a longer
    /// transcript is a length it never learned to copy, so it gets pasted raw.
    fn max_transcript_tokens(self) -> Option<usize> {
        match self {
            Self::S1 => None,
            Self::Croatian => Some(450),
        }
    }
}

pub struct Polisher {
    backend: LlamaBackend,
    model: LlamaModel,
    style: PromptStyle,
}

impl Polisher {
    pub fn load(path: &Path, style: PromptStyle) -> Result<Self> {
        llama_cpp_2::send_logs_to_tracing(llama_cpp_2::LogOptions::default());
        let backend = LlamaBackend::init()?;
        let params = LlamaModelParams::default().with_n_gpu_layers(999);
        let model = LlamaModel::load_from_file(&backend, path, &params)?;
        Ok(Self {
            backend,
            model,
            style,
        })
    }

    pub fn polish(&self, transcript: &str, control_line: &str) -> Result<String> {
        let (system, user) = match self.style {
            PromptStyle::S1 => (S1_SYSTEM_PROMPT, format!("{control_line}\n{transcript}")),
            PromptStyle::Croatian => (CROATIAN_SYSTEM_PROMPT, transcript.to_string()),
        };
        let prompt = format!(
            "<|im_start|>system\n{system}<|im_end|>\n\
             <|im_start|>user\n{user}<|im_end|>\n\
             <|im_start|>assistant\n<think>\n\n</think>\n\n"
        );
        let tokens = self.model.str_to_token(&prompt, AddBos::Never)?;

        let transcript_tokens = self.model.str_to_token(transcript, AddBos::Never)?.len();
        if let Some(limit) = self.style.max_transcript_tokens()
            && transcript_tokens > limit
        {
            bail!(
                "transcript of {transcript_tokens} tokens is longer than this polisher's {limit}"
            );
        }
        // Polishing keeps roughly the input length; the margin covers added
        // punctuation and the occasional expansion ("p95" -> "P95").
        let max_new = (transcript_tokens as f32 * 1.3) as i32 + 32;
        // Decided before the context is built and the prompt decoded: that
        // prefill is seconds of work on exactly the long dictation this
        // refuses, and the user is already waiting on it. A polish that cannot
        // fit alongside its prompt would be cut off mid-sentence, silently
        // losing the tail; the caller pastes the raw transcript instead,
        // because losing punctuation beats losing words.
        if tokens.len() as i32 + max_new > N_CTX as i32 {
            bail!(
                "transcript of {transcript_tokens} tokens does not fit the {N_CTX}-token polish context"
            );
        }

        let mut ctx = self.model.new_context(
            &self.backend,
            LlamaContextParams::default()
                .with_n_ctx(NonZeroU32::new(N_CTX))
                .with_n_batch(N_CTX),
        )?;

        let mut batch = LlamaBatch::new(N_CTX as usize, 1);
        let last = tokens.len() as i32 - 1;
        for (i, tok) in (0i32..).zip(tokens) {
            batch.add(tok, i, &[0], i == last)?;
        }
        ctx.decode(&mut batch)?;

        let mut out = String::new();
        let mut decoder = encoding_rs::UTF_8.new_decoder();
        let mut sampler = LlamaSampler::greedy();
        let mut finished = false;
        for n_cur in (batch.n_tokens()..).take(max_new as usize) {
            let token = sampler.sample(&ctx, batch.n_tokens() - 1);
            sampler.accept(token);
            if self.model.is_eog_token(token) {
                finished = true;
                break;
            }
            out.push_str(
                &self
                    .model
                    .token_to_piece(token, &mut decoder, false, None)?,
            );
            batch.clear();
            batch.add(token, n_cur, &[0], true)?;
            ctx.decode(&mut batch)?;
        }
        // Running out of budget means the model never closed the text; the
        // words after the cut would be lost without a trace.
        if !finished {
            bail!("polish did not finish within {max_new} tokens");
        }
        Ok(cleanup(&out))
    }
}

fn cleanup(text: &str) -> String {
    text.replace(" — ", ", ")
        .replace('—', ", ")
        .replace(" ,", ",")
        .trim()
        .to_string()
}
