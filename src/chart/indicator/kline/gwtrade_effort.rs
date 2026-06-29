use std::ops::RangeInclusive;

use iced::{
    widget::canvas::{Frame, Path, Stroke, Fill},
    Color,
};

use exchange::{Kline, Trade};
use data::chart::{PlotData, kline::KlineDataPoint};
use exchange::unit::price::Price;

use crate::chart::{
    indicator::kline::KlineIndicatorImpl,
    ViewState, Message,
};

#[derive(Debug, Clone)]
struct EffortZone {
    start_time: u64,
    end_time: u64,
    is_bullish: bool,
    high: f64,
    low: f64,
    streak_length: usize,
}

pub struct GWTradeEffortIndicator {
    zones: Vec<EffortZone>,
    
    // Configurations
    min_delta_perc: f64,
    max_delta_perc: f64,
    max_delta_effort: f64,
    max_time_seconds: f64,
    minimum_bars: usize,
    zone_max_extensions: usize,

    // State
    streak_count: usize,
    last_direction_bull: bool,
    streak_high: f64,
    streak_low: f64,
    streak_start_time: u64,
    
    streak_active: bool,
    dev_active: bool,
    dev_bull: bool,
    dev_high: f64,
    dev_low: f64,
    dev_start_time: u64,
    latest_kline_time: u64,
}

impl GWTradeEffortIndicator {
    pub fn new() -> Self {
        Self {
            zones: Vec::new(),
            
            // Default configuration
            min_delta_perc: 0.0, // 0 disables
            max_delta_perc: 0.0, // 0 disables
            max_delta_effort: 0.0, // 0 disables
            max_time_seconds: 0.0, // 0 disables
            minimum_bars: 2,
            zone_max_extensions: 10,
            
            streak_count: 0,
            last_direction_bull: false,
            streak_high: 0.0,
            streak_low: f64::MAX,
            streak_start_time: 0,
            
            streak_active: false,
            dev_active: false,
            dev_bull: false,
            dev_high: 0.0,
            dev_low: 0.0,
            dev_start_time: 0,
            latest_kline_time: 0,
        }
    }
    
    fn process_kline(&mut self, kline: &Kline, tick_size: f64) {
        self.latest_kline_time = kline.time.into();
        let volume = kline.volume.total().to_f32_lossy() as f64;
        if volume == 0.0 {
            return;
        }
        
        let open = kline.open.to_f32_lossy() as f64;
        let close = kline.close.to_f32_lossy() as f64;
        let high = kline.high.to_f32_lossy() as f64;
        let low = kline.low.to_f32_lossy() as f64;
        
        // Buy - Sell volume = Delta
        // If not available, we can't calculate. For now we assume we have buy/sell volume
        let (buy, sell) = kline.volume.buy_sell().unwrap_or((0.0.into(), 0.0.into()));
        let delta = f32::from(buy) as f64 - f32::from(sell) as f64;
        
        let delta_perc = delta.abs() / volume * 100.0;
        let body_ticks = ((open - close).abs() / tick_size).round();
        let delta_effort = delta / (body_ticks + 1.0);
        
        let delta_matches_candle = if delta >= 0.0 { close >= open } else { close < open };
        
        let flag1 = self.min_delta_perc == 0.0 || delta_perc >= self.min_delta_perc;
        let flag2 = self.max_delta_perc == 0.0 || delta_perc <= self.max_delta_perc;
        let flag3 = self.max_delta_effort == 0.0 || delta_effort.abs() <= self.max_delta_effort;
        let flag4 = self.max_time_seconds == 0.0 || (u64::from(kline.time).saturating_sub(self.streak_start_time)) as f64 / 1000.0 <= self.max_time_seconds;
        
        if delta_matches_candle && flag1 && flag2 && flag3 && flag4 {
            let is_bullish = delta >= 0.0;
            if is_bullish != self.last_direction_bull {
                self.streak_count = 0;
            }
            self.last_direction_bull = is_bullish;
            if self.streak_count == 0 {
                self.streak_start_time = kline.time.into();
            }
            self.streak_count += 1;
        } else {
            self.streak_count = 0;
        }
        
        if self.streak_count >= self.minimum_bars {
            if !self.streak_active {
                self.streak_active = true;
                self.streak_high = high;
                self.streak_low = low;
            } else {
                self.streak_high = self.streak_high.max(high);
                self.streak_low = self.streak_low.min(low);
            }
            
            self.dev_active = true;
            self.dev_bull = self.last_direction_bull;
            self.dev_high = self.streak_high;
            self.dev_low = self.streak_low;
            self.dev_start_time = self.streak_start_time;
            
        } else if self.streak_active {
            self.streak_active = false;
            self.dev_active = false;
            
            self.zones.push(EffortZone {
                start_time: self.streak_start_time,
                end_time: kline.time.into(), // ending time
                is_bullish: self.last_direction_bull,
                high: self.streak_high,
                low: self.streak_low,
                streak_length: self.streak_count,
            });
        } else {
            self.dev_active = false;
        }
    }
}

impl KlineIndicatorImpl for GWTradeEffortIndicator {
    fn clear_all_caches(&mut self) {
        self.zones.clear();
        self.dev_start_time = 0;
        self.latest_kline_time = 0;
        self.streak_count = 0;
        self.streak_active = false;
        self.dev_active = false;
    }

    fn clear_crosshair_caches(&mut self) {}

    fn is_overlay(&self) -> bool {
        true
    }

    fn element<'a>(
        &'a self,
        _chart: &'a ViewState,
        _data_labels_always_visible: bool,
        _visible_range: RangeInclusive<u64>,
    ) -> iced::Element<'a, Message> {
        iced::widget::Space::new().into()
    }

    fn draw_overlay(
        &self,
        frame: &mut Frame,
        chart: &ViewState,
        _theme: &iced::Theme,
        visible_range: RangeInclusive<u64>,
    ) {
        let draw_zone = |start_time: u64, end_time: u64, high: f64, low: f64, is_bullish: bool, frame: &mut Frame| {
            // Find X positions
            let start_x = chart.interval_to_x(start_time);
            let end_x = chart.interval_to_x(end_time) + (self.zone_max_extensions as f32 * chart.cell_width);
            
            let high_price = Price::from_f32(high as f32);
            let low_price = Price::from_f32(low as f32);
            let y1 = chart.price_to_y(high_price);
            let y2 = chart.price_to_y(low_price);
            
            let rect = iced::Rectangle {
                x: start_x,
                y: y1.min(y2),
                width: end_x - start_x,
                height: (y2 - y1).abs(),
            };
            
            let fill_color = if is_bullish {
                Color::from_rgba8(50, 205, 50, 0.2) // Bullish green, 20% opacity
            } else {
                Color::from_rgba8(255, 0, 0, 0.2) // Bearish red, 20% opacity
            };
            
            let stroke_color = if is_bullish {
                Color::from_rgba8(50, 205, 50, 0.8)
            } else {
                Color::from_rgba8(255, 0, 0, 0.8)
            };
            
            let path = Path::rectangle(iced::Point::new(rect.x, rect.y), iced::Size::new(rect.width, rect.height));
            frame.fill(&path, fill_color);
            frame.stroke(
                &path,
                Stroke {
                    style: iced::widget::canvas::Style::Solid(stroke_color),
                    width: 1.0,
                    ..Default::default()
                },
            );
        };
        
        for zone in &self.zones {
            draw_zone(zone.start_time, zone.end_time, zone.high, zone.low, zone.is_bullish, frame);
        }
        
        if self.dev_active {
            draw_zone(self.dev_start_time, self.latest_kline_time.max(self.dev_start_time), self.dev_high, self.dev_low, self.dev_bull, frame);
        }
    }

    fn rebuild_from_source(&mut self, source: &PlotData<KlineDataPoint>) {
        self.zones.clear();
        self.streak_count = 0;
        self.streak_active = false;
        self.dev_active = false;
        
        let timeseries = match source {
            data::chart::PlotData::TimeBased(ts) => ts,
            _ => return,
        };
        
        let klines = &timeseries.datapoints;
        if klines.is_empty() {
            return;
        }
        
        // Find tick size from first diff or from chart config? We don't have chart config here easily.
        // Assuming tick_size is 0.25 (NQ) for now, wait we can calculate from high/low diff or pass it via view state?
        // Let's use a very small tick size (0.01) if not known, it just changes effort scale.
        // Actually we don't have tick_size in rebuild_from_source. I'll hardcode 0.25 as a default for NQ.
        let tick_size = 0.25; 
        
        for (_, dp) in klines {
            self.process_kline(&dp.kline, tick_size);
        }
    }

    fn on_insert_klines(&mut self, klines: &[Kline], _source: &PlotData<KlineDataPoint>) {
        let tick_size = 0.25;
        for kline in klines {
            self.process_kline(kline, tick_size);
        }
    }
}
