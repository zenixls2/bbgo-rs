use crate::Model;
use crate::replay::TradePrint;
use bbgo_types::*;
use serde::{Deserialize, Serialize};

/// Ordering is availability, BBO-before-trade priority, then source and sequence.
/// A deterministic tie break does not establish real causal order.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Position {
    pub available_at_ns: i64,
    pub priority: u8,
    pub source: String,
    pub sequence: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Metadata {
    pub source_id: String,
    pub writer_run_id: String,
    pub exchange_time_ns: Option<i64>,
    pub received_at_ns: i64,
    pub generation: u64,
    pub gap_before_ms: Option<i64>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Tick {
    Book {
        quote: BookTicker,
        metadata: Metadata,
    },
    Trade {
        print: TradePrint,
        metadata: Metadata,
    },
}
impl Tick {
    pub fn metadata(&self) -> &Metadata {
        match self {
            Self::Book { metadata, .. } | Self::Trade { metadata, .. } => metadata,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Input {
    Candle(KLine),
    Tick(Tick),
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Frame {
    pub position: Position,
    pub input: Input,
}
#[async_trait::async_trait]
pub trait BacktestSource: Send {
    fn model(&self) -> Model;
    fn identity(&self) -> &str;
    async fn next_frame(&mut self) -> Result<Option<Frame>>;
    async fn close(&mut self) -> Result<()>;
}
/// Streaming closed candles supplied by any database/service adapter. No CSV path.
pub struct KlineSource<I> {
    identity: String,
    rows: I,
    sequence: u64,
    closed: bool,
}
impl<I: Iterator<Item = KLine> + Send> KlineSource<I> {
    pub fn new(identity: String, rows: I) -> Result<Self> {
        if identity.is_empty() {
            return Err(Error::Invalid("empty candle source identity".into()));
        }
        Ok(Self {
            identity,
            rows,
            sequence: 0,
            closed: false,
        })
    }
}
#[async_trait::async_trait]
impl<I: Iterator<Item = KLine> + Send> BacktestSource for KlineSource<I> {
    fn model(&self) -> Model {
        Model::Kline
    }
    fn identity(&self) -> &str {
        &self.identity
    }
    async fn next_frame(&mut self) -> Result<Option<Frame>> {
        if self.closed {
            return Err(Error::Invalid("closed candle source".into()));
        }
        let Some(candle) = self.rows.next() else {
            return Ok(None);
        };
        self.sequence = self
            .sequence
            .checked_add(1)
            .ok_or_else(|| Error::Invalid("candle sequence overflow".into()))?;
        let available_at_ns = candle
            .end_time
            .checked_mul(1_000_000)
            .ok_or_else(|| Error::Invalid("candle nanosecond time overflow".into()))?;
        Ok(Some(Frame {
            position: Position {
                available_at_ns,
                priority: 0,
                source: self.identity.clone(),
                sequence: self.sequence,
            },
            input: Input::Candle(candle),
        }))
    }
    async fn close(&mut self) -> Result<()> {
        self.closed = true;
        Ok(())
    }
}

/// K-way merge with one lookahead per source, not a whole-history sort.
pub struct MergedSource {
    sources: Vec<Box<dyn BacktestSource>>,
    heads: Vec<Option<Frame>>,
    initialized: bool,
    identity: String,
    model: Model,
}
impl MergedSource {
    pub fn new(sources: Vec<Box<dyn BacktestSource>>) -> Result<Self> {
        let model = sources
            .first()
            .ok_or_else(|| Error::Invalid("empty backtest source set".into()))?
            .model();
        let mut ids = std::collections::BTreeSet::new();
        if sources
            .iter()
            .any(|s| s.model() != model || !ids.insert(s.identity().to_owned()))
        {
            return Err(Error::Invalid(
                "mixed source models or duplicate identities".into(),
            ));
        }
        let identity = serde_json::to_string(&ids).map_err(|e| Error::Invalid(e.to_string()))?;
        Ok(Self {
            heads: vec![None; sources.len()],
            sources,
            initialized: false,
            identity,
            model,
        })
    }
}
#[async_trait::async_trait]
impl BacktestSource for MergedSource {
    fn model(&self) -> Model {
        self.model
    }
    fn identity(&self) -> &str {
        &self.identity
    }
    async fn next_frame(&mut self) -> Result<Option<Frame>> {
        if !self.initialized {
            for (source, head) in self.sources.iter_mut().zip(&mut self.heads) {
                *head = source.next_frame().await?;
            }
            self.initialized = true;
        }
        let Some(index) = self
            .heads
            .iter()
            .enumerate()
            .filter_map(|(i, h)| h.as_ref().map(|v| (i, &v.position)))
            .min_by(|a, b| a.1.cmp(b.1))
            .map(|(i, _)| i)
        else {
            return Ok(None);
        };
        let frame = self.heads[index].take().expect("selected nonempty head");
        self.heads[index] = self.sources[index].next_frame().await?;
        if self.heads[index]
            .as_ref()
            .is_some_and(|next| next.position <= frame.position)
        {
            return Err(Error::Invalid("merged input source regressed".into()));
        }
        Ok(Some(frame))
    }
    async fn close(&mut self) -> Result<()> {
        let mut errors = Vec::new();
        for source in &mut self.sources {
            if let Err(error) = source.close().await {
                errors.push(error.to_string());
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(Error::Adapter(errors.join("; ")))
        }
    }
}
