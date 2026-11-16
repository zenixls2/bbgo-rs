//! Strategy/service boundaries. This is not yet a live BBGO engine.
pub mod checkpoint;
pub mod recovery;
pub mod session;
use async_trait::async_trait;
use bbgo_types::*;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::Mutex};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunMode {
    #[default]
    Replay,
    Paper,
    Live,
}

#[async_trait]
pub trait OrderExecutor: Send + Sync {
    async fn submit(&self, order: SubmitOrder) -> Result<Order>;
    async fn cancel(&self, orders: &[Order]) -> Result<()>;
}

/// Limit-order intent recorder, NOT a matching or portfolio accounting engine.
/// No Exchange/TradeService is held, so it cannot reach a live order endpoint.
#[derive(Debug)]
pub struct PaperExecutor {
    mode: RunMode,
    orders: Mutex<BTreeMap<String, Order>>,
}
impl PaperExecutor {
    pub fn new(mode: RunMode) -> Result<Self> {
        if mode == RunMode::Live {
            return Err(Error::Unsupported(
                "live execution has not been implemented or approved".into(),
            ));
        }
        Ok(Self {
            mode,
            orders: Mutex::new(BTreeMap::new()),
        })
    }
    pub fn mode(&self) -> RunMode {
        self.mode
    }
    pub fn orders(&self) -> Result<Vec<Order>> {
        Ok(self
            .orders
            .lock()
            .map_err(|_| Error::Adapter("order lock poisoned".into()))?
            .values()
            .cloned()
            .collect())
    }
}
#[async_trait]
impl OrderExecutor for PaperExecutor {
    async fn submit(&self, order: SubmitOrder) -> Result<Order> {
        if order.client_order_id.is_empty()
            || order.symbol.is_empty()
            || order.quantity <= Value::ZERO
            || order.price <= Value::ZERO
            || order.order_type == OrderType::Market
        {
            return Err(Error::Invalid(
                "paper executor requires a positive limit order and client ID".into(),
            ));
        }
        let mut orders = self
            .orders
            .lock()
            .map_err(|_| Error::Adapter("order lock poisoned".into()))?;
        if let Some(existing) = orders.get(&order.client_order_id) {
            if existing.request == order {
                return Ok(existing.clone());
            }
            return Err(Error::Invalid(
                "client order ID reused for a different intent".into(),
            ));
        }
        let created = Order {
            id: format!("paper:{}", order.client_order_id),
            request: order,
            status: OrderStatus::New,
            executed_quantity: Value::ZERO,
        };
        orders.insert(created.request.client_order_id.clone(), created.clone());
        Ok(created)
    }
    async fn cancel(&self, requested: &[Order]) -> Result<()> {
        let mut orders = self
            .orders
            .lock()
            .map_err(|_| Error::Adapter("order lock poisoned".into()))?;
        for order in requested {
            if !orders
                .get(&order.request.client_order_id)
                .is_some_and(|v| v.id == order.id)
            {
                return Err(Error::Invalid("unknown paper order".into()));
            }
        }
        for order in requested {
            let stored = orders.get_mut(&order.request.client_order_id).unwrap();
            if stored.status == OrderStatus::New {
                stored.status = OrderStatus::Canceled;
            }
        }
        Ok(())
    }
}

#[async_trait]
pub trait CheckpointStore: Send + Sync {
    async fn load(&self, key: &str) -> Result<Option<Vec<u8>>>;
    /// Implementations must atomically replace a key, never expose partial writes.
    async fn save(&self, key: &str, value: &[u8]) -> Result<()>;
}
#[derive(Default)]
pub struct MemoryStore(Mutex<BTreeMap<String, Vec<u8>>>);
#[async_trait]
impl CheckpointStore for MemoryStore {
    async fn load(&self, key: &str) -> Result<Option<Vec<u8>>> {
        Ok(self
            .0
            .lock()
            .map_err(|_| Error::Adapter("checkpoint lock poisoned".into()))?
            .get(key)
            .cloned())
    }
    async fn save(&self, key: &str, value: &[u8]) -> Result<()> {
        self.0
            .lock()
            .map_err(|_| Error::Adapter("checkpoint lock poisoned".into()))?
            .insert(key.into(), value.into());
        Ok(())
    }
}
#[async_trait]
pub trait Notifier: Send + Sync {
    async fn notify(&self, message: &str) -> Result<()>;
}

pub struct StrategyContext<'a> {
    pub mode: RunMode,
    pub orders: &'a dyn OrderExecutor,
    pub checkpoints: &'a dyn CheckpointStore,
    pub notifier: &'a dyn Notifier,
}
#[async_trait]
pub trait Strategy: Send {
    fn id(&self) -> &'static str;
    fn instance_id(&self) -> &str;
    /// Discovery occurs before stream subscription.
    fn subscriptions(&self, markets: &BTreeMap<String, Market>) -> Result<Vec<String>>;
    async fn start(&mut self, context: &StrategyContext<'_>) -> Result<()>;
    async fn on_market_event(
        &mut self,
        event: &MarketEvent,
        context: &StrategyContext<'_>,
    ) -> Result<()>;
    async fn on_user_event(
        &mut self,
        event: &UserEvent,
        context: &StrategyContext<'_>,
    ) -> Result<()>;
    async fn shutdown(&mut self, context: &StrategyContext<'_>) -> Result<()>;
    /// Reconnect is fail-closed unless a strategy explicitly resets/resynchronizes.
    async fn reconnect(&mut self, _generation: u64, _context: &StrategyContext<'_>) -> Result<()> {
        Err(Error::Unsupported("strategy reconnect handling".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::executor::block_on;
    #[test]
    fn live_is_not_an_accidental_default() {
        assert_eq!(RunMode::default(), RunMode::Replay);
        assert!(PaperExecutor::new(RunMode::Live).is_err());
        assert_eq!(
            PaperExecutor::new(RunMode::Paper).unwrap().mode(),
            RunMode::Paper
        );
    }
    #[test]
    fn paper_orders_are_idempotent_and_cancelable() {
        block_on(async {
            let executor = PaperExecutor::new(RunMode::Replay).unwrap();
            let intent = SubmitOrder {
                client_order_id: "one".into(),
                symbol: "BTCJPY".into(),
                side: Side::Buy,
                order_type: OrderType::Limit,
                price: "100".parse().unwrap(),
                quantity: Value::ONE,
            };
            let order = executor.submit(intent.clone()).await.unwrap();
            assert_eq!(executor.submit(intent.clone()).await.unwrap(), order);
            let mut conflict = intent.clone();
            conflict.quantity = "2".parse().unwrap();
            assert!(executor.submit(conflict).await.is_err());
            assert_eq!(executor.orders().unwrap().len(), 1);
            executor.cancel(std::slice::from_ref(&order)).await.unwrap();
            executor.cancel(std::slice::from_ref(&order)).await.unwrap();
            assert_eq!(executor.orders().unwrap()[0].status, OrderStatus::Canceled);
            assert_eq!(
                executor.submit(intent).await.unwrap().status,
                OrderStatus::Canceled
            );
            let store = MemoryStore::default();
            assert!(store.load("missing").await.unwrap().is_none());
            store.save("state", b"one").await.unwrap();
            store.save("state", b"two").await.unwrap();
            assert_eq!(store.load("state").await.unwrap().unwrap(), b"two");
        });
    }
}
