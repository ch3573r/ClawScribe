//! The official E5 ONNX CPU baseline. No network access occurs during inference.
use super::{
    model::{VerifiedModel, PINS},
    types::{EmbeddingPurpose, EmbeddingSpace, KnowledgeError},
};
use ort::{
    execution_providers::CPUExecutionProvider,
    session::{builder::GraphOptimizationLevel, Session},
    value::Tensor,
};
use tokenizers::Tokenizer;

pub trait EmbeddingBackend: Send {
    fn space(&self) -> EmbeddingSpace;
    fn embed(&mut self, text: &str, purpose: EmbeddingPurpose) -> Result<Vec<f32>, KnowledgeError>;
    fn spans(&self, _id: &str, _text: &str) -> Result<Vec<super::types::TextSpan>, KnowledgeError> {
        Err(KnowledgeError::ModelUnavailable)
    }
}

pub fn prefixed_input(text: &str, purpose: EmbeddingPurpose) -> Result<String, KnowledgeError> {
    // Bound allocation before tokenization as well as the resulting token count.
    let limit = match purpose {
        EmbeddingPurpose::Query => 1024,
        EmbeddingPurpose::Passage => 16 * 1024,
    };
    if text.trim().is_empty() || text.len() > limit {
        return Err(KnowledgeError::InvalidInput);
    }
    let prefix = match purpose {
        EmbeddingPurpose::Query => "query: ",
        EmbeddingPurpose::Passage => "passage: ",
    };
    Ok(format!("{prefix}{text}"))
}

pub fn normalize(mut vector: Vec<f32>) -> Result<Vec<f32>, KnowledgeError> {
    if vector.len() != 384 || vector.iter().any(|v| !v.is_finite()) {
        return Err(KnowledgeError::InvalidInput);
    }
    let norm = vector
        .iter()
        .map(|v| (*v as f64).powi(2))
        .sum::<f64>()
        .sqrt();
    if !norm.is_finite() || norm <= f64::EPSILON {
        return Err(KnowledgeError::InvalidInput);
    }
    for v in &mut vector {
        *v = (*v as f64 / norm) as f32;
    }
    Ok(vector)
}

fn masked_mean(hidden: &[f32], mask: &[i64]) -> Result<Vec<f32>, KnowledgeError> {
    if mask.is_empty()
        || hidden.len() != mask.len() * 384
        || mask.iter().any(|v| !matches!(v, 0 | 1))
    {
        return Err(KnowledgeError::InvalidInput);
    }
    let mut sum = vec![0.; 384];
    let mut count = 0;
    for (row, present) in hidden.chunks_exact(384).zip(mask) {
        if *present == 1 {
            count += 1;
            for (total, value) in sum.iter_mut().zip(row) {
                *total += value;
            }
        }
    }
    if count == 0 {
        return Err(KnowledgeError::InvalidInput);
    }
    for value in &mut sum {
        *value /= count as f32;
    }
    normalize(sum)
}

pub struct OnnxEmbedding {
    session: Session,
    tokenizer: Tokenizer,
}
impl OnnxEmbedding {
    // Called only on the scheduler's blocking worker after artifact verification.
    pub fn load(model: &VerifiedModel) -> Result<Self, KnowledgeError> {
        #[cfg(test)]
        acceptance::trace_load_phase("before_session");
        let session = (|| -> ort::Result<Session> {
            Session::builder()?
                .with_execution_providers([CPUExecutionProvider::default()
                    .with_arena_allocator(false)
                    .build()])?
                // Keep the approved float32 artifact without graph rewrites or
                // extra packed-weight copies. ORT's documented config uses 1
                // to disable prepacking (not a disabled-optimization synonym).
                .with_optimization_level(GraphOptimizationLevel::Disable)?
                .with_config_entry("session.disable_prepacking", "1")?
                .with_intra_threads(2)?
                .with_inter_threads(1)?
                .with_parallel_execution(false)?
                .with_memory_pattern(false)?
                .with_config_entry("session.intra_op.allow_spinning", "0")?
                .commit_from_file(model.root().join("onnx/model.onnx"))
        })()
        .map_err(|_| KnowledgeError::ModelUnavailable)?;
        let names: Vec<_> = session.inputs.iter().map(|i| i.name.as_str()).collect();
        if names != ["input_ids", "attention_mask", "token_type_ids"]
            || !session
                .outputs
                .iter()
                .any(|o| o.name == "last_hidden_state")
        {
            return Err(KnowledgeError::ModelUnavailable);
        }
        #[cfg(test)]
        acceptance::trace_load_phase("after_session");
        // Session construction temporarily retains protobuf/initializer data.
        // Do not overlap that phase with the resident tokenizer allocation.
        #[cfg(test)]
        acceptance::trace_load_phase("before_tokenizer");
        let tokenizer = Tokenizer::from_file(model.root().join("tokenizer.json"))
            .map_err(|_| KnowledgeError::ModelUnavailable)?;
        #[cfg(test)]
        acceptance::trace_load_phase("after_tokenizer");
        Ok(Self { session, tokenizer })
    }
    pub fn encode(
        &self,
        text: &str,
        purpose: EmbeddingPurpose,
    ) -> Result<tokenizers::Encoding, KnowledgeError> {
        let encoded = self
            .tokenizer
            .encode(prefixed_input(text, purpose)?, true)
            .map_err(|_| KnowledgeError::InvalidInput)?;
        // Chunking owns the separate 320 body-token budget. Prefix/special
        // tokens count against the model input limit, for both purposes.
        let limit = 512;
        if encoded.is_empty() || encoded.len() > limit {
            return Err(KnowledgeError::InvalidInput);
        }
        Ok(encoded)
    }
}
impl EmbeddingBackend for OnnxEmbedding {
    fn spans(&self, id: &str, text: &str) -> Result<Vec<super::types::TextSpan>, KnowledgeError> {
        super::chunking::semantic_spans(&self.tokenizer, id, text)
    }
    fn space(&self) -> EmbeddingSpace {
        PINS.space()
    }
    fn embed(&mut self, text: &str, purpose: EmbeddingPurpose) -> Result<Vec<f32>, KnowledgeError> {
        let encoded = self.encode(text, purpose)?;
        let mask: Vec<i64> = encoded
            .get_attention_mask()
            .iter()
            .map(|v| *v as i64)
            .collect();
        let ids: Vec<i64> = encoded.get_ids().iter().map(|v| *v as i64).collect();
        let types: Vec<i64> = encoded.get_type_ids().iter().map(|v| *v as i64).collect();
        let shape = [1, encoded.len()];
        let infer = (|| -> ort::Result<Vec<f32>> {
            let outputs = self.session.run(ort::inputs![
                "input_ids" => Tensor::from_array((shape, ids))?,
                "attention_mask" => Tensor::from_array((shape, mask.clone()))?,
                "token_type_ids" => Tensor::from_array((shape, types))?
            ])?;
            let output = outputs
                .get("last_hidden_state")
                .ok_or_else(|| ort::Error::new("Missing embedding output"))?;
            let (dims, values) = output.try_extract_tensor::<f32>()?;
            if dims.as_ref() != [1, encoded.len() as i64, 384] {
                return Err(ort::Error::new("Invalid embedding shape"));
            }
            Ok(values.to_vec())
        })()
        .map_err(|_| KnowledgeError::ProviderFailure)?;
        masked_mean(&infer, &mask)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pooling_ignores_padding_and_rejects_empty_mask() {
        let mut hidden = vec![0.; 3 * 384];
        hidden[0] = 3.;
        hidden[384 + 1] = 4.;
        hidden[768] = f32::NAN;
        let result = masked_mean(&hidden, &[1, 1, 0]).unwrap();
        assert!((result[0] - 0.6).abs() < 1e-6);
        assert!((result[1] - 0.8).abs() < 1e-6);
        assert!(masked_mean(&hidden, &[0, 0, 0]).is_err());
        assert!(masked_mean(&hidden, &[1]).is_err());
    }
    #[test]
    fn rejects_bad_dimensions_and_nonfinite_vectors() {
        assert!(normalize(vec![1.; 383]).is_err());
        assert!(normalize(vec![f32::NAN; 384]).is_err());
        assert!(normalize(vec![f32::INFINITY; 384]).is_err());
        assert!(normalize(vec![0.; 384]).is_err());
        let v = normalize(vec![1.; 384]).unwrap();
        assert_eq!(v.len(), 384);
        assert!(v.iter().all(|x| x.is_finite()));
        assert!((v.iter().map(|x| x * x).sum::<f32>() - 1.).abs() < 1e-5);
    }
    #[test]
    fn query_and_passage_prefixes() {
        assert_eq!(
            prefixed_input("Ja.", EmbeddingPurpose::Query).unwrap(),
            "query: Ja."
        );
        assert_eq!(
            prefixed_input("Ja.", EmbeddingPurpose::Passage).unwrap(),
            "passage: Ja."
        );
        assert!(prefixed_input("  ", EmbeddingPurpose::Query).is_err());
        assert!(prefixed_input(&"ä".repeat(513), EmbeddingPurpose::Query).is_err());
    }
}

#[cfg(test)]
mod acceptance {
    use super::*;
    use crate::knowledge::{
        model::{ModelDownloads, VerifiedModel, LOADING_POLICY},
        scheduler::{run_indexing, CancellationRegistry},
    };
    use std::{
        path::PathBuf,
        sync::{
            atomic::{AtomicBool, AtomicUsize, Ordering},
            Arc, Mutex,
        },
        time::{Duration, Instant},
    };
    // Test-only diagnostics: no model paths, input text, or production logging.
    pub(super) fn trace_load_phase(stage: &str) {
        if std::env::var_os("CLAWSCRIBE_KNOWLEDGE_REFERENCE").is_some() {
            if let Some(memory) = memory_stats::memory_stats() {
                println!(
                    "KNOWLEDGE_PHASE stage={stage} rss_bytes={} private_bytes={}",
                    memory.physical_mem, memory.virtual_mem
                );
            }
        }
    }

    #[derive(serde::Deserialize)]
    struct Reference {
        purpose: String,
        text: String,
        ids: Vec<u32>,
        vector: Vec<f32>,
    }

    #[tokio::test]
    #[ignore = "manual trusted-runner model download and resource gate"]
    async fn pinned_onnx_reference_and_resource_gate() {
        let _serial = crate::audio::inference::GLOBAL_JOB_TEST_LOCK.lock().await;
        let root = PathBuf::from(
            std::env::var_os("CLAWSCRIBE_KNOWLEDGE_MODEL").expect("manual model directory"),
        );
        let reference = PathBuf::from(
            std::env::var_os("CLAWSCRIBE_KNOWLEDGE_REFERENCE").expect("independent reference file"),
        );
        ModelDownloads::default()
            .download(&root, tokio_util::sync::CancellationToken::new())
            .await
            .unwrap();
        let verified = VerifiedModel::verify(&root).await.unwrap();
        let fixtures: Vec<Reference> =
            serde_json::from_slice(&std::fs::read(reference).unwrap()).unwrap();
        let baseline = memory_stats::memory_stats().unwrap();
        println!(
            "KNOWLEDGE_BASELINE rss_bytes={} private_bytes={}",
            baseline.physical_mem, baseline.virtual_mem
        );
        let peak_rss = Arc::new(AtomicUsize::new(baseline.physical_mem));
        let peak_private = Arc::new(AtomicUsize::new(baseline.virtual_mem));
        let monitoring = Arc::new(AtomicBool::new(true));
        let monitor = {
            let rss = peak_rss.clone();
            let private = peak_private.clone();
            let running = monitoring.clone();
            std::thread::spawn(move || {
                while running.load(Ordering::Acquire) {
                    if let Some(memory) = memory_stats::memory_stats() {
                        rss.fetch_max(memory.physical_mem, Ordering::AcqRel);
                        private.fetch_max(memory.virtual_mem, Ordering::AcqRel);
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
            })
        };
        let registry = Arc::new(CancellationRegistry::default());
        let model: Arc<Mutex<Option<OnnxEmbedding>>> = Arc::new(Mutex::new(None));
        let loaded = model.clone();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let cold = tokio::spawn(run_indexing(registry.clone(), move || {
            let _ = started_tx.send(());
            *loaded.lock().unwrap() = Some(OnnxEmbedding::load(&verified)?);
            Ok(())
        }));
        started_rx.await.unwrap();
        let started = Instant::now();
        let cold_priority =
            crate::audio::inference::claim_job_preempting_local_summary("knowledge acceptance")
                .await;
        let cold_preemption = started.elapsed();
        let cold_acquired = cold_priority.is_ok();
        drop(cold_priority);
        let _ = cold.await;
        let cold_memory = memory_stats::memory_stats().unwrap();
        println!("KNOWLEDGE_LOAD policy={LOADING_POLICY} peak_rss_delta_bytes={} peak_private_delta_bytes={} resident_rss_delta_bytes={} resident_private_delta_bytes={}",
            peak_rss.load(Ordering::Acquire).saturating_sub(baseline.physical_mem),
            peak_private.load(Ordering::Acquire).saturating_sub(baseline.virtual_mem),
            cold_memory.physical_mem.saturating_sub(baseline.physical_mem),
            cold_memory.virtual_mem.saturating_sub(baseline.virtual_mem));
        let mut max_error = 0.0f32;
        let mut min_cosine = 1.0f32;
        let mut max_warm = Duration::ZERO;
        for item in fixtures {
            let loaded = model.clone();
            let purpose = if item.purpose == "query" {
                EmbeddingPurpose::Query
            } else {
                EmbeddingPurpose::Passage
            };
            let started = Instant::now();
            let (ids, actual) = run_indexing(registry.clone(), move || {
                let mut slot = loaded.lock().unwrap();
                let runtime = slot.as_mut().unwrap();
                let ids = runtime.encode(&item.text, purpose)?.get_ids().to_vec();
                let actual = runtime.embed(&item.text, purpose)?;
                Ok((ids, actual))
            })
            .await
            .unwrap();
            max_warm = max_warm.max(started.elapsed());
            assert_eq!(ids, item.ids, "tokenizer parity");
            assert_eq!(actual.len(), 384);
            let norm = actual.iter().map(|v| v * v).sum::<f32>();
            assert!((norm - 1.).abs() < 1e-5);
            let cosine = actual
                .iter()
                .zip(&item.vector)
                .map(|(a, b)| a * b)
                .sum::<f32>();
            min_cosine = min_cosine.min(cosine);
            for (a, b) in actual.iter().zip(item.vector) {
                max_error = max_error.max((a - b).abs());
            }
        }
        println!("KNOWLEDGE_PARITY fixtures=6 max_error={max_error:.8} min_cosine={min_cosine:.8} warm_max_ms={} cold_preemption_ms={} cold_acquired={cold_acquired}", max_warm.as_millis(), cold_preemption.as_millis());
        // Count with the pinned tokenizer: the prefix need not be two tokens.
        // This constructs a worst-size valid query rather than assuming a
        // vocabulary-dependent word/token ratio.
        let longest_query = {
            let slot = model.lock().unwrap();
            let runtime = slot.as_ref().unwrap();
            (1..=508)
                .rev()
                .find_map(|count| {
                    let text = "a ".repeat(count);
                    runtime
                        .encode(&text, EmbeddingPurpose::Query)
                        .ok()
                        .filter(|encoded| encoded.len() >= 500)
                        .map(|encoded| (text, encoded.len()))
                })
                .expect("near-512-token query within 1024 UTF-8 bytes")
        };
        println!(
            "KNOWLEDGE_INPUT query_tokens={} query_bytes={}",
            longest_query.1,
            longest_query.0.len()
        );
        let mut max_preemption = Duration::ZERO;
        let mut all_acquired = true;
        for n in 0..10 {
            let loaded = model.clone();
            let (started_tx, started_rx) = tokio::sync::oneshot::channel();
            let query = longest_query.0.clone();
            let task = tokio::spawn(run_indexing(registry.clone(), move || {
                let mut slot = loaded.lock().unwrap();
                let runtime = slot.as_mut().unwrap();
                let (input, purpose) = if n % 2 == 0 {
                    (query, EmbeddingPurpose::Query)
                } else {
                    ("garden ".repeat(320), EmbeddingPurpose::Passage)
                };
                let encoded = runtime.encode(&input, purpose)?;
                assert!(encoded.len() >= 315, "exercise near-boundary native input");
                let _ = started_tx.send(());
                runtime.embed(&input, purpose)
            }));
            started_rx.await.unwrap();
            let started = Instant::now();
            let foreground =
                crate::audio::inference::claim_job_preempting_local_summary("knowledge acceptance")
                    .await;
            max_preemption = max_preemption.max(started.elapsed());
            all_acquired &= foreground.is_ok();
            drop(foreground);
            assert!(matches!(
                task.await.unwrap(),
                Err(KnowledgeError::Cancelled)
            ));
        }
        monitoring.store(false, Ordering::Release);
        monitor.join().unwrap();
        let rss_delta = peak_rss
            .load(Ordering::Acquire)
            .saturating_sub(baseline.physical_mem);
        let private_delta = peak_private
            .load(Ordering::Acquire)
            .saturating_sub(baseline.virtual_mem);
        println!("KNOWLEDGE_GATE policy={LOADING_POLICY} model=multilingual-e5-small revision={} engine=onnx backend=cpu intra_threads=2 batch=1 dimensions=384 max_error={max_error:.8} min_cosine={min_cosine:.8} warm_max_ms={} cold_preemption_ms={} warm_preemption_max_ms={} peak_rss_delta_bytes={rss_delta} peak_private_delta_bytes={private_delta}", PINS.revision, max_warm.as_millis(), cold_preemption.as_millis(), max_preemption.as_millis());
        assert!(
            max_error <= 0.0001 && min_cosine >= 0.9999,
            "independent reference parity failed"
        );
        assert!(
            rss_delta <= 1024 * 1024 * 1024 && private_delta <= 1024 * 1024 * 1024,
            "incremental worker memory exceeds 1 GiB"
        );
        assert!(
            max_warm <= Duration::from_secs(2),
            "warm input exceeds two seconds"
        );
        assert!(
            all_acquired && max_preemption <= Duration::from_secs(2),
            "warm recording preemption exceeds two seconds"
        );
        assert!(
            cold_acquired && cold_preemption <= Duration::from_secs(2),
            "cold recording preemption exceeds two seconds"
        );
    }
}
