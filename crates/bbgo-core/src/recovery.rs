//! Strategy-independent checkpoint participation; no live restore coordinator.
use async_trait::async_trait;
use bbgo_types::{Error, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Exact compatibility identity, including caller-defined configuration digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StateDescriptor {
    pub component_id: String,
    pub format: String,
    pub version: u32,
    pub configuration_sha256: [u8; 32],
}
impl StateDescriptor {
    pub fn validate(&self) -> Result<()> {
        if self.component_id.is_empty()
            || self.component_id.len() > 256
            || self.format.is_empty()
            || self.format.len() > 256
            || self.version == 0
        {
            return Err(Error::Invalid("invalid checkpoint descriptor".into()));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComponentCheckpoint {
    pub descriptor: StateDescriptor,
    pub payload: Vec<u8>,
    pub sha256: [u8; 32],
}
impl ComponentCheckpoint {
    pub fn new(descriptor: StateDescriptor, payload: Vec<u8>, max_bytes: usize) -> Result<Self> {
        let checkpoint = Self {
            descriptor,
            sha256: Sha256::digest(&payload).into(),
            payload,
        };
        checkpoint.validate(&checkpoint.descriptor, max_bytes)?;
        Ok(checkpoint)
    }
    /// Check before parsing payloads. Deserializers must ALSO bound input bytes
    /// before allocation. A checksum is not authentication or rollback protection.
    pub fn validate(&self, expected: &StateDescriptor, max_bytes: usize) -> Result<()> {
        self.descriptor.validate()?;
        if &self.descriptor != expected
            || self.payload.len() > max_bytes
            || self.sha256 != <[u8; 32]>::from(Sha256::digest(&self.payload))
        {
            return Err(Error::Invalid(
                "incompatible or corrupt component checkpoint".into(),
            ));
        }
        Ok(())
    }
}

/// Snapshot only at an application-defined quiescent barrier. Component snapshots
/// must not mutate state; an outer coordinator owns cursor/account consistency.
#[async_trait]
pub trait CheckpointParticipant: Send + Sync {
    fn state_descriptor(&self) -> StateDescriptor;
    async fn snapshot(&self) -> Result<ComponentCheckpoint>;
}

/// Restore into a NEW, isolated object. Validate schema/configuration/checksum
/// and semantic invariants before returning it. Never mutate a running instance.
/// An application publishes all staged participants only after every one succeeds.
#[async_trait]
pub trait CheckpointFactory: Send + Sync {
    /// Preserve the component's operational API after staging, e.g.
    /// `dyn InferenceModel`, instead of erasing it to snapshot-only methods.
    type Component: CheckpointParticipant + ?Sized;
    fn state_descriptor(&self) -> StateDescriptor;
    async fn restore(&self, checkpoint: &ComponentCheckpoint) -> Result<Box<Self::Component>>;
}
