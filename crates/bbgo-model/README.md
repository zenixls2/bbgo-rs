# Model integration interfaces

Strategy-neutral contracts only. No gamma algorithm, training implementation,
HTTP provider, Python runtime, ONNX/Torch dependency or live-order path is added.
Adapters can live in separate crates/applications.

| Interface | Adapter responsibility |
| --- | --- |
| `FeatureExtractor` | Transform shared market events into schema-bound numeric features |
| `InferenceModel` | Local/native or remote inference, with explicit checkpoint participation |
| `ModelProvider` | Load a verified artifact and optionally stage restored model state |
| `ArtifactStore` | Bounded immutable artifact storage, exact size/hash verification |
| `training::DatasetReader` | Stream bounded feature/target batches from SQL or other storage |
| `training::TrainingBackend` | Submit, poll and explicitly cancel external training jobs |
| `bbgo_core::recovery::CheckpointParticipant` | Snapshot arbitrary components at a quiescent barrier |
| `bbgo_core::recovery::CheckpointFactory` | Restore a fresh isolated component, not mutate a live one |

All async traits are object-safe. `ModelProvider::load` returns
`Box<dyn InferenceModel>`; no model SDK type leaks into trading code. Configuration
is non-secret JSON, provider and runtime IDs are explicit. Supply credentials
out of band. An adapter can translate this contract to ONNX Runtime, a native
Rust model, a Python worker, or a third-party service, but none is bundled.

## Compatibility, timing and validation

- `NumericSchema` defines **ordered scalar f64** fields. Its SHA-256 includes
  field order and version. v1 does not pretend to cover text/images/arbitrary
  tensors. Adapters needing those must define a new versioned protocol.
- `FeatureRow` carries observation and first-availability nanoseconds. Call
  `ModelSpec::validate_request` before inference: future/unavailable features,
  incompatible shapes, nonfinite numbers, duplicate IDs and excess rows fail.
  These guards depend on honest source metadata; they do not discover leakage
  hidden inside a feature computation.
- `training_information_through_ns` includes label availability and rejects
  applying a trained model before its training information existed. `None` is
  for non-trained models, not missing training provenance.
- Call `InferenceResponse::validate` at the application boundary as well as in
  the adapter. It verifies artifact/schema/request identity, row ordering, shape
  and finite outputs. A prediction is **not** authorization to submit orders.
- `DatasetRef` identifies an immutable manifest containing source snapshots,
  transform versions and label policy. Readers must verify manifest identity.
  `TrainingRequest::validate` requires chronological validation strictly after
  the training **label horizon**, not just its final feature time. Per-example
  validation checks label and feature bounds. This is not a complete
  walk-forward/OOS evaluation or statistical leakage detector.
- Artifact digests provide identity/integrity, not authenticity. Only execute
  artifacts from trusted publishers; unsafe formats (e.g. arbitrary Python
  pickle) are not sandboxed by these interfaces.

## Checkpoints

`bbgo-core::recovery` supplies a versioned `StateDescriptor` and
`ComponentCheckpoint` with payload digest and a caller-provided byte limit.
Configuration digests should bind artifact identity, runtime/schema versions,
parameters, preprocessing and other state compatibility requirements.
Stateful models checkpoint RNG/rolling state; immutable weights belong in the
artifact repository. Stateless models still identify their configuration.

Validate exact descriptor, payload size/digest, and semantic invariants **before**
restoring. `CheckpointFactory` and `ModelProvider` stage NEW objects; applications
publish them only after all participants validate. No coordinator, automatic
barrier, multi-key transaction, cursor restore, or gamma wiring is implemented.
The existing file/SQL `CheckpointStore` interfaces persist opaque bytes; their
4 MiB limits also apply to any envelope overhead. Bound serialized input before
deserializing, not only after constructing an in-memory value.

## Cancellation and external jobs

Inference adapters honor cancellation without committing partial model state.
Applications explicitly call `InferenceModel::close` on all exit paths to release
worker/session resources; a closed model cannot accept further inference.
For remote training, cancelling a submission/poll transport does **not** imply
the remote job stopped. Use `cancel_job` and check status explicitly. Stable
training request IDs are idempotency keys; conflicting reuse must fail.
Adapters without safe idempotency/cancellation must return an explicit error
rather than silently retrying jobs. Applications own deadlines, concurrency,
queue limits, retries, resource budgets and shutdown cleanup.

See `tests/contracts.rs` for an SDK-free external-provider mock implementing
artifact verification, inference, isolated checkpoint recovery and cancellation,
plus compatibility and training-boundary rejection tests.
