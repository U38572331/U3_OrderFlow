use std::collections::HashMap;

use iced::{
    widget::canvas::{Frame, Path, Stroke},
    Color,
};

use crate::{
    chart::{
        indicator::kline::KlineIndicatorImpl,
        ViewState, Message,
    },
    connector::gex_client::{GexEvent, GexLevel},
};
use exchange::unit::price::Price;


pub struct GexLevelsIndicator {
    levels: HashMap<String, GexLevel>,
}

impl GexLevelsIndicator {
    pub fn new() -> Self {
        Self {
            levels: HashMap::new(),
        }
    }
    
    fn color_for_level(&self, level: &GexLevel) -> Color {
        match level.kind.as_str() {
            "gamma" => {
                if level.value >= 0.0 {
                    Color::from_rgb8(50, 205, 50) // LimeGreen
                } else {
                    Color::from_rgb8(255, 0, 0) // Red
                }
            }
            "oi" => {
                if level.value >= 0.0 {
                    Color::from_rgb8(0, 191, 255) // DeepSkyBlue
                } else {
                    Color::from_rgb8(65, 105, 225) // RoyalBlue
                }
            }
            "call_wall" => Color::from_rgb8(54, 198, 144),
            "put_wall" => Color::from_rgb8(224, 93, 93),
            "zero_gamma" => Color::from_rgb8(245, 194, 107),
            "max_gamma" => Color::from_rgb8(74, 163, 255),
            "top_abs" => Color::from_rgb8(94, 200, 216),
            _ => Color::WHITE,
        }
    }
}

impl KlineIndicatorImpl for GexLevelsIndicator {
    fn clear_all_caches(&mut self) {
        self.levels.clear();
    }

    fn clear_crosshair_caches(&mut self) {}

    fn is_overlay(&self) -> bool {
        true
    }

    fn element<'a>(
        &'a self,
        _chart: &'a ViewState,
        _data_labels_always_visible: bool,
        _visible_range: std::ops::RangeInclusive<u64>,
    ) -> iced::Element<'a, Message> {
        iced::widget::Space::new().into()
    }

    fn draw_overlay(
        &self,
        frame: &mut Frame,
        chart: &ViewState,
        _theme: &iced::Theme,
        _visible_range: std::ops::RangeInclusive<u64>,
    ) {
        let bounds = chart.bounds;
        let width = bounds.width;

        // Find max absolute value to normalize bar widths
        let mut max_abs_value = 0.0f64;
        for level in self.levels.values() {
            let abs_val = level.value.abs();
            if abs_val > max_abs_value {
                max_abs_value = abs_val;
            }
        }
        
        let max_bar_width = 120.0;

        for level in self.levels.values() {
            let price = Price::from_f32(level.strike_ndx as f32);
            let y = chart.price_to_y(price);
            
            // Only draw if roughly visible
            if y < -50.0 || y > bounds.height + 50.0 {
                continue;
            }

            let color = self.color_for_level(level);

            // Draw Line
            let line_path = Path::line(
                iced::Point::new(0.0, y),
                iced::Point::new(width, y),
            );
            frame.stroke(
                &line_path,
                Stroke {
                    style: iced::widget::canvas::Style::Solid(color),
                    width: 1.0,
                    ..Default::default()
                },
            );

            // Draw Bar (Histogram style anchored to right)
            if max_abs_value > 0.0 {
                let bar_width = (level.value.abs() / max_abs_value) as f32 * max_bar_width;
                if bar_width > 2.0 {
                    let bar_path = Path::line(
                        iced::Point::new(width - bar_width, y),
                        iced::Point::new(width, y),
                    );
                    frame.stroke(
                        &bar_path,
                        Stroke {
                            style: iced::widget::canvas::Style::Solid(color),
                            width: 3.0,
                            ..Default::default()
                        },
                    );
                }
            }
        }
    }

    fn on_gex_event(&mut self, event: &crate::connector::gex_client::GexEvent) {
        match event {
            GexEvent::Set(level) => {
                self.levels.insert(level.id.clone(), level.clone());
            }
            GexEvent::Remove(id) => {
                self.levels.remove(id);
            }
            GexEvent::Clear | GexEvent::Disconnected => {
                self.levels.clear();
            }
            _ => {}
        }
    }
}
