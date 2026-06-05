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
use data::chart::{PlotData, kline::KlineDataPoint};
use exchange::{Kline, Trade, UnixMs, unit::Price};
use iced::{
    Point,
    widget::canvas::{Frame, Path, Stroke},
};
use std::collections::BTreeSet;
use std::ops::RangeInclusive;

#[derive(Debug, Clone, Copy, Default)]
pub struct VWAPPoint {
    pub cumulative_vol_price: f64,
    pub cumulative_vol: f64,
    pub vwap: f32,
}

pub struct VWAPIndicator {
    cache: Caches,
    data: BasisSeries<VWAPPoint>,
    availability: IndicatorAvailability,
    deltas: BasisSeries<(UnixMs, f64, f64)>, // (time, vol_price, vol)
}

impl VWAPIndicator {
    pub fn new() -> Self {
        Self {
            cache: Caches::default(),
            data: BasisSeries::default(),
            availability: IndicatorAvailability::Unknown,
            deltas: BasisSeries::default(),
        }
    }

    fn rebuild_cumulative(&mut self) {
        let mut vol_price = 0.0;
        let mut vol = 0.0;
        let mut current_day = None;

        self.data = self.deltas.map(|&(time, vp, v)| {
            let day = time.as_datetime_utc().map(|dt| dt.date_naive());
            if current_day != day || current_day.is_none() {
                current_day = day;
                vol_price = vp;
                vol = v;
            } else {
                vol_price += vp;
                vol += v;
            }

            let vwap = if vol > 0.0 {
                (vol_price / vol) as f32
            } else {
                0.0
            };

            VWAPPoint {
                cumulative_vol_price: vol_price,
                cumulative_vol: vol,
                vwap,
            }
        });

        self.clear_all_caches();
    }
}

impl KlineIndicatorImpl for VWAPIndicator {
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
        let stroke = Stroke {
            style: iced::widget::canvas::Style::Solid(theme.extended_palette().secondary.strong.color),
            width: 1.5,
            ..Default::default()
        };

        let mut prev: Option<(f32, f32)> = None;
        let series = self.data.as_plot_series();

        series.for_each_in(visible_range, |x, y| {
            if y.vwap > 0.0 {
                let sx = chart.interval_to_x(x);
                let sy = chart.price_to_y(Price::from_f32(y.vwap));

                if let Some((px, py)) = prev {
                    frame.stroke(
                        &Path::line(Point::new(px, py), Point::new(sx, sy)),
                        stroke.clone(),
                    );
                }
                prev = Some((sx, sy));
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
        let deltas = source.map_basis_series(
            |timeseries| {
                timeseries
                    .datapoints
                    .iter()
                    .map(|(&time, dp)| {
                        let k = &dp.kline;
                        let typical = (k.high.to_f32() as f64 + k.low.to_f32() as f64 + k.close.to_f32() as f64) / 3.0;
                        let v = k.volume.total().to_f32_lossy() as f64;
                        (time, (time, typical * v, v))
                    })
                    .collect()
            },
            |tickseries| {
                tickseries
                    .datapoints
                    .iter()
                    .enumerate()
                    .map(|(idx, dp)| {
                        let k = &dp.kline;
                        let typical = (k.high.to_f32() as f64 + k.low.to_f32() as f64 + k.close.to_f32() as f64) / 3.0;
                        let v = k.volume.total().to_f32_lossy() as f64;
                        (idx as u64, (k.time, typical * v, v))
                    })
                    .collect()
            },
        );

        let has_points = match source {
            PlotData::TimeBased(ts) => !ts.datapoints.is_empty(),
            PlotData::TickBased(ts) => !ts.datapoints.is_empty(),
        };

        self.availability = if has_points {
            IndicatorAvailability::Available
        } else {
            IndicatorAvailability::Unknown
        };

        self.deltas = deltas;
        self.rebuild_cumulative();
    }

    fn on_insert_klines(&mut self, klines: &[Kline], source: &PlotData<KlineDataPoint>) {
        let has_data = {
            let PlotData::TimeBased(_) = source else {
                return;
            };

            let Some(deltas) = self.deltas.time_mut() else {
                return;
            };

            for kline in klines {
                let typical = (kline.high.to_f32() as f64 + kline.low.to_f32() as f64 + kline.close.to_f32() as f64) / 3.0;
                let v = kline.volume.total().to_f32_lossy() as f64;
                deltas.insert(kline.time, (kline.time, typical * v, v));
            }

            !deltas.is_empty()
        };

        if has_data {
            self.availability = IndicatorAvailability::Available;
            self.rebuild_cumulative();
        }
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

                let Some(deltas) = self.deltas.time_mut() else {
                    return;
                };

                let mut touched_times = BTreeSet::new();
                for trade in trades {
                    let rounded_time = trade.time.floor_to(timeseries.interval);
                    touched_times.insert(rounded_time);
                }

                for time in touched_times {
                    if let Some(dp) = timeseries.datapoints.get(&time) {
                        let k = &dp.kline;
                        let typical = (k.high.to_f32() as f64 + k.low.to_f32() as f64 + k.close.to_f32() as f64) / 3.0;
                        let v = k.volume.total().to_f32_lossy() as f64;
                        deltas.insert(time, (time, typical * v, v));
                        touched = true;
                    }
                }
            }
            PlotData::TickBased(tickseries) => {
                let Some(deltas) = self.deltas.tick_mut() else {
                    return;
                };

                let start_idx = old_dp_len.saturating_sub(1);

                for (idx, dp) in tickseries.datapoints.iter().enumerate().skip(start_idx) {
                    let k = &dp.kline;
                    let typical = (k.high.to_f32() as f64 + k.low.to_f32() as f64 + k.close.to_f32() as f64) / 3.0;
                    let v = k.volume.total().to_f32_lossy() as f64;
                    deltas.insert(idx as u64, (k.time, typical * v, v));
                    touched = true;
                }
            }
        }

        if touched {
            self.availability = IndicatorAvailability::Available;
            self.rebuild_cumulative();
        }
    }

    fn on_ticksize_change(&mut self, source: &PlotData<KlineDataPoint>) {
        self.rebuild_from_source(source);
    }

    fn on_basis_change(&mut self, source: &PlotData<KlineDataPoint>) {
        self.rebuild_from_source(source);
    }
}
