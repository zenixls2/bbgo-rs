//! Backend-neutral feature, inference, training and artifact contracts.
//! No strategy, ML framework, HTTP client or Python runtime is selected here.
pub mod training;
use async_trait::async_trait;
use bbgo_core::recovery::{CheckpointParticipant, ComponentCheckpoint};
use bbgo_types::{Error, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

fn invalid(message: &str) -> Error {
    Error::Invalid(message.into())
}
fn identifier(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256
}

/// Ordered dense numeric features/outputs. Fields are scalar f64 for this v1
/// contract; tensor/text/image protocols belong in separately versioned adapters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NumericSchema {
    pub id: String,
    pub version: u32,
    pub fields: Vec<String>,
}
impl NumericSchema {
    pub fn validate(&self) -> Result<()> {
        let names: std::collections::BTreeSet<_> = self.fields.iter().collect();
        if !identifier(&self.id)
            || self.version == 0
            || self.fields.is_empty()
            || self.fields.len() > 65536
            || names.len() != self.fields.len()
            || self.fields.iter().any(|name| !identifier(name))
        {
            return Err(invalid("invalid numeric schema"));
        }
        Ok(())
    }
    pub fn sha256(&self) -> Result<[u8; 32]> {
        self.validate()?;
        // Ordered fields + struct layout, no unordered maps.
        Ok(Sha256::digest(serde_json::to_vec(self).map_err(|e| invalid(&e.to_string()))?).into())
    }
    pub fn validate_values(&self, values: &[f64]) -> Result<()> {
        self.validate()?;
        if values.len() != self.fields.len() || values.iter().any(|value| !value.is_finite()) {
            return Err(invalid("numeric shape mismatch or nonfinite value"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactRef {
    /// Stable content-addressed repository identity, not a credential-bearing URL.
    pub id: String,
    pub sha256: [u8; 32],
    pub bytes: u64,
    /// Examples: application/onnx, application/json, application/octet-stream.
    pub media_type: String,
}
impl ArtifactRef {
    pub fn validate(&self) -> Result<()> {
        if !identifier(&self.id) || !identifier(&self.media_type) {
            return Err(invalid("invalid artifact identity"));
        }
        Ok(())
    }
    pub fn verify(&self, bytes: &[u8], max_bytes: usize) -> Result<()> {
        self.validate()?;
        if bytes.len() > max_bytes
            || self.bytes != bytes.len() as u64
            || self.sha256 != <[u8; 32]>::from(Sha256::digest(bytes))
        {
            return Err(invalid("artifact size or digest mismatch"));
        }
        Ok(())
    }
}
#[async_trait]
pub trait ArtifactStore: Send + Sync {
    /// Must enforce the limit BEFORE allocating/reading the whole object and
    /// verify digest/size. Never execute artifact contents here.
    async fn get(&self, artifact: &ArtifactRef, max_bytes: usize) -> Result<Vec<u8>>;
    /// Immutable content-addressed insertion; conflicting reuse must fail.
    async fn put(&self, media_type: &str, bytes: &[u8]) -> Result<ArtifactRef>;
}

/// Caller records BOTH market observation and first feature availability times.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FeatureRow {
    pub id: String,
    pub observed_at_ns: i64,
    pub available_at_ns: i64,
    pub values: Vec<f64>,
}
impl FeatureRow {
    pub fn validate(&self, schema: &NumericSchema, as_of_ns: i64) -> Result<()> {
        schema.validate_values(&self.values)?;
        if !identifier(&self.id)
            || self.observed_at_ns < 0
            || self.available_at_ns < self.observed_at_ns
            || self.available_at_ns > as_of_ns
        {
            return Err(invalid(
                "invalid feature identity or future/unavailable data",
            ));
        }
        Ok(())
    }
}

/// Optional feature-transform plugin. The caller supplies actual availability
/// nanoseconds from the source envelope (not reconstructed millisecond fields).
#[async_trait]
pub trait FeatureExtractor: CheckpointParticipant {
    fn schema(&self) -> &NumericSchema;
    async fn on_market_event(
        &mut self,
        event: &bbgo_types::MarketEvent,
        available_at_ns: i64,
        cancel: &CancellationToken,
    ) -> Result<Vec<FeatureRow>>;
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InferenceRequest {
    pub request_id: String,
    pub as_of_ns: i64,
    pub schema_sha256: [u8; 32],
    pub rows: Vec<FeatureRow>,
}
impl InferenceRequest {
    pub fn validate(&self, schema: &NumericSchema, max_rows: usize) -> Result<()> {
        if !identifier(&self.request_id)
            || self.as_of_ns < 0
            || self.rows.is_empty()
            || self.rows.len() > max_rows
            || self.schema_sha256 != schema.sha256()?
        {
            return Err(invalid("invalid inference request/schema/row limit"));
        }
        let mut ids = std::collections::BTreeSet::new();
        for row in &self.rows {
            row.validate(schema, self.as_of_ns)?;
            if !ids.insert(&row.id) {
                return Err(invalid("duplicate inference row identity"));
            }
        }
        Ok(())
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Prediction {
    pub row_id: String,
    pub values: Vec<f64>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InferenceResponse {
    pub request_id: String,
    pub artifact: ArtifactRef,
    pub output_schema_sha256: [u8; 32],
    pub predictions: Vec<Prediction>,
}
impl InferenceResponse {
    pub fn validate(&self, request: &InferenceRequest, spec: &ModelSpec) -> Result<()> {
        spec.validate_request(request)?;
        if self.request_id != request.request_id
            || self.artifact != spec.artifact
            || self.output_schema_sha256 != spec.outputs.sha256()?
            || self.predictions.len() != request.rows.len()
        {
            return Err(invalid("inference response identity/schema mismatch"));
        }
        for (prediction, row) in self.predictions.iter().zip(&request.rows) {
            if prediction.row_id != row.id {
                return Err(invalid("prediction ordering mismatch"));
            }
            spec.outputs.validate_values(&prediction.values)?;
        }
        Ok(())
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelSpec {
    pub provider_id: String,
    pub runtime_version: String,
    pub artifact: ArtifactRef,
    pub inputs: NumericSchema,
    pub outputs: NumericSchema,
    pub max_batch_rows: usize,
    /// Latest information used in training, INCLUDING label availability.
    /// None is reserved for non-trained models; unknown training provenance
    /// must not be represented as None by an adapter.
    pub training_information_through_ns: Option<i64>,
    /// Non-secret, provider-specific settings. Credentials are injected out of band.
    pub parameters: serde_json::Value,
}
impl ModelSpec {
    pub fn validate(&self) -> Result<()> {
        self.artifact.validate()?;
        self.inputs.validate()?;
        self.outputs.validate()?;
        if !identifier(&self.provider_id)
            || !identifier(&self.runtime_version)
            || self.max_batch_rows == 0
            || self.max_batch_rows > 1_000_000
            || self
                .training_information_through_ns
                .is_some_and(|at| at < 0)
        {
            return Err(invalid("invalid model specification"));
        }
        Ok(())
    }
    pub fn validate_request(&self, request: &InferenceRequest) -> Result<()> {
        self.validate()?;
        request.validate(&self.inputs, self.max_batch_rows)?;
        if self
            .training_information_through_ns
            .is_some_and(|at| at > request.as_of_ns)
        {
            return Err(invalid("model trained on information after inference time"));
        }
        Ok(())
    }
}
/// Local ONNX/Torch, remote services and native Rust models implement this same
/// boundary. Stateful adapters serialize access and checkpoint their RNG/state.
#[async_trait]
pub trait InferenceModel: CheckpointParticipant {
    fn spec(&self) -> &ModelSpec;
    /// Validate inputs and responses; honor cancellation without publishing
    /// partial state. Calling applications must validate untrusted responses too.
    async fn infer(
        &mut self,
        request: &InferenceRequest,
        cancel: &CancellationToken,
    ) -> Result<InferenceResponse>;
    /// Release worker/session resources; idempotent. No further inference may
    /// succeed after close. Applications call this explicitly on every exit path.
    async fn close(&mut self) -> Result<()>;
}
#[async_trait]
pub trait ModelProvider: Send + Sync {
    fn provider_id(&self) -> &str;
    /// Stage a NEW model; verify artifact and exact checkpoint compatibility.
    /// Stateless adapters checkpoint configuration/artifact identity, not weights.
    async fn load(
        &self,
        spec: &ModelSpec,
        artifacts: &dyn ArtifactStore,
        checkpoint: Option<&ComponentCheckpoint>,
        cancel: &CancellationToken,
    ) -> Result<Box<dyn InferenceModel>>;
}
