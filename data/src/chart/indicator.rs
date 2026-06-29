use std::fmt::{self, Debug, Display};

use enum_map::Enum;
use exchange::adapter::MarketKind;
use serde::{Deserialize, Serialize};

pub trait Indicator: PartialEq + Display + 'static {
    fn for_market(market: MarketKind) -> &'static [Self]
    where
        Self: Sized;
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize, Serialize, Eq, Enum)]
pub enum KlineIndicator {
    Volume,
    CumulativeDelta,
    OpenInterest,
    DeltaBar,
    VWAP,
    CVDDivergence,
    SessionDeltaWave,
    GWTradeEffort,
    GexLevels,
}

impl Indicator for KlineIndicator {
    fn for_market(market: MarketKind) -> &'static [Self] {
        match market {
            MarketKind::Spot => &Self::FOR_SPOT,
            MarketKind::LinearPerps | MarketKind::InversePerps => &Self::FOR_PERPS,
        }
    }
}

impl KlineIndicator {
    // Indicator togglers on UI menus depend on these arrays.
    // Every variant needs to be in either SPOT, PERPS or both.
    /// Indicators that can be used with spot market tickers
    const FOR_SPOT: [KlineIndicator; 8] = [
        KlineIndicator::Volume,
        KlineIndicator::CumulativeDelta,
        KlineIndicator::DeltaBar,
        KlineIndicator::VWAP,
        KlineIndicator::CVDDivergence,
        KlineIndicator::SessionDeltaWave,
        KlineIndicator::GWTradeEffort,
        KlineIndicator::GexLevels,
    ];
    /// Indicators that can be used with perpetual swap market tickers
    const FOR_PERPS: [KlineIndicator; 9] = [
        KlineIndicator::Volume,
        KlineIndicator::CumulativeDelta,
        KlineIndicator::OpenInterest,
        KlineIndicator::DeltaBar,
        KlineIndicator::VWAP,
        KlineIndicator::CVDDivergence,
        KlineIndicator::SessionDeltaWave,
        KlineIndicator::GWTradeEffort,
        KlineIndicator::GexLevels,
    ];
}

impl Display for KlineIndicator {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            KlineIndicator::Volume => write!(f, "Volume"),
            KlineIndicator::CumulativeDelta => write!(f, "CVD"),
            KlineIndicator::OpenInterest => write!(f, "Open Interest"),
            KlineIndicator::DeltaBar => write!(f, "Delta Bar"),
            KlineIndicator::VWAP => write!(f, "VWAP"),
            KlineIndicator::CVDDivergence => write!(f, "CVD Divergence"),
            KlineIndicator::SessionDeltaWave => write!(f, "Session Delta Wave"),
            KlineIndicator::GWTradeEffort => write!(f, "GWTrade Effort"),
            KlineIndicator::GexLevels => write!(f, "GEX Levels"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize, Serialize, Eq, Enum)]
pub enum HeatmapIndicator {
    Volume,
}

impl Indicator for HeatmapIndicator {
    fn for_market(market: MarketKind) -> &'static [Self] {
        match market {
            MarketKind::Spot => &Self::FOR_SPOT,
            MarketKind::LinearPerps | MarketKind::InversePerps => &Self::FOR_PERPS,
        }
    }
}

impl HeatmapIndicator {
    // Indicator togglers on UI menus depend on these arrays.
    // Every variant needs to be in either SPOT, PERPS or both.
    /// Indicators that can be used with spot market tickers
    const FOR_SPOT: [HeatmapIndicator; 1] = [HeatmapIndicator::Volume];
    /// Indicators that can be used with perpetual swap market tickers
    const FOR_PERPS: [HeatmapIndicator; 1] = [HeatmapIndicator::Volume];
}

impl Display for HeatmapIndicator {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            HeatmapIndicator::Volume => write!(f, "Volume"),
        }
    }
}

#[derive(Debug, Clone, Copy)]
/// Temporary workaround,
/// represents any indicator type in the UI
pub enum UiIndicator {
    Heatmap(HeatmapIndicator),
    Kline(KlineIndicator),
}

impl From<KlineIndicator> for UiIndicator {
    fn from(k: KlineIndicator) -> Self {
        UiIndicator::Kline(k)
    }
}

impl From<HeatmapIndicator> for UiIndicator {
    fn from(h: HeatmapIndicator) -> Self {
        UiIndicator::Heatmap(h)
    }
}
