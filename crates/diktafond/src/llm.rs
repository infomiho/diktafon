use anyhow::Result;
use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{AddBos, LlamaModel};
use llama_cpp_2::sampling::LlamaSampler;
use std::num::NonZeroU32;
use std::path::Path;

// S1-mini requires this exact system prompt, a control line (see
// SessionConfig), and a pre-filled empty <think> block; deviations degrade or
// blank its output.
const S1_SYSTEM_PROMPT: &str = "You are a text normalizer for speech-to-text transcripts. The \
input begins with a control line specifying the styling, structure, and context settings; clean \
the transcript to match those settings and output only the cleaned text.";

const N_CTX: u32 = 4096;

pub struct Polisher {
    backend: LlamaBackend,
    model: LlamaModel,
}

impl Polisher {
    pub fn load(path: &Path) -> Result<Self> {
        llama_cpp_2::send_logs_to_tracing(llama_cpp_2::LogOptions::default());
        let backend = LlamaBackend::init()?;
        let params = LlamaModelParams::default().with_n_gpu_layers(999);
        let model = LlamaModel::load_from_file(&backend, path, &params)?;
        Ok(Self { backend, model })
    }

    pub fn polish(&self, transcript: &str, control_line: &str) -> Result<String> {
        let prompt = format!(
            "<|im_start|>system\n{S1_SYSTEM_PROMPT}<|im_end|>\n\
             <|im_start|>user\n{control_line}\n{transcript}<|im_end|>\n\
             <|im_start|>assistant\n<think>\n\n</think>\n\n"
        );
        let tokens = self.model.str_to_token(&prompt, AddBos::Never)?;

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

        let transcript_tokens = self.model.str_to_token(transcript, AddBos::Never)?.len();
        let max_new = (transcript_tokens as f32 * 1.3) as i32 + 32;
        let mut out = String::new();
        let mut decoder = encoding_rs::UTF_8.new_decoder();
        let mut sampler = LlamaSampler::greedy();
        for n_cur in (batch.n_tokens()..).take(max_new as usize) {
            let token = sampler.sample(&ctx, batch.n_tokens() - 1);
            sampler.accept(token);
            if self.model.is_eog_token(token) {
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
