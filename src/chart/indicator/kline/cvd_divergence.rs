use crate::chart::{
    Caches, Message, ViewState,
    indicator::{
        kline::{
            AvailabilityCause, BasisSeries, BasisSeriesExt, IndicatorAvailability,
            KlineIndicatorImpl,
        },
        plot::Series,
    },
};
use data::chart::{
    PlotData,
    kline::{KlineDataPoint, KlineTrades},
};
use exchange::{Kline, Trade, Volume, unit::Qty};
use iced::{
    Point,
    widget::canvas::Frame,
};
use std::collections::BTreeSet;
use std::ops::RangeInclusive;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DivergenceType {
    Bullish, // Price down, Delta up
    Bearish, // Price up, Delta down
}

#[derive(Debug, Clone, Copy)]
pub struct DivPoint {
    pub kline: Kline,
    pub div_type: DivergenceType,
}

pub struct CVDDivergenceIndicator {
    cache: Caches,
    data: BasisSeries<DivPoint>,
    availability: IndicatorAvailability,
}

impl CVDDivergenceIndicator {
    pub fn new() -> Self {
        Self {
            cache: Caches::default(),
            data: BasisSeries::default(),
            availability: IndicatorAvailability::Unknown,
        }
    }

    fn check_divergence(kline: &Kline, footprint: &KlineTrades) -> Option<DivergenceType> {
        let delta = if footprint.trades.is_empty() {
            kline
                .volume
                .buy_sell()
                .map(|(b, s)| b - s)
                .unwrap_or(Qty::ZERO)
        } else {
            footprint
                .trades
                .values()
                .fold(Qty::ZERO, |acc, g| acc + g.delta_qty())
        };

        if delta == Qty::ZERO {
            return None;
        }

        let is_bullish_candle = kline.close > kline.open;
        let is_bearish_candle = kline.close < kline.open;

        if is_bearish_candle && delta > Qty::ZERO {
            Some(DivergenceType::Bearish)
        } else if is_bullish_candle && delta < Qty::ZERO {
            Some(DivergenceType::Bullish)
        } else {
            None
        }
    }

    fn has_directional_volume(volume: Volume) -> bool {
        volume.buy_sell().is_some()
    }

    fn is_directional_parts(footprint: &KlineTrades, volume: Volume) -> bool {
        !footprint.trades.is_empty() || Self::has_directional_volume(volume)
    }
}

impl KlineIndicatorImpl for CVDDivergenceIndicator {
    fn is_overlay(&self) -> bool {
        true
    }

    fn draw_overlay(
        &self,
        frame: &mut Frame,
        chart: &ViewState,
        theme: &iced::Theme,
        visible_range: RangeInclusive<u64>,
    ) {
        let palette = theme.extended_palette();
        let series = self.data.as_plot_series();
        let arrow_size = chart.cell_width.max(6.0).min(12.0);

        series.for_each_in(visible_range, |x, y| {
            let cx = chart.interval_to_x(x);

            match y.div_type {
                DivergenceType::Bullish => {
                    let sy = chart.price_to_y(y.kline.low) + arrow_size * 0.5;
                    let mut path = iced::widget::canvas::path::Builder::new();
                    path.move_to(Point::new(cx, sy));
                    path.line_to(Point::new(cx - arrow_size / 2.0, sy + arrow_size));
                    path.line_to(Point::new(cx + arrow_size / 2.0, sy + arrow_size));
                    path.close();

                    frame.fill(&path.build(), palette.success.strong.color);
                }
                DivergenceType::Bearish => {
                    let sy = chart.price_to_y(y.kline.high) - arrow_size * 0.5;
                    let mut path = iced::widget::canvas::path::Builder::new();
                    path.move_to(Point::new(cx, sy));
                    path.line_to(Point::new(cx - arrow_size / 2.0, sy - arrow_size));
                    path.line_to(Point::new(cx + arrow_size / 2.0, sy - arrow_size));
                    path.close();

                    frame.fill(&path.build(), palette.danger.strong.color);
                }
            }
        });
    }

    fn clear_all_caches(&mut self) {
        self.cache.clear_all();
    }

    fn clear_crosshair_caches(&mut self) {
        self.cache.clear_crosshair();
    }

    fn element<'a>(
        &'a self,
        _chart: &'a ViewState,
        _data_labels_always_visible: bool,
        _visible_range: RangeInclusive<u64>,
    ) -> iced::Element<'a, Message> {
        iced::widget::Space::new().into()
    }

    fn availability(&self, _chart: &ViewState) -> IndicatorAvailability {
        self.availability.clone()
    }

    fn rebuild_from_source(&mut self, source: &PlotData<KlineDataPoint>) {
        self.data = source.map_basis_series(
            |timeseries| {
                let mut map = std::collections::BTreeMap::new();
                for (&time, dp) in &timeseries.datapoints {
                    if let Some(div_type) = Self::check_divergence(&dp.kline, &dp.footprint) {
                        map.insert(
                            time,
                            DivPoint {
                                kline: dp.kline,
                                div_type,
                            },
                        );
                    }
                }
                map
            },
            |tickseries| {
                let mut map = std::collections::BTreeMap::new();
                for (idx, dp) in tickseries.datapoints.iter().enumerate() {
                    if let Some(div_type) = Self::check_divergence(&dp.kline, &dp.footprint) {
                        map.insert(
                            idx as u64,
                            DivPoint {
                                kline: dp.kline,
                                div_type,
                            },
                        );
                    }
                }
                map
            },
        );

        let (has_points, has_directional) = match source {
            PlotData::TimeBased(timeseries) => {
                let has_points = !timeseries.datapoints.is_empty();
                let has_directional = timeseries
                    .datapoints
                    .values()
                    .any(|dp| Self::is_directional_parts(&dp.footprint, dp.kline.volume));
                (has_points, has_directional)
            }
            PlotData::TickBased(tickseries) => {
                let has_points = !tickseries.datapoints.is_empty();
                let has_directional = tickseries
                    .datapoints
                    .iter()
                    .any(|dp| Self::is_directional_parts(&dp.footprint, dp.kline.volume));
                (has_points, has_directional)
            }
        };

        self.availability = if !has_points {
            IndicatorAvailability::Unknown
        } else if has_directional {
            IndicatorAvailability::Available
        } else {
            IndicatorAvailability::Unavailable(AvailabilityCause::TradeData)
        };

        self.clear_all_caches();
    }

    fn on_insert_klines(&mut self, klines: &[Kline], source: &PlotData<KlineDataPoint>) {
        let has_data = {
            let PlotData::TimeBased(timeseries) = source else {
                return;
            };

            let Some(data) = self.data.time_mut() else {
                return;
            };

            let mut directional = false;

            for kline in klines {
                let footprint = timeseries
                    .datapoints
                    .get(&kline.time)
                    .map(|dp| &dp.footprint);

                let dummy_fp = KlineTrades::default();
                let fp = footprint.unwrap_or(&dummy_fp);

                if Self::is_directional_parts(fp, kline.volume) {
                    directional = true;
                }

                if let Some(div_type) = Self::check_divergence(kline, fp) {
                    data.insert(
                        kline.time,
                        DivPoint {
                            kline: *kline,
                            div_type,
                        },
                    );
                } else {
                    data.remove(&kline.time);
                }
            }

            if directional {
                self.availability = IndicatorAvailability::Available;
            }

            !data.is_empty()
        };

        if self.availability == IndicatorAvailability::Unknown && has_data {
            self.availability = IndicatorAvailability::Unavailable(AvailabilityCause::TradeData);
        }

        self.clear_all_caches();
    }

    fn on_insert_trades(
        &mut self,
        trades: &[Trade],
        old_dp_len: usize,
        source: &PlotData<KlineDataPoint>,
    ) {
        let mut touched = false;

        match source {
            PlotData::TimeBased(timeseries) => {
                if trades.is_empty() {
                    return;
                }

                let Some(data) = self.data.time_mut() else {
                    return;
                };

                let mut touched_times = BTreeSet::new();
                for trade in trades {
                    let rounded_time = trade.time.floor_to(timeseries.interval);
                    touched_times.insert(rounded_time);
                }

                for time in touched_times {
                    if let Some(dp) = timeseries.datapoints.get(&time) {
                        if let Some(div_type) = Self::check_divergence(&dp.kline, &dp.footprint) {
                            data.insert(
                                time,
                                DivPoint {
                                    kline: dp.kline,
                                    div_type,
                                },
                            );
                        } else {
                            data.remove(&time);
                        }
                        touched = true;
                    }
                }
            }
            PlotData::TickBased(tickseries) => {
                let Some(data) = self.data.tick_mut() else {
                    return;
                };

                let start_idx = old_dp_len.saturating_sub(1);

                for (idx, dp) in tickseries.datapoints.iter().enumerate().skip(start_idx) {
                    if let Some(div_type) = Self::check_divergence(&dp.kline, &dp.footprint) {
                        data.insert(
                            idx as u64,
                            DivPoint {
                                kline: dp.kline,
                                div_type,
                            },
                        );
                    } else {
                        data.remove(&(idx as u64));
                    }
                    touched = true;
                }
            }
        }

        if touched {
            self.availability = IndicatorAvailability::Available;
            self.clear_all_caches();
        }
    }

    fn on_ticksize_change(&mut self, source: &PlotData<KlineDataPoint>) {
        self.rebuild_from_source(source);
    }

    fn on_basis_change(&mut self, source: &PlotData<KlineDataPoint>) {
        self.rebuild_from_source(source);
    }
}
