//! Local RAG embedder: a vendored int8 `all-MiniLM-L6-v2` ONNX biencoder run
//! in-process via `ort` (statically linked ONNX Runtime) + `tokenizers`.
//!
//! The model and tokenizer are embedded into the binary with `include_bytes!`,
//! so the embedder is fully offline and self-contained. Initialization is lazy
//! and **degrades gracefully**: if the model fails to load, [`embedder`] returns
//! `None` and callers fall back to sending full (truncated) context.

use std::borrow::Cow;
use std::sync::{Mutex, OnceLock};

use anyhow::Result;
use ort::session::Session;
use ort::value::Tensor;
use tokenizers::Tokenizer;

/// Identifier stored alongside cached vectors so a model swap invalidates them.
pub const EMBEDDING_MODEL: &str = "all-MiniLM-L6-v2-int8";

/// Vendored model + tokenizer, embedded into the binary (Git LFS in-repo).
static MODEL_BYTES: &[u8] = include_bytes!("../models/all-MiniLM-L6-v2/model_int8.onnx");
static TOKENIZER_BYTES: &[u8] = include_bytes!("../models/all-MiniLM-L6-v2/tokenizer.json");

/// Cap on tokens per text. all-MiniLM-L6-v2 is trained at 256; longer inputs add
/// cost without improving retrieval for our paragraph-sized chunks.
const MAX_TOKENS: usize = 256;

/// A loaded biencoder. `Session::run` needs `&mut self`, so the session is behind
/// a `Mutex` (embedding happens serially on the IO thread anyway).
pub struct Embedder {
    session: Mutex<Session>,
    tokenizer: Tokenizer,
    /// The model's expected input tensor names, so we only feed inputs it wants
    /// (some exports omit `token_type_ids`).
    input_names: Vec<String>,
}

static EMBEDDER: OnceLock<Option<Embedder>> = OnceLock::new();

/// Returns the process-wide embedder, lazily initializing it on first use.
/// Returns `None` if the model failed to load (→ full-context fallback).
pub fn embedder() -> Option<&'static Embedder> {
    EMBEDDER.get_or_init(|| Embedder::load().ok()).as_ref()
}

impl Embedder {
    fn load() -> Result<Self> {
        let session = Session::builder()?.commit_from_memory(MODEL_BYTES)?;
        let input_names = session
            .inputs()
            .iter()
            .map(|i| i.name().to_string())
            .collect();
        let mut tokenizer = Tokenizer::from_bytes(TOKENIZER_BYTES)
            .map_err(|e| anyhow::anyhow!("tokenizer: {e}"))?;
        // We embed one text at a time and mean-pool over real tokens; padding
        // would only add zero-mask positions and waste compute.
        tokenizer.with_padding(None);
        Ok(Self {
            session: Mutex::new(session),
            tokenizer,
            input_names,
        })
    }

    /// Embeds each text into an L2-normalized, mean-pooled sentence vector.
    pub fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        texts.iter().map(|t| self.embed_one(t)).collect()
    }

    fn embed_one(&self, text: &str) -> Result<Vec<f32>> {
        let encoding = self
            .tokenizer
            .encode(text, true)
            .map_err(|e| anyhow::anyhow!("tokenize: {e}"))?;

        let mut ids: Vec<i64> = encoding.get_ids().iter().map(|&x| x as i64).collect();
        let mut mask: Vec<i64> = encoding
            .get_attention_mask()
            .iter()
            .map(|&x| x as i64)
            .collect();
        if ids.len() > MAX_TOKENS {
            ids.truncate(MAX_TOKENS);
            mask.truncate(MAX_TOKENS);
        }
        let seq = ids.len();
        if seq == 0 {
            anyhow::bail!("empty tokenization");
        }
        let shape = vec![1i64, seq as i64];
        let type_ids = vec![0i64; seq];

        // Feed only the inputs this model declares.
        let mut inputs: Vec<(Cow<str>, Tensor<i64>)> = Vec::with_capacity(self.input_names.len());
        for name in &self.input_names {
            let tensor = match name.as_str() {
                "input_ids" => Tensor::from_array((shape.clone(), ids.clone()))?,
                "attention_mask" => Tensor::from_array((shape.clone(), mask.clone()))?,
                "token_type_ids" => Tensor::from_array((shape.clone(), type_ids.clone()))?,
                _ => continue,
            };
            inputs.push((Cow::Owned(name.clone()), tensor));
        }

        // Run + pool inside the lock; the output borrows the session.
        let mut session = self
            .session
            .lock()
            .map_err(|_| anyhow::anyhow!("embedder mutex poisoned"))?;
        let outputs = session.run(inputs)?;
        let (_shape, data) = outputs[0].try_extract_tensor::<f32>()?;

        // last_hidden_state is [1, seq, hidden]; batch is 1 so hidden = len/seq.
        let hidden = data.len() / seq;
        let pooled = mean_pool(data, &mask, seq, hidden);
        Ok(l2_normalize(pooled))
    }
}

/// Mean-pools `[seq, hidden]` token embeddings over the masked (real) tokens.
fn mean_pool(data: &[f32], mask: &[i64], seq: usize, hidden: usize) -> Vec<f32> {
    let mut pooled = vec![0.0f32; hidden];
    let mut count = 0.0f32;
    for t in 0..seq {
        if mask.get(t).copied().unwrap_or(1) == 0 {
            continue;
        }
        count += 1.0;
        let row = &data[t * hidden..(t + 1) * hidden];
        for (acc, &v) in pooled.iter_mut().zip(row) {
            *acc += v;
        }
    }
    if count > 0.0 {
        for v in pooled.iter_mut() {
            *v /= count;
        }
    }
    pooled
}

/// L2-normalizes a vector in place, so cosine similarity reduces to a dot product.
fn l2_normalize(mut v: Vec<f32>) -> Vec<f32> {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
    v
}

/// Cosine similarity of two equal-length vectors. Returns 0.0 for mismatched or
/// empty inputs or a zero-magnitude vector (so degenerate cases never produce
/// NaN that would poison threshold comparisons).
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0f32;
    let mut norm_a = 0.0f32;
    let mut norm_b = 0.0f32;
    for i in 0..a.len() {
        dot += a[i] * b[i];
        norm_a += a[i] * a[i];
        norm_b += b[i] * b[i];
    }
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot / (norm_a.sqrt() * norm_b.sqrt())
}

/// Returns the indices of `chunk_embeddings` whose cosine similarity to
/// `query_embedding` is at or above `threshold`, preserving original order.
/// Threshold-based (not ranked): an empty result means "nothing relevant",
/// signalling the caller to fall back to full context.
pub fn select_above_threshold(
    query_embedding: &[f32],
    chunk_embeddings: &[Vec<f32>],
    threshold: f32,
) -> Vec<usize> {
    chunk_embeddings
        .iter()
        .enumerate()
        .filter(|(_, emb)| cosine_similarity(query_embedding, emb) >= threshold)
        .map(|(i, _)| i)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cosine_similarity_identical_orthogonal_opposite() {
        let a = vec![1.0, 0.0, 0.0];
        assert!((cosine_similarity(&a, &a) - 1.0).abs() < 1e-6);
        assert!((cosine_similarity(&a, &[0.0, 1.0, 0.0]) - 0.0).abs() < 1e-6);
        assert!((cosine_similarity(&a, &[-1.0, 0.0, 0.0]) + 1.0).abs() < 1e-6);
        // Magnitude-invariant.
        assert!((cosine_similarity(&a, &[5.0, 0.0, 0.0]) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_cosine_similarity_degenerate_inputs_are_zero() {
        assert_eq!(cosine_similarity(&[], &[]), 0.0);
        assert_eq!(cosine_similarity(&[1.0, 2.0], &[1.0]), 0.0);
        assert_eq!(cosine_similarity(&[0.0, 0.0], &[1.0, 1.0]), 0.0);
    }

    #[test]
    fn test_select_above_threshold() {
        let query = vec![1.0, 0.0];
        let chunks = vec![
            vec![1.0, 0.0],  // sim 1.0
            vec![0.0, 1.0],  // sim 0.0
            vec![0.9, 0.1],  // sim ~0.994
            vec![-1.0, 0.0], // sim -1.0
        ];
        // None qualify at an impossible threshold → full-context fallback signal.
        assert_eq!(
            select_above_threshold(&query, &chunks, 1.1),
            Vec::<usize>::new()
        );
        // A mid threshold selects the close ones, in original order.
        assert_eq!(select_above_threshold(&query, &chunks, 0.5), vec![0, 2]);
        // A low threshold selects everything non-opposite.
        assert_eq!(select_above_threshold(&query, &chunks, 0.0), vec![0, 1, 2]);
    }

    #[test]
    fn test_mean_pool_masks_padding() {
        // 3 tokens, hidden 2; the third token is masked out and must be ignored.
        let data = vec![1.0, 1.0, 3.0, 3.0, 100.0, 100.0];
        let mask = vec![1, 1, 0];
        assert_eq!(mean_pool(&data, &mask, 3, 2), vec![2.0, 2.0]);
    }

    #[test]
    fn test_l2_normalize_unit_length() {
        let v = l2_normalize(vec![3.0, 4.0]);
        assert!((v[0] - 0.6).abs() < 1e-6 && (v[1] - 0.8).abs() < 1e-6);
        // Zero vector stays zero (no NaN).
        assert_eq!(l2_normalize(vec![0.0, 0.0]), vec![0.0, 0.0]);
    }

    // Live model test: requires the vendored ONNX bytes to be present (real LFS
    // content, not a pointer). Ignored by default so CI without LFS/network is
    // unaffected; run with `cargo test --lib -- --ignored embedder_live`.
    #[test]
    #[ignore]
    fn embedder_live_produces_normalized_semantic_vectors() {
        let e = embedder().expect("embedder should load from vendored model");
        let v = e.embed(&["the cat sat on the mat"]).unwrap();
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].len(), 384, "all-MiniLM-L6-v2 is 384-dimensional");
        // Output is L2-normalized.
        let norm: f32 = v[0].iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!(
            (norm - 1.0).abs() < 1e-3,
            "expected unit vector, got {norm}"
        );

        // Semantically related text scores higher than unrelated text.
        let embs = e
            .embed(&[
                "a feline rested on a rug",
                "quarterly financial earnings report",
            ])
            .unwrap();
        let related = cosine_similarity(&v[0], &embs[0]);
        let unrelated = cosine_similarity(&v[0], &embs[1]);
        assert!(
            related > unrelated,
            "related {related} should exceed unrelated {unrelated}"
        );
    }
}
