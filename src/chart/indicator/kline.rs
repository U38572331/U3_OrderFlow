use crate::chart::{Basis, Message, ViewState};
use crate::connector::fetcher::FetchRange;

use data::chart::indicator::KlineIndicator;
use data::chart::kline::KlineDataPoint;
use data::chart::{BasisSeries, PlotData};
use exchange::adapter::Exchange;
use exchange::{Kline, Timeframe, Trade, UnixMs};

use super::plot::AnySeries;

pub mod cumulative_delta;
pub mod open_interest;
pub mod volume;

pub mod cvd_divergence;
pub mod delta_bar;
pub mod session_delta_wave;
pub mod vwap;
pub mod gex_levels;
pub mod gwtrade_effort;


/// UI adapter methods for converting domain `BasisSeries` into plot-ready series.
trait BasisSeriesExt<T> {
    fn as_plot_series(&self) -> AnySeries<'_, T>;
}

impl<T> BasisSeriesExt<T> for BasisSeries<T> {
    fn as_plot_series(&self) -> AnySeries<'_, T> {
        match self {
            BasisSeries::Time(data) => AnySeries::forward_unix_ms(data),
            BasisSeries::Tick(data) => AnySeries::reversed_u64(data),
        }
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone, Default, PartialEq)]
pub enum IndicatorAvailability {
    /// Indicator can be rendered normally.
    #[default]
    Available,
    /// Availability cannot be determined yet (e.g. no datapoints loaded).
    Unknown,
    /// Indicator cannot be rendered for the current source/context.
    Unavailable(AvailabilityCause),
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq)]
pub enum AvailabilityCause {
    Exchange(Exchange),
    Timeframe(Timeframe),
    Basis(Basis),
    TradeData,
}

impl IndicatorAvailability {
    pub fn unavailable_message(&self, indicator: &str) -> Option<String> {
        match self {
            IndicatorAvailability::Available | IndicatorAvailability::Unknown => None,
            IndicatorAvailability::Unavailable(cause) => Some(match cause {
                AvailabilityCause::Exchange(exchange) => {
                    format!("{indicator} is not available for {exchange}.")
                }
                AvailabilityCause::Timeframe(timeframe) => {
                    format!("{indicator} is not available on {timeframe} timeframe.")
                }
                AvailabilityCause::Basis(Basis::Tick(_)) => {
                    format!("{indicator} is not available for tick charts.")
                }
                AvailabilityCause::Basis(basis) => {
                    format!("{indicator} is not available on {basis} basis.")
                }
                AvailabilityCause::TradeData => {
                    format!("{indicator} requires directional trade-volume data.")
                }
            }),
        }
    }
}

pub trait KlineIndicatorImpl {
    /// Clear all caches for a full redraw
    fn clear_all_caches(&mut self);

    /// Clear caches related to crosshair only
    /// e.g. tooltips and scale labels for a partial redraw
    fn clear_crosshair_caches(&mut self);

    fn element<'a>(
        &'a self,
        chart: &'a ViewState,
        // Whether to show last value labels on top right/left when not hovering
        data_labels_always_visible: bool,
        visible_range: std::ops::RangeInclusive<u64>,
    ) -> iced::Element<'a, Message>;

    /// Whether this indicator is drawn directly on the main price chart
    /// (e.g. VWAP, divergence arrows). If true, it won't be allocated a sub-pane.
    fn is_overlay(&self) -> bool {
        false
    }

    /// Allows the indicator to draw directly on the main Kline chart canvas.
    /// Only called if `is_overlay()` is true, or if you want to draw things over the main chart regardless.
    fn draw_overlay(
        &self,
        _frame: &mut iced::widget::canvas::Frame,
        _chart: &ViewState,
        _theme: &iced::Theme,
        _visible_range: std::ops::RangeInclusive<u64>,
    ) {}

    fn draw_underlay(
        &self,
        _frame: &mut iced::widget::canvas::Frame,
        _chart: &ViewState,
        _theme: &iced::Theme,
        _visible_range: std::ops::RangeInclusive<u64>,
    ) {
    }

    fn on_gex_event(&mut self, _event: &crate::connector::gex_client::GexEvent) {}

    fn availability(&self, _chart: &ViewState) -> IndicatorAvailability {
        IndicatorAvailability::Available
    }

    fn unavailable_message(&self, chart: &ViewState, indicator: &str) -> Option<String> {
        self.availability(chart).unavailable_message(indicator)
    }

    /// If the indicator needs data fetching, return the required range
    fn fetch_range(&mut self, _ctx: &FetchCtx) -> Option<FetchRange> {
        None
    }

    /// Rebuild data using kline(OHLCV) source
    fn rebuild_from_source(&mut self, _source: &PlotData<KlineDataPoint>) {}

    fn on_insert_klines(&mut self, _klines: &[Kline], _source: &PlotData<KlineDataPoint>) {}

    fn on_insert_trades(
        &mut self,
        _trades: &[Trade],
        _old_dp_len: usize,
        _source: &PlotData<KlineDataPoint>,
    ) {
    }

    fn on_ticksize_change(&mut self, _source: &PlotData<KlineDataPoint>) {}

    /// Timeframe/tick interval has changed
    fn on_basis_change(&mut self, _source: &PlotData<KlineDataPoint>) {}

    fn on_open_interest(&mut self, _pairs: &[exchange::OpenInterest]) {}
}

pub struct FetchCtx<'a> {
    pub main_chart: &'a ViewState,
    pub timeframe: Timeframe,
    pub visible_earliest: UnixMs,
    pub kline_latest: UnixMs,
    pub prefetch_earliest: UnixMs,
}

pub fn make_empty(which: KlineIndicator) -> Box<dyn KlineIndicatorImpl> {
    match which {
        KlineIndicator::Volume => Box::new(super::kline::volume::VolumeIndicator::new()),
        KlineIndicator::CumulativeDelta => {
            Box::new(super::kline::cumulative_delta::CumulativeDeltaIndicator::new())
        }
        KlineIndicator::OpenInterest => {
            Box::new(super::kline::open_interest::OpenInterestIndicator::new())
        }
        KlineIndicator::DeltaBar => {
            Box::new(super::kline::delta_bar::DeltaBarIndicator::new())
        }
        KlineIndicator::VWAP => {
            Box::new(super::kline::vwap::VWAPIndicator::new())
        }
        KlineIndicator::CVDDivergence => {
            Box::new(super::kline::cvd_divergence::CVDDivergenceIndicator::new())
        }
        KlineIndicator::SessionDeltaWave => {
            Box::new(super::kline::session_delta_wave::SessionDeltaWaveIndicator::new())
        }
        KlineIndicator::GexLevels => Box::new(gex_levels::GexLevelsIndicator::new()),
        KlineIndicator::GWTradeEffort => Box::new(gwtrade_effort::GWTradeEffortIndicator::new()),
    }
}
