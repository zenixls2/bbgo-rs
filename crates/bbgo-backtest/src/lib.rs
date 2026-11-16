//! One offline runner, with distinct candle-simulation and recorded-tick models.
//! Sources never submit orders. Both models share reservations/accounting.
pub mod simulation;
pub mod source;
pub mod sql;
pub mod sqlite;

use bbgo_core::{CheckpointStore, Notifier, RunMode, Strategy, StrategyContext};
use bbgo_types::{accounting::Ledger, *};
use serde::{Deserialize, Serialize};
use simulation::Simulation;
use source::BacktestSource;
use std::collections::BTreeMap;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Model {
    /// Synthetic open/low/high/close (up) or open/high/low/close (down).
    /// Full resting-limit fills; no inferred actual intrabar market sequence.
    Kline,
    /// Only observed public trade volume fills resting limits. BBO never fills.
    Event,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Completion {
    Exhausted,
    Cancelled,
}
#[derive(Debug, Serialize)]
pub struct Report {
    pub source_identity: String,
    pub model: Model,
    pub frames: u64,
    pub completion: Completion,
    pub last_position: Option<source::Position>,
    pub ledger: Ledger,
    pub orders: Vec<Order>,
}
pub struct Backtest {
    simulation: Simulation,
    used: bool,
}
impl Backtest {
    pub fn new(
        model: Model,
        markets: BTreeMap<String, Market>,
        seed: Ledger,
        fee: Value,
    ) -> Result<Self> {
        Ok(Self {
            simulation: Simulation::new(model, markets, seed, fee)?,
            used: false,
        })
    }
    pub fn executor(&self) -> &Simulation {
        &self.simulation
    }
    /// A run owns source cleanup and strategy start/shutdown. Never reuses a
    /// finished/failed simulation. Match first, then user updates, then market
    /// callbacks: reacting orders cannot fill on the already processed input.
    pub async fn run(
        &mut self,
        strategy: &mut dyn Strategy,
        source: &mut dyn BacktestSource,
        checkpoints: &dyn CheckpointStore,
        notifier: &dyn Notifier,
        cancel: &CancellationToken,
    ) -> Result<Report> {
        if self.used || source.model() != self.simulation.model() {
            return combine(
                Err(Error::Invalid(
                    "used backtest or source/model mismatch".into(),
                )),
                source.close().await,
            );
        }
        self.used = true;
        let context = StrategyContext {
            mode: RunMode::Replay,
            orders: &self.simulation,
            checkpoints,
            notifier,
        };
        if let Err(error) = strategy.start(&context).await {
            return combine(Err(error), source.close().await);
        }
        let result = self.dispatch(strategy, source, &context, cancel).await;
        let shutdown = strategy.shutdown(&context).await;
        let closed = source.close().await;
        let mut report = combine(combine(result, shutdown), closed)?;
        report.ledger = self.simulation.ledger()?;
        report.orders = self.simulation.orders()?;
        Ok(report)
    }
    async fn dispatch(
        &self,
        strategy: &mut dyn Strategy,
        source: &mut dyn BacktestSource,
        context: &StrategyContext<'_>,
        cancel: &CancellationToken,
    ) -> Result<Report> {
        let mut frames = 0_u64;
        let mut last: Option<source::Position> = None;
        let mut generation = 0_u64;
        let mut source_generations = BTreeMap::new();
        let completion = loop {
            if cancel.is_cancelled() {
                break Completion::Cancelled;
            }
            // Do not drop an in-flight source read or partially dispatched
            // strategy callback. SQLite pages do synchronous offline query work;
            // native SQL reads await the server and have statement deadlines.
            let Some(mut frame) = source.next_frame().await? else {
                break Completion::Exhausted;
            };
            if last
                .as_ref()
                .is_some_and(|previous| frame.position <= *previous)
            {
                return Err(Error::Invalid(
                    "backtest source position regressed/duplicated".into(),
                ));
            }
            self.simulation
                .set_callback_clock(frame.position.available_at_ns)?;
            if let source::Input::Tick(tick) = &mut frame.input {
                let next = tick.metadata().generation;
                if matches!(tick, source::Tick::Book { quote, metadata } if quote.generation != metadata.generation)
                {
                    return Err(Error::Invalid("source quote generation mismatch".into()));
                }
                let previous = source_generations
                    .get(&frame.position.source)
                    .copied()
                    .unwrap_or(0);
                if next != previous {
                    if next < previous {
                        return Err(Error::Invalid("tick generation regressed".into()));
                    }
                    generation = generation
                        .checked_add(1_u64)
                        .ok_or_else(|| Error::Invalid("session generation overflow".into()))?;
                    strategy.reconnect(generation, context).await?;
                }
                source_generations.insert(frame.position.source.clone(), next);
                match tick {
                    source::Tick::Book { quote, metadata } => {
                        quote.generation = generation;
                        metadata.generation = generation;
                    }
                    source::Tick::Trade { metadata, .. } => metadata.generation = generation,
                }
            }
            let batch = self.simulation.process(&frame)?;
            for event in batch.user {
                strategy.on_user_event(&event, context).await?;
            }
            if let Some(event) = batch.market {
                strategy.on_market_event(&event, context).await?;
            }
            frames = frames
                .checked_add(1)
                .ok_or_else(|| Error::Invalid("frame count overflow".into()))?;
            last = Some(frame.position);
            tokio::task::yield_now().await;
        };
        Ok(Report {
            source_identity: source.identity().into(),
            model: source.model(),
            frames,
            last_position: last,
            completion,
            ledger: self.simulation.ledger()?,
            orders: self.simulation.orders()?,
        })
    }
}
fn combine<T>(primary: Result<T>, cleanup: Result<()>) -> Result<T> {
    match (primary, cleanup) {
        (Ok(result), Ok(())) => Ok(result),
        (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
        (Err(primary), Err(cleanup)) => {
            Err(Error::Adapter(format!("{primary}; cleanup: {cleanup}")))
        }
    }
}
