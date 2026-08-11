//! Dense embedding providers and vector math.
//!
//! Production search uses model2vec with the Semble-compatible
//! `minishlab/potion-code-16M` model cached under the local Devo model
//! directory. Missing model files are fetched on first use; load/download
//! failures become typed `ModelUnavailable` errors so the tool can report a
//! recoverable cache/model problem instead of panicking. Tests use a deterministic
//! hash provider to exercise indexing without network or model dependencies.

use std::path::PathBuf;
use std::sync::Mutex;

use devo_network_proxy::NetworkProxyConfig;
use devo_util_paths::find_devo_home;
use hf_hub::HFClient;
use hf_hub::HFClientSync;
use model2vec::model::Model2Vec;
use sha2::{Digest, Sha256};

use crate::types::CodeSearchError;

const DEFAULT_MODEL_OWNER: &str = "minishlab";
const DEFAULT_MODEL_NAME: &str = "potion-code-16M";
const DEFAULT_MODEL_ID: &str = "minishlab/potion-code-16M";
const LOCAL_MODELS_DIR: &str = "local-models";
const MODEL_FILES: [&str; 3] = ["tokenizer.json", "model.safetensors", "config.json"];

#[derive(Debug)]
struct DefaultModelCacheInputs {
    devo_home: std::io::Result<PathBuf>,
    temp_dir: PathBuf,
}

/// Embeds code-search documents and queries into dense vectors.
///
/// Implementations must return exactly one vector for each input text, keep vector
/// dimensionality stable for the lifetime of a provider, and return normalized or
/// otherwise cosine-compatible vectors.
pub trait EmbeddingProvider: Send + Sync {
    /// Stable model identity used for cache invalidation.
    fn model_id(&self) -> &str;
    /// Embeds each supplied text into one dense vector.
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, CodeSearchError>;
}

/// Lazy model2vec provider for production dense retrieval.
///
/// The model is loaded behind a mutex on first embedding call. Keeping it lazy
/// avoids downloading or deserializing model files for validation paths that
/// return before indexing.
pub struct Model2VecEmbeddingProvider {
    model_id: String,
    model_dir: PathBuf,
    network_proxy: NetworkProxyConfig,
    model: Mutex<Option<Model2Vec>>,
}

impl Model2VecEmbeddingProvider {
    /// Creates a provider that uses Devo's default local model cache.
    pub fn default_cached() -> Self {
        Self::default_cached_with_network_proxy(NetworkProxyConfig::default())
    }

    /// Creates a provider that uses Devo's default local model cache and HTTP policy.
    pub fn default_cached_with_network_proxy(network_proxy: NetworkProxyConfig) -> Self {
        let model_dir = default_model_cache_dir();
        Self {
            model_id: DEFAULT_MODEL_ID.to_string(),
            model_dir,
            network_proxy,
            model: Mutex::new(None),
        }
    }

    /// Ensures required model files exist and loads model2vec state.
    ///
    /// `normalize_from_config` is left to the model config so the provider stays
    /// aligned with upstream model metadata.
    fn load_model(&self) -> Result<Model2Vec, CodeSearchError> {
        ensure_model_files(&self.model_dir, &self.network_proxy)?;
        let normalize_from_config = None;
        let subdir = None;
        Model2Vec::from_pretrained(&self.model_dir, normalize_from_config, subdir)
            .map_err(|error| CodeSearchError::ModelUnavailable(error.to_string()))
    }
}

impl Default for Model2VecEmbeddingProvider {
    fn default() -> Self {
        Self::default_cached()
    }
}

impl EmbeddingProvider for Model2VecEmbeddingProvider {
    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, CodeSearchError> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let mut guard = self
            .model
            .lock()
            .map_err(|_| CodeSearchError::Index("model lock poisoned".to_string()))?;
        if guard.is_none() {
            *guard = Some(self.load_model()?);
        }
        let model = guard
            .as_ref()
            .ok_or_else(|| CodeSearchError::ModelUnavailable("model failed to load".to_string()))?;
        let matrix = model
            .encode(texts)
            .map_err(|error| CodeSearchError::ModelUnavailable(error.to_string()))?;
        Ok(matrix.rows().into_iter().map(|row| row.to_vec()).collect())
    }
}

/// Deterministic embedding provider used by tests.
///
/// Hash embeddings are not semantically meaningful, but they preserve the core
/// invariants the index relies on: stable dimensions, one row per text, and
/// cosine-compatible normalization.
#[derive(Debug)]
pub struct HashEmbeddingProvider {
    model_id: String,
    dimensions: usize,
}

impl HashEmbeddingProvider {
    /// Creates a deterministic provider with a fixed vector dimension.
    pub fn new(model_id: impl Into<String>, dimensions: usize) -> Self {
        Self {
            model_id: model_id.into(),
            dimensions,
        }
    }
}

impl EmbeddingProvider for HashEmbeddingProvider {
    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, CodeSearchError> {
        Ok(texts
            .iter()
            .map(|text| hash_embedding(text, self.dimensions))
            .collect())
    }
}

/// Computes cosine similarity, returning zero for incompatible vectors.
///
/// Returning zero keeps retrieval robust when a malformed cache or provider bug
/// slips past earlier validation; callers then simply get no positive semantic
/// hit for that pair.
pub fn cosine_similarity(left: &[f32], right: &[f32]) -> f32 {
    if left.is_empty() || left.len() != right.len() {
        return 0.0;
    }
    let mut dot = 0.0;
    let mut left_norm = 0.0;
    let mut right_norm = 0.0;
    if left.len() == 2 {
        let left_0 = left[0];
        let left_1 = left[1];
        let right_0 = right[0];
        let right_1 = right[1];
        return finish_cosine_similarity(
            left_0 * right_0 + left_1 * right_1,
            left_0 * left_0 + left_1 * left_1,
            right_0 * right_0 + right_1 * right_1,
        );
    }
    if left.len() < 4 {
        for (left_value, right_value) in left.iter().zip(right) {
            dot += left_value * right_value;
            left_norm += left_value * left_value;
            right_norm += right_value * right_value;
        }
        return finish_cosine_similarity(dot, left_norm, right_norm);
    }
    let mut idx = 0usize;
    while idx + 4 <= left.len() {
        let left_0 = left[idx];
        let left_1 = left[idx + 1];
        let left_2 = left[idx + 2];
        let left_3 = left[idx + 3];
        let right_0 = right[idx];
        let right_1 = right[idx + 1];
        let right_2 = right[idx + 2];
        let right_3 = right[idx + 3];
        dot += left_0 * right_0 + left_1 * right_1 + left_2 * right_2 + left_3 * right_3;
        left_norm += left_0 * left_0 + left_1 * left_1 + left_2 * left_2 + left_3 * left_3;
        right_norm += right_0 * right_0 + right_1 * right_1 + right_2 * right_2 + right_3 * right_3;
        idx += 4;
    }
    while idx < left.len() {
        let left_value = left[idx];
        let right_value = right[idx];
        dot += left_value * right_value;
        left_norm += left_value * left_value;
        right_norm += right_value * right_value;
        idx += 1;
    }
    finish_cosine_similarity(dot, left_norm, right_norm)
}

fn finish_cosine_similarity(dot: f32, left_norm: f32, right_norm: f32) -> f32 {
    let denominator = left_norm.sqrt() * right_norm.sqrt();
    if denominator <= f32::EPSILON {
        0.0
    } else {
        dot / denominator
    }
}

/// Downloads any missing model files into the local cache directory.
///
/// The check is all-or-download so a partially populated cache is repaired on
/// demand. If the final files are still missing, callers receive a typed model
/// error that can be shown as a recoverable tool failure.
fn ensure_model_files(
    model_dir: &PathBuf,
    network_proxy: &NetworkProxyConfig,
) -> Result<(), CodeSearchError> {
    if MODEL_FILES
        .iter()
        .all(|file| model_dir.join(file).is_file())
    {
        return Ok(());
    }
    std::fs::create_dir_all(model_dir)?;
    let client = build_hf_client(network_proxy)?;
    let repo = client.model(DEFAULT_MODEL_OWNER, DEFAULT_MODEL_NAME);
    for file in MODEL_FILES {
        repo.download_file()
            .filename(file.to_string())
            .local_dir(model_dir.clone())
            .send()
            .map_err(|error| CodeSearchError::ModelUnavailable(error.to_string()))?;
    }
    if MODEL_FILES
        .iter()
        .all(|file| model_dir.join(file).is_file())
    {
        Ok(())
    } else {
        Err(CodeSearchError::ModelUnavailable(format!(
            "model files for {DEFAULT_MODEL_ID} were not found in {} after download",
            model_dir.display()
        )))
    }
}

fn build_hf_client(network_proxy: &NetworkProxyConfig) -> Result<HFClientSync, CodeSearchError> {
    let Some(proxy_url) = non_empty(network_proxy.proxy_url.as_deref()) else {
        return HFClientSync::new()
            .map_err(|error| CodeSearchError::ModelUnavailable(error.to_string()));
    };

    let no_proxy =
        non_empty(network_proxy.no_proxy.as_deref()).and_then(reqwest_hf::NoProxy::from_string);
    let proxy = reqwest_hf::Proxy::all(proxy_url)
        .map_err(|error| {
            CodeSearchError::ModelUnavailable(format!(
                "invalid Hugging Face proxy URL `{proxy_url}`: {error}"
            ))
        })?
        .no_proxy(no_proxy);
    let http_client = reqwest_hf::Client::builder()
        .proxy(proxy)
        .build()
        .map_err(|error| CodeSearchError::ModelUnavailable(error.to_string()))?;

    HFClient::builder()
        .client(http_client)
        .build_sync()
        .map_err(|error| CodeSearchError::ModelUnavailable(error.to_string()))
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

/// Computes the on-disk directory for the default model.
fn default_model_cache_dir() -> PathBuf {
    default_model_cache_dir_from_inputs(DefaultModelCacheInputs {
        devo_home: find_devo_home(),
        temp_dir: std::env::temp_dir(),
    })
}

fn default_model_cache_dir_from_inputs(inputs: DefaultModelCacheInputs) -> PathBuf {
    let base_dir = inputs
        .devo_home
        .unwrap_or_else(|_| inputs.temp_dir.join(".devo"));
    base_dir
        .join(LOCAL_MODELS_DIR)
        .join(DEFAULT_MODEL_ID.replace('/', "--"))
}

/// Produces deterministic normalized bag-of-token vectors for tests.
fn hash_embedding(text: &str, dimensions: usize) -> Vec<f32> {
    let dimensions = dimensions.max(1);
    let mut vector = vec![0.0; dimensions];
    for token in text
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|token| !token.is_empty())
    {
        let mut hasher = Sha256::new();
        let lowercase;
        let token_bytes = if token.bytes().any(|byte| byte.is_ascii_uppercase()) {
            lowercase = token.to_ascii_lowercase();
            lowercase.as_bytes()
        } else {
            token.as_bytes()
        };
        hasher.update(token_bytes);
        let digest = hasher.finalize();
        let index = u64::from_le_bytes([
            digest[0], digest[1], digest[2], digest[3], digest[4], digest[5], digest[6], digest[7],
        ]) as usize
            % dimensions;
        vector[index] += 1.0;
    }
    let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
    if norm > f32::EPSILON {
        for value in &mut vector {
            *value /= norm;
        }
    }
    vector
}

#[cfg(test)]
mod tests {
    use std::hint::black_box;
    use std::time::Instant;

    use pretty_assertions::assert_eq;

    use super::*;

    /// Trace: L2-DES-TOOL-001
    /// Verifies: test embedding provider returns one stable vector per input.
    #[test]
    fn hash_embedding_provider_is_deterministic() {
        let provider = HashEmbeddingProvider::new("test", 8);
        let texts = vec!["parse input".to_string(), "parse input".to_string()];
        let embeddings = provider.embed(&texts).expect("embed");

        assert_eq!(embeddings.len(), 2);
        assert_eq!(embeddings[0], embeddings[1]);
    }

    /// Trace: L2-DES-TOOL-001
    /// Verifies: dense ranking similarity is cosine-compatible.
    #[test]
    fn cosine_similarity_scores_identical_vectors_highest() {
        assert_eq!(cosine_similarity(&[1.0, 0.0], &[1.0, 0.0]), 1.0);
        assert_eq!(cosine_similarity(&[1.0, 0.0], &[0.0, 1.0]), 0.0);
    }

    #[test]
    #[ignore]
    fn bench_cosine_similarity_512_dimensions() {
        let left = (0..512)
            .map(|idx| (idx as f32 + 1.0) / 512.0)
            .collect::<Vec<_>>();
        let right = (0..512)
            .map(|idx| ((512 - idx) as f32) / 512.0)
            .collect::<Vec<_>>();
        let iterations = 500_000;
        let expected = cosine_similarity(&left, &right);
        let started = Instant::now();
        let mut total = 0.0f64;

        for _ in 0..iterations {
            total += black_box(cosine_similarity(black_box(&left), black_box(&right))) as f64;
        }

        let elapsed = started.elapsed();
        assert!((total / iterations as f64 - f64::from(expected)).abs() < 0.0001);
        println!(
            "cosine_similarity_512_dimensions iterations={iterations} elapsed_ms={} per_call_ns={:.2}",
            elapsed.as_secs_f64() * 1_000.0,
            elapsed.as_secs_f64() * 1_000_000_000.0 / iterations as f64
        );
    }

    #[test]
    fn default_model_cache_dir_uses_resolved_devo_home() {
        let devo_home = PathBuf::from("devo-home");
        let inputs = DefaultModelCacheInputs {
            devo_home: Ok(devo_home.clone()),
            temp_dir: PathBuf::from("temp"),
        };

        assert_eq!(
            default_model_cache_dir_from_inputs(inputs),
            devo_home
                .join(LOCAL_MODELS_DIR)
                .join("minishlab--potion-code-16M")
        );
    }

    #[test]
    fn default_model_cache_dir_falls_back_to_temp_when_devo_home_is_unavailable() {
        let temp_dir = PathBuf::from("temp");
        let inputs = DefaultModelCacheInputs {
            devo_home: Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "home unavailable",
            )),
            temp_dir: temp_dir.clone(),
        };

        assert_eq!(
            default_model_cache_dir_from_inputs(inputs),
            temp_dir
                .join(".devo")
                .join(LOCAL_MODELS_DIR)
                .join("minishlab--potion-code-16M")
        );
    }

    #[test]
    fn hf_client_builder_accepts_configured_proxy_and_no_proxy() {
        let config = devo_network_proxy::NetworkProxyConfig {
            proxy_url: Some("socks5h://127.0.0.1:7890".to_string()),
            no_proxy: Some("localhost,127.0.0.1,::1".to_string()),
        };

        build_hf_client(&config).expect("build proxied hf client");
    }

    #[test]
    fn hf_client_builder_rejects_invalid_proxy_url() {
        let config = devo_network_proxy::NetworkProxyConfig {
            proxy_url: Some("not a proxy url".to_string()),
            no_proxy: None,
        };

        let error = build_hf_client(&config).expect_err("invalid proxy should fail");

        assert!(error.to_string().contains("invalid Hugging Face proxy URL"));
    }
}
