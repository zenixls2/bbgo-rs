use async_trait::async_trait;
use bbgo_core::recovery::{
    CheckpointFactory, CheckpointParticipant, ComponentCheckpoint, StateDescriptor,
};
use bbgo_model::{training::*, *};
use bbgo_types::{Error, Result};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

fn schema() -> NumericSchema {
    NumericSchema {
        id: "numeric-v1".into(),
        version: 1,
        fields: vec!["midpoint".into()],
    }
}
fn descriptor() -> StateDescriptor {
    StateDescriptor {
        component_id: "test-model".into(),
        format: "counter-json".into(),
        version: 1,
        configuration_sha256: [7; 32],
    }
}
fn artifact() -> ArtifactRef {
    ArtifactRef {
        id: "constant-v1".into(),
        bytes: 1,
        sha256: Sha256::digest([42]).into(),
        media_type: "application/octet-stream".into(),
    }
}
fn spec() -> ModelSpec {
    ModelSpec {
        provider_id: "test-provider".into(),
        runtime_version: "1".into(),
        artifact: artifact(),
        inputs: schema(),
        outputs: schema(),
        max_batch_rows: 10,
        training_information_through_ns: Some(5),
        parameters: serde_json::Value::Null,
    }
}
fn request() -> InferenceRequest {
    InferenceRequest {
        request_id: "request-1".into(),
        as_of_ns: 10,
        schema_sha256: schema().sha256().unwrap(),
        rows: vec![FeatureRow {
            id: "row-1".into(),
            observed_at_ns: 8,
            available_at_ns: 9,
            values: vec![100.],
        }],
    }
}

#[test]
fn schema_timing_shape_and_checkpoint_guards() {
    let good = request();
    spec().validate_request(&good).unwrap();
    let mut bad = good.clone();
    bad.rows[0].available_at_ns = 11;
    assert!(spec().validate_request(&bad).is_err());
    bad = good.clone();
    bad.rows[0].values[0] = f64::NAN;
    assert!(spec().validate_request(&bad).is_err());
    bad = good.clone();
    bad.rows.push(bad.rows[0].clone());
    assert!(spec().validate_request(&bad).is_err());
    bad = good.clone();
    bad.schema_sha256 = [0; 32];
    assert!(spec().validate_request(&bad).is_err());
    let mut future = spec();
    future.training_information_through_ns = Some(11);
    assert!(future.validate_request(&good).is_err());
    let mut duplicate = schema();
    duplicate.fields.push("midpoint".into());
    assert!(duplicate.validate().is_err());
    let checkpoint = ComponentCheckpoint::new(descriptor(), b"0".to_vec(), 10).unwrap();
    checkpoint.validate(&descriptor(), 10).unwrap();
    assert!(checkpoint.validate(&descriptor(), 0).is_err());
    let mut corrupt = checkpoint.clone();
    corrupt.payload[0] = b'1';
    assert!(corrupt.validate(&descriptor(), 10).is_err());
    let mut incompatible = descriptor();
    incompatible.configuration_sha256 = [8; 32];
    assert!(checkpoint.validate(&incompatible, 10).is_err());
    let mut invalid_descriptor = descriptor();
    invalid_descriptor.version = 0;
    assert!(ComponentCheckpoint::new(invalid_descriptor, vec![], 10).is_err());
    assert!(artifact().verify(&[43], 10).is_err());
    assert!(artifact().verify(&[42], 0).is_err());
}

struct Repository;
#[async_trait]
impl ArtifactStore for Repository {
    async fn get(&self, reference: &ArtifactRef, max_bytes: usize) -> Result<Vec<u8>> {
        let bytes = vec![42];
        reference.verify(&bytes, max_bytes)?;
        Ok(bytes)
    }
    async fn put(&self, _: &str, _: &[u8]) -> Result<ArtifactRef> {
        Err(Error::Unsupported("test repository is read-only".into()))
    }
}
struct Model {
    spec: ModelSpec,
    calls: u64,
    closed: bool,
}
fn model_descriptor(spec: &ModelSpec) -> StateDescriptor {
    StateDescriptor {
        configuration_sha256: Sha256::digest(serde_json::to_vec(spec).unwrap()).into(),
        ..descriptor()
    }
}
#[async_trait]
impl CheckpointParticipant for Model {
    fn state_descriptor(&self) -> StateDescriptor {
        model_descriptor(&self.spec)
    }
    async fn snapshot(&self) -> Result<ComponentCheckpoint> {
        ComponentCheckpoint::new(
            self.state_descriptor(),
            serde_json::to_vec(&self.calls).unwrap(),
            64,
        )
    }
}
#[async_trait]
impl InferenceModel for Model {
    fn spec(&self) -> &ModelSpec {
        &self.spec
    }
    async fn infer(
        &mut self,
        request: &InferenceRequest,
        cancel: &CancellationToken,
    ) -> Result<InferenceResponse> {
        if cancel.is_cancelled() || self.closed {
            return Err(Error::Invalid("cancelled or closed".into()));
        }
        self.spec.validate_request(request)?;
        let response = InferenceResponse {
            request_id: request.request_id.clone(),
            artifact: self.spec.artifact.clone(),
            output_schema_sha256: self.spec.outputs.sha256()?,
            predictions: request
                .rows
                .iter()
                .map(|row| Prediction {
                    row_id: row.id.clone(),
                    values: vec![row.values[0] * 2.],
                })
                .collect(),
        };
        response.validate(request, &self.spec)?;
        self.calls = self
            .calls
            .checked_add(1)
            .ok_or_else(|| Error::Invalid("counter overflow".into()))?;
        Ok(response)
    }
    async fn close(&mut self) -> Result<()> {
        self.closed = true;
        Ok(())
    }
}
struct Provider;
struct Factory {
    spec: ModelSpec,
}
#[async_trait]
impl CheckpointFactory for Factory {
    type Component = dyn InferenceModel;
    fn state_descriptor(&self) -> StateDescriptor {
        model_descriptor(&self.spec)
    }
    async fn restore(&self, checkpoint: &ComponentCheckpoint) -> Result<Box<Self::Component>> {
        Provider
            .load(
                &self.spec,
                &Repository,
                Some(checkpoint),
                &CancellationToken::new(),
            )
            .await
    }
}
#[async_trait]
impl ModelProvider for Provider {
    fn provider_id(&self) -> &str {
        "test-provider"
    }
    async fn load(
        &self,
        spec: &ModelSpec,
        repository: &dyn ArtifactStore,
        checkpoint: Option<&ComponentCheckpoint>,
        cancel: &CancellationToken,
    ) -> Result<Box<dyn InferenceModel>> {
        spec.validate()?;
        if spec.provider_id != self.provider_id() || cancel.is_cancelled() {
            return Err(Error::Invalid("provider mismatch or cancelled".into()));
        }
        repository.get(&spec.artifact, 100).await?;
        let calls = if let Some(checkpoint) = checkpoint {
            checkpoint.validate(&model_descriptor(spec), 64)?;
            serde_json::from_slice(&checkpoint.payload)
                .map_err(|e| Error::Invalid(e.to_string()))?
        } else {
            0
        };
        Ok(Box::new(Model {
            spec: spec.clone(),
            calls,
            closed: false,
        }))
    }
}
#[tokio::test]
async fn external_provider_can_stage_checkpoint_infer_and_reject_bad_results() {
    let provider: Box<dyn ModelProvider> = Box::new(Provider);
    let cancel = CancellationToken::new();
    let mut model = provider
        .load(&spec(), &Repository, None, &cancel)
        .await
        .unwrap();
    let mut response = model.infer(&request(), &cancel).await.unwrap();
    response.validate(&request(), model.spec()).unwrap();
    assert_eq!(response.predictions[0].values, vec![200.]);
    response.predictions[0].row_id = "wrong".into();
    assert!(response.validate(&request(), model.spec()).is_err());
    let checkpoint = model.snapshot().await.unwrap();
    let factory = Factory { spec: spec() };
    let factory: &dyn CheckpointFactory<Component = dyn InferenceModel> = &factory;
    let mut staged = factory.restore(&checkpoint).await.unwrap();
    staged.infer(&request(), &cancel).await.unwrap();
    assert_eq!(staged.snapshot().await.unwrap().payload, b"2");
    staged.close().await.unwrap();
    let mut incompatible = spec();
    incompatible.runtime_version = "2".into();
    assert!(
        provider
            .load(&incompatible, &Repository, Some(&checkpoint), &cancel)
            .await
            .is_err()
    );
    let mut restored = provider
        .load(&spec(), &Repository, Some(&checkpoint), &cancel)
        .await
        .unwrap();
    restored.infer(&request(), &cancel).await.unwrap();
    assert_eq!(restored.snapshot().await.unwrap().payload, b"2");
    restored.close().await.unwrap();
    restored.close().await.unwrap();
    assert!(
        restored
            .infer(&request(), &CancellationToken::new())
            .await
            .is_err()
    );
    cancel.cancel();
    assert!(restored.infer(&request(), &cancel).await.is_err());
    assert_eq!(restored.snapshot().await.unwrap().payload, b"2");
}

fn dataset(from: i64, through: i64, labels: i64) -> DatasetRef {
    DatasetRef {
        id: format!("dataset-{from}"),
        manifest_sha256: [3; 32],
        feature_schema_sha256: schema().sha256().unwrap(),
        target_schema_sha256: schema().sha256().unwrap(),
        from_ns: from,
        through_ns: through,
        labels_available_through_ns: labels,
    }
}
#[test]
fn training_purges_future_label_horizons_and_checks_sample_provenance() {
    let mut training = TrainingRequest {
        request_id: "fit-1".into(),
        trainer_id: "external".into(),
        training: dataset(0, 100, 110),
        validation: Some(dataset(111, 200, 210)),
        inputs: schema(),
        targets: schema(),
        initial_model: None,
        seed: 42,
        parameters: serde_json::Value::Null,
    };
    training.validate().unwrap();
    training.validation.as_mut().unwrap().from_ns = 110;
    assert!(training.validate().is_err());
    training.validation = None;
    training.validate().unwrap();
    let mut example = TrainingExample {
        features: request().rows.remove(0),
        targets: vec![1.],
        label_available_at_ns: 105,
    };
    example
        .validate(&training.training, &schema(), &schema())
        .unwrap();
    example.label_available_at_ns = 111;
    assert!(
        example
            .validate(&training.training, &schema(), &schema())
            .is_err()
    );
    example.label_available_at_ns = 105;
    example.targets[0] = f64::INFINITY;
    assert!(
        example
            .validate(&training.training, &schema(), &schema())
            .is_err()
    );
    let mut status = TrainingStatus::Succeeded {
        artifact: artifact(),
        metrics: [("loss".into(), 0.2)].into(),
    };
    status.validate().unwrap();
    if let TrainingStatus::Succeeded { metrics, .. } = &mut status {
        metrics.insert("loss".into(), f64::NAN);
    }
    assert!(status.validate().is_err());
}
