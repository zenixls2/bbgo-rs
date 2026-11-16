//! Dataset provenance and asynchronous external training-job interface.
use crate::{ArtifactRef, FeatureRow, NumericSchema, identifier, invalid};
use async_trait::async_trait;
use bbgo_types::Result;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DatasetRef {
    pub id: String,
    /// Digest of an immutable manifest (source snapshots, transforms and labels).
    pub manifest_sha256: [u8; 32],
    pub feature_schema_sha256: [u8; 32],
    pub target_schema_sha256: [u8; 32],
    pub from_ns: i64,
    pub through_ns: i64,
    /// Includes future label horizons; train/validation purging uses THIS bound.
    pub labels_available_through_ns: i64,
}
impl DatasetRef {
    pub fn validate(&self) -> Result<()> {
        if !identifier(&self.id)
            || self.from_ns < 0
            || self.through_ns < self.from_ns
            || self.labels_available_through_ns < self.through_ns
        {
            return Err(invalid("invalid dataset identity or bounds"));
        }
        Ok(())
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrainingExample {
    pub features: FeatureRow,
    pub targets: Vec<f64>,
    pub label_available_at_ns: i64,
}
impl TrainingExample {
    pub fn validate(
        &self,
        dataset: &DatasetRef,
        inputs: &NumericSchema,
        targets: &NumericSchema,
    ) -> Result<()> {
        dataset.validate()?;
        if dataset.feature_schema_sha256 != inputs.sha256()?
            || dataset.target_schema_sha256 != targets.sha256()?
        {
            return Err(invalid("training schema mismatch"));
        }
        self.features.validate(inputs, dataset.through_ns)?;
        targets.validate_values(&self.targets)?;
        if self.features.observed_at_ns < dataset.from_ns
            || self.label_available_at_ns < self.features.available_at_ns
            || self.label_available_at_ns > dataset.labels_available_through_ns
        {
            return Err(invalid("training example outside dataset/label bounds"));
        }
        Ok(())
    }
}
#[async_trait]
pub trait DatasetReader: Send {
    fn dataset(&self) -> &DatasetRef;
    /// Bounded streaming batches. None is EOF; adapters reject empty batches,
    /// schema/provenance drift and oversized data rather than silently dropping.
    async fn next_batch(
        &mut self,
        max_rows: usize,
        cancel: &CancellationToken,
    ) -> Result<Option<Vec<TrainingExample>>>;
    async fn close(&mut self) -> Result<()>;
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrainingRequest {
    pub request_id: String,
    pub trainer_id: String,
    pub training: DatasetRef,
    pub validation: Option<DatasetRef>,
    pub inputs: NumericSchema,
    pub targets: NumericSchema,
    pub initial_model: Option<ArtifactRef>,
    pub seed: u64,
    pub parameters: serde_json::Value,
}
impl TrainingRequest {
    pub fn validate(&self) -> Result<()> {
        if !identifier(&self.request_id) || !identifier(&self.trainer_id) {
            return Err(invalid("invalid training identity"));
        }
        let inputs = self.inputs.sha256()?;
        let targets = self.targets.sha256()?;
        for dataset in std::iter::once(&self.training).chain(self.validation.iter()) {
            dataset.validate()?;
            if dataset.feature_schema_sha256 != inputs || dataset.target_schema_sha256 != targets {
                return Err(invalid("training dataset schema mismatch"));
            }
        }
        if self.validation.as_ref().is_some_and(|validation| {
            self.training.labels_available_through_ns >= validation.from_ns
        }) {
            return Err(invalid(
                "validation overlaps training features or label horizon",
            ));
        }
        if let Some(artifact) = &self.initial_model {
            artifact.validate()?;
        }
        Ok(())
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum TrainingStatus {
    Queued,
    Running,
    Succeeded {
        artifact: ArtifactRef,
        metrics: std::collections::BTreeMap<String, f64>,
    },
    Failed {
        message: String,
    },
    Cancelled,
}
impl TrainingStatus {
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Succeeded { artifact, metrics } => {
                artifact.validate()?;
                if metrics.len() > 10000
                    || metrics
                        .iter()
                        .any(|(name, value)| !identifier(name) || !value.is_finite())
                {
                    return Err(invalid("invalid training metrics"));
                }
            }
            Self::Failed { message } if message.is_empty() || message.len() > 4096 => {
                return Err(invalid("invalid training failure message"));
            }
            _ => {}
        }
        Ok(())
    }
}
/// Long-running jobs are not tied to an inference runtime or trading strategy.
#[async_trait]
pub trait TrainingBackend: Send + Sync {
    fn trainer_id(&self) -> &str;
    /// Request ID is an idempotency key: identical retries return the same job;
    /// conflicting reuse fails. Token cancels transport, NOT an accepted remote job.
    async fn submit(&self, request: &TrainingRequest, cancel: &CancellationToken)
    -> Result<String>;
    async fn status(&self, job_id: &str, cancel: &CancellationToken) -> Result<TrainingStatus>;
    /// Explicit remote-job cancellation, idempotent; unsupported providers fail.
    async fn cancel_job(&self, job_id: &str) -> Result<()>;
}
