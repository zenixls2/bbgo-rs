//! Sequential, backpressured strategy dispatch with explicit reconnect and cleanup.
use crate::{RunMode, Strategy, StrategyContext};
use bbgo_types::{Error, EventStream, MarketEvent, Result, UserEvent};
use tokio_util::sync::CancellationToken;

pub enum SessionEvent {
    Market(MarketEvent),
    User(UserEvent),
    Disconnected,
    Reconnected { generation: u64 },
}
/// Cleanup is attempted after cancellation, EOF, dispatch or transport errors.
/// A reconnect requires a strictly increasing generation and a successful strategy
/// hook before further market events. Initial generation is zero.
pub async fn run(
    strategy: &mut dyn Strategy,
    stream: &mut dyn EventStream<SessionEvent>,
    context: &StrategyContext<'_>,
    cancel: &CancellationToken,
) -> Result<()> {
    if context.mode == RunMode::Live {
        let closed = stream.close().await;
        return combine(
            Err(Error::Unsupported("live session disabled".into())),
            closed,
        );
    }
    if let Err(error) = strategy.start(context).await {
        return combine(Err(error), stream.close().await);
    }
    let result = dispatch(strategy, stream, context, cancel).await;
    let shutdown = strategy.shutdown(context).await;
    let close = stream.close().await;
    combine(combine(result, shutdown), close)
}
fn combine(primary: Result<()>, cleanup: Result<()>) -> Result<()> {
    match (primary, cleanup) {
        (Ok(()), result) | (result, Ok(())) => result,
        (Err(primary), Err(cleanup)) => {
            Err(Error::Adapter(format!("{primary}; cleanup: {cleanup}")))
        }
    }
}
async fn dispatch(
    strategy: &mut dyn Strategy,
    stream: &mut dyn EventStream<SessionEvent>,
    context: &StrategyContext<'_>,
    cancel: &CancellationToken,
) -> Result<()> {
    let mut connected = true;
    let mut generation = 0;
    loop {
        // next_event must be cancellation/drop-safe. Strategy handlers complete
        // atomically from the runner's perspective (not dropped mid-checkpoint).
        let event = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Ok(()),
            event = stream.next_event() => event?,
        };
        match event {
            None => return Ok(()),
            Some(SessionEvent::Disconnected) => connected = false,
            Some(SessionEvent::Reconnected { generation: next }) => {
                if connected || next <= generation {
                    return Err(Error::Invalid("invalid reconnect generation".into()));
                }
                strategy.reconnect(next, context).await?;
                generation = next;
                connected = true;
            }
            Some(SessionEvent::Market(event)) => {
                if !connected {
                    return Err(Error::Invalid("market event while disconnected".into()));
                }
                if let MarketEvent::Quote(quote) = &event {
                    if quote.generation != generation {
                        return Err(Error::Invalid("quote/session generation mismatch".into()));
                    }
                }
                let event_generation = match &event {
                    MarketEvent::BookTicker(quote) => Some(quote.generation),
                    MarketEvent::PublicTrade(trade) => Some(trade.generation),
                    _ => None,
                };
                if event_generation.is_some_and(|value| value != generation) {
                    return Err(Error::Invalid("market/session generation mismatch".into()));
                }
                strategy.on_market_event(&event, context).await?;
            }
            Some(SessionEvent::User(event)) => strategy.on_user_event(&event, context).await?,
        }
    }
}
