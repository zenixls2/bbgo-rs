//! Deterministic recovery by replaying successful operations, never by trusting
//! serialized derived balances, locks, fee accumulators or order status.
use super::*;
use bbgo_core::CheckpointStore;

pub const MAX_CHECKPOINT_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_RECOVERY_ACTIONS: usize = 10_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Action {
    Submit {
        request: SubmitOrder,
    },
    Trade {
        print: TradePrint,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        eligible: Option<BTreeSet<String>>,
    },
    Cancel {
        ids: Vec<String>,
    },
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Checkpoint {
    version: u32,
    markets: BTreeMap<String, Market>,
    fee_rate: Value,
    seed: Ledger,
    actions: Vec<Action>,
}
impl ReplayExecutor {
    /// An atomic in-memory snapshot. Use FileStore for checksummed durable writes.
    /// The journal includes non-filling prints to preserve clocks/deduplication.
    /// Offline scope only: history is not compacted; oversized saves fail explicitly.
    pub fn checkpoint(&self) -> Result<Vec<u8>> {
        let state = self
            .state
            .lock()
            .map_err(|_| Error::Adapter("replay lock poisoned".into()))?;
        if state.journal.len() > MAX_RECOVERY_ACTIONS {
            return Err(Error::Invalid(
                "replay recovery action limit exceeded".into(),
            ));
        }
        let bytes = serde_json::to_vec(&Checkpoint {
            version: if state.journal.iter().any(|action| {
                matches!(
                    action,
                    Action::Trade {
                        eligible: Some(_),
                        ..
                    }
                )
            }) {
                2
            } else {
                1
            },
            markets: self.markets.clone(),
            fee_rate: self.fee_rate,
            seed: self.seed.clone(),
            actions: state.journal.clone(),
        })
        .map_err(|e| Error::Invalid(e.to_string()))?;
        if bytes.len() > MAX_CHECKPOINT_BYTES {
            return Err(Error::Invalid(
                "replay checkpoint byte limit exceeded".into(),
            ));
        }
        Ok(bytes)
    }
    /// Rebuild into a new executor, publishing nothing if any operation fails.
    /// Trusted expected markets, seed account and fees must match exactly.
    /// This is recovery, not authentication: protect the underlying checkpoint store.
    pub fn from_checkpoint(
        markets: BTreeMap<String, Market>,
        seed: Ledger,
        fee_rate: Value,
        bytes: &[u8],
    ) -> Result<Self> {
        if bytes.len() > MAX_CHECKPOINT_BYTES {
            return Err(Error::Invalid(
                "replay checkpoint byte limit exceeded".into(),
            ));
        }
        let checkpoint: Checkpoint = serde_json::from_slice(bytes)
            .map_err(|e| Error::Invalid(format!("invalid replay checkpoint: {e}")))?;
        let expected_version = if checkpoint.actions.iter().any(|action| {
            matches!(
                action,
                Action::Trade {
                    eligible: Some(_),
                    ..
                }
            )
        }) {
            2
        } else {
            1
        };
        if checkpoint.version != expected_version
            || checkpoint.markets != markets
            || checkpoint.seed != seed
            || checkpoint.fee_rate != fee_rate
            || checkpoint.actions.len() > MAX_RECOVERY_ACTIONS
        {
            return Err(Error::Invalid(
                "replay checkpoint version/config/account mismatch".into(),
            ));
        }
        let executor = Self::new(markets, seed, fee_rate)?;
        for (index, action) in checkpoint.actions.into_iter().enumerate() {
            match action {
                Action::Submit { request } => {
                    executor.submit_limit(request)?;
                }
                Action::Trade { print, eligible } => {
                    if let Some(ids) = eligible {
                        executor.on_trades_for_orders(vec![print], &ids)?;
                    } else {
                        executor.on_trade(print)?;
                    }
                }
                Action::Cancel { ids } => {
                    let known = executor.orders()?;
                    let orders: Vec<Order> =
                        ids.iter()
                            .map(|id| {
                                known.iter().find(|o| &o.id == id).cloned().ok_or_else(|| {
                                    Error::Invalid("unknown recovered cancel ID".into())
                                })
                            })
                            .collect::<Result<_>>()?;
                    executor.cancel_orders(&orders)?;
                }
            }
            // The journal is canonical: duplicates and terminal no-ops are not
            // recorded by normal execution, so reject them in recovery too.
            if executor
                .state
                .lock()
                .map_err(|_| Error::Adapter("replay lock poisoned".into()))?
                .journal
                .len()
                != index + 1
            {
                return Err(Error::Invalid(
                    "noncanonical replay recovery operation".into(),
                ));
            }
        }
        Ok(executor)
    }
    /// Caller must quiesce processing and coordinate this save with its strategy
    /// checkpoint and input cursor; independent keys are NOT one transaction.
    pub async fn save_checkpoint(&self, store: &dyn CheckpointStore, key: &str) -> Result<()> {
        store.save(key, &self.checkpoint()?).await
    }
    pub async fn load_checkpoint(
        markets: BTreeMap<String, Market>,
        seed: Ledger,
        fee_rate: Value,
        store: &dyn CheckpointStore,
        key: &str,
    ) -> Result<Option<Self>> {
        store
            .load(key)
            .await?
            .map(|bytes| Self::from_checkpoint(markets, seed, fee_rate, &bytes))
            .transpose()
    }
}
