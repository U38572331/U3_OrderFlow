use super::{
    Action, Basis, Chart, Interaction, Message, PlotConstants, PlotData, TEXT_SIZE, ViewState,
    indicator, request_fetch, scale::linear::PriceInfoLabel,
};
use crate::chart::indicator::kline::KlineIndicatorImpl;
use crate::connector::fetcher::{FetchRange, RequestHandler, is_trade_fetch_enabled};
use crate::{modal::pane::settings::study, style};
use data::aggr::ticks::TickAggr;
use data::aggr::time::TimeSeries;
use data::chart::indicator::{Indicator, KlineIndicator};
use data::chart::kline::{
    ClusterKind, ClusterScaling, Config, FootprintStudy, KlineDataPoint, KlineTrades, NPoc,
    PointOfControl,
};
use data::chart::{Autoscale, KlineChartKind, ViewConfig};

use data::util::abbr_large_numbers;
use exchange::unit::{Price, PriceStep, Qty};
use exchange::{Kline, OpenInterest as OIData, TickerInfo, Trade, UnixMs};

use iced::task::Handle;
use iced::theme::palette::Extended;
use iced::widget::canvas::{self, Event, Geometry, Path, Stroke};
use iced::{Alignment, Element, Point, Rectangle, Renderer, Size, Theme, Vector, mouse};

use enum_map::EnumMap;
use std::time::Instant;

impl Chart for KlineChart {
    type IndicatorKind = KlineIndicator;

    fn state(&self) -> &ViewState {
        &self.chart
    }

    fn mut_state(&mut self) -> &mut ViewState {
        &mut self.chart
    }

    fn invalidate_crosshair(&mut self) {
        self.chart.cache.clear_crosshair();
        self.indicators
            .values_mut()
            .filter_map(Option::as_mut)
            .for_each(|indi| indi.clear_crosshair_caches());
    }

    fn invalidate_all(&mut self) {
        self.invalidate(None);
    }

    fn view_indicators(&'_ self, enabled: &[Self::IndicatorKind]) -> Vec<Element<'_, Message>> {
        let chart_state = self.state();
        let visible_region = chart_state.visible_region(chart_state.bounds.size());
        let (earliest, latest) = chart_state.interval_range(&visible_region);
        if earliest > latest {
            return vec![];
        }

        let data_labels_always_visible = self.visual_config.data_labels_always_visible;

        let market = chart_state.ticker_info.market_type();
        let mut elements = vec![];

        for selected_indicator in enabled {
            if !KlineIndicator::for_market(market).contains(selected_indicator) {
                continue;
            }
            if let Some(indi) = self.indicators[*selected_indicator].as_ref() {
                if !indi.is_overlay() {
                    elements.push(indi.element(
                        chart_state,
                        data_labels_always_visible,
                        earliest..=latest,
                    ));
                }
            }
        }
        elements
    }

    fn visible_timerange(&self) -> Option<(u64, u64)> {
        let chart = self.state();
        let region = chart.visible_region(chart.bounds.size());

        if region.width == 0.0 {
            return None;
        }

        Some(chart.interval_range(&region))
    }

    fn interval_keys(&self) -> Option<Vec<u64>> {
        match &self.data_source {
            PlotData::TimeBased(_) => None,
            PlotData::TickBased(tick_aggr) => Some(
                tick_aggr
                    .datapoints
                    .iter()
                    .map(|dp| dp.kline.time.as_u64())
                    .collect(),
            ),
        }
    }

    fn autoscaled_coords(&self) -> Vector {
        let chart = self.state();
        let x_translation = match &self.kind {
            KlineChartKind::Footprint { .. } => {
                0.5 * (chart.bounds.width / chart.scaling) - (chart.cell_width / chart.scaling)
            }
            KlineChartKind::Candles => {
                0.5 * (chart.bounds.width / chart.scaling)
                    - (8.0 * chart.cell_width / chart.scaling)
            }
        };
        Vector::new(x_translation, chart.translation.y)
    }

    fn supports_fit_autoscaling(&self) -> bool {
        true
    }

    fn is_empty(&self) -> bool {
        match &self.data_source {
            PlotData::TimeBased(timeseries) => timeseries.datapoints.is_empty(),
            PlotData::TickBased(tick_aggr) => tick_aggr.datapoints.is_empty(),
        }
    }

    fn drawing_tool(&self) -> data::chart::drawing::DrawingType {
        self.drawing_tool
    }

    fn handle_drawing(&mut self, event: iced::mouse::Event, cursor: iced::Point) {
        let state = &self.chart;
        let bounds_size = state.bounds.size();
        let region = state.visible_region(bounds_size);
        
        if self.drawing_tool == data::chart::drawing::DrawingType::Cursor {
            if let iced::mouse::Event::ButtonPressed(iced::mouse::Button::Left) = event {
                // Hit test drawings
                let mut hit_index = None;
                
                for (i, drawing) in self.drawings.drawings.iter().enumerate().rev() {
                    let mut points = match &drawing.state {
                        data::chart::drawing::DrawingState::Initial => continue,
                        data::chart::drawing::DrawingState::OnePoint(p) => vec![*p],
                        data::chart::drawing::DrawingState::Completed(p1, p2) => vec![*p1, *p2],
                    };
                    
                    if points.len() == 2 {
                        let p1 = chart_point_to_screen(&points[0], state, bounds_size);
                        let p2 = chart_point_to_screen(&points[1], state, bounds_size);
                        
                        let dist = match drawing.kind {
                            data::chart::drawing::DrawingType::TrendLine | data::chart::drawing::DrawingType::Ray => {
                                distance_to_segment(cursor, p1, p2)
                            }
                            data::chart::drawing::DrawingType::HorizontalLine => {
                                (cursor.y - p1.y).abs()
                            }
                            _ => f32::MAX, // fallback
                        };
                        
                        if dist < 8.0 {
                            hit_index = Some(i);
                            break;
                        }
                    } else if points.len() == 1 {
                        let p1 = chart_point_to_screen(&points[0], state, bounds_size);
                        if ((cursor.x - p1.x).powi(2) + (cursor.y - p1.y).powi(2)).sqrt() < 8.0 {
                            hit_index = Some(i);
                            break;
                        }
                    }
                }
                
                let mut changed = false;
                for (i, drawing) in self.drawings.drawings.iter_mut().enumerate() {
                    let should_be_selected = Some(i) == hit_index;
                    if drawing.selected != should_be_selected {
                        drawing.selected = should_be_selected;
                        changed = true;
                    }
                }
                
                if changed {
                    self.invalidate_all();
                }
            }
            return;
        }
        
        let (timestamp, _) = state.snap_x_to_index(cursor.x, bounds_size, region);
        
        let ratio = cursor.y / bounds_size.height;
        let (highest_p, lowest_p) = state.price_range(&region);
        let highest = highest_p.to_f32_lossy();
        let lowest = lowest_p.to_f32_lossy();
        
        let price_f32 = highest + ratio * (lowest - highest);
        let price = exchange::unit::Price::from_f32_lossy(price_f32);
        
        let effective_step = if state.tick_size.units > 0 {
            state.tick_size
        } else {
            state.ticker_info.min_ticksize.into()
        };
        
        let tick_units = effective_step.units;
        let tick_index = price.units.div_euclid(tick_units);
        let rounded_price = exchange::unit::Price::from_units(tick_index * tick_units);
        
        let chart_point = data::chart::drawing::ChartPoint {
            time: timestamp,
            price: rounded_price,
        };

        let drawing_event = match event {
            iced::mouse::Event::ButtonPressed(iced::mouse::Button::Left) => {
                data::chart::drawing::DrawingEvent::MouseDown(chart_point)
            }
            iced::mouse::Event::CursorMoved { .. } => {
                data::chart::drawing::DrawingEvent::MouseMove(chart_point)
            }
            iced::mouse::Event::ButtonReleased(iced::mouse::Button::Left) => {
                data::chart::drawing::DrawingEvent::MouseUp(chart_point)
            }
            _ => return,
        };

        if self.drawings.handle_event(drawing_event, self.drawing_tool) {
            self.invalidate_all();
        }
    }

    fn handle_drawing_key(&mut self, event: iced::keyboard::Event) {
        if let iced::keyboard::Event::KeyPressed { key, .. } = event {
            if matches!(key.as_ref(), iced::keyboard::Key::Named(iced::keyboard::key::Named::Delete) | iced::keyboard::Key::Named(iced::keyboard::key::Named::Backspace)) {
                if self.drawings.delete_selected() {
                    self.invalidate_all();
                }
            }
        }
    }
}

fn chart_point_to_screen(
    point: &data::chart::drawing::ChartPoint,
    state: &ViewState,
    bounds_size: iced::Size,
) -> iced::Point {
    let region = state.visible_region(bounds_size);
    let (highest_p, lowest_p) = state.price_range(&region);
    let highest = highest_p.to_f32_lossy();
    let lowest = lowest_p.to_f32_lossy();

    let chart_x = state.interval_to_x(point.time);
    
    let ratio_x = if region.width > 0.0 {
        (chart_x - region.x) / region.width
    } else {
        0.5
    };
    let screen_x = ratio_x * bounds_size.width;

    let price_f32 = point.price.to_f32_lossy();
    let ratio_y = if highest - lowest > 0.0 {
        (highest - price_f32) / (highest - lowest)
    } else {
        0.5
    };
    let screen_y = ratio_y * bounds_size.height;

    iced::Point::new(screen_x, screen_y)
}

fn distance_to_segment(p: iced::Point, v: iced::Point, w: iced::Point) -> f32 {
    let l2 = (v.x - w.x).powi(2) + (v.y - w.y).powi(2);
    if l2 == 0.0 {
        return ((p.x - v.x).powi(2) + (p.y - v.y).powi(2)).sqrt();
    }
    let t = ((p.x - v.x) * (w.x - v.x) + (p.y - v.y) * (w.y - v.y)) / l2;
    // Don't clamp for Ray, but clamp for TrendLine...
    // Actually, we'll just clamp to segment for now
    let t = t.clamp(0.0, 1.0);
    let projection = iced::Point::new(v.x + t * (w.x - v.x), v.y + t * (w.y - v.y));
    ((p.x - projection.x).powi(2) + (p.y - projection.y).powi(2)).sqrt()
}

impl PlotConstants for KlineChart {
    fn min_scaling(&self) -> f32 {
        self.kind.min_scaling()
    }

    fn max_scaling(&self) -> f32 {
        self.kind.max_scaling()
    }

    fn max_cell_width(&self) -> f32 {
        self.kind.max_cell_width()
    }

    fn min_cell_width(&self) -> f32 {
        self.kind.min_cell_width()
    }

    fn max_cell_height(&self) -> f32 {
        self.kind.max_cell_height()
    }

    fn min_cell_height(&self) -> f32 {
        self.kind.min_cell_height()
    }

    fn default_cell_width(&self) -> f32 {
        self.kind.default_cell_width()
    }
}

pub struct KlineChart {
    chart: ViewState,
    data_source: PlotData<KlineDataPoint>,
    raw_trades: Vec<Trade>,
    indicators: EnumMap<KlineIndicator, Option<Box<dyn KlineIndicatorImpl>>>,
    fetching_trades: (bool, Option<Handle>),
    pub(crate) kind: KlineChartKind,
    request_handler: RequestHandler,
    study_configurator: study::Configurator<FootprintStudy>,
    last_tick: Instant,
    visual_config: Config,
    pub drawing_tool: data::chart::drawing::DrawingType,
    pub drawings: data::chart::drawing::ChartDrawings,
}

impl KlineChart {
    pub fn new(
        layout: ViewConfig,
        basis: Basis,
        step: PriceStep,
        klines_raw: &[Kline],
        raw_trades: Vec<Trade>,
        enabled_indicators: &[KlineIndicator],
        ticker_info: TickerInfo,
        kind: &KlineChartKind,
        visual_config: Option<Config>,
    ) -> Self {
        let visual_config = visual_config.unwrap_or_default();

        match basis {
            Basis::Time(interval) => {
                let timeseries = TimeSeries::<KlineDataPoint>::new(interval, step, klines_raw)
                    .with_trades(&raw_trades);

                let base_price_y = timeseries.base_price();
                let latest_x = timeseries
                    .latest_timestamp()
                    .map_or(0, |timestamp| timestamp.as_u64());
                let (scale_high, scale_low) = timeseries.price_scale({
                    match kind {
                        KlineChartKind::Footprint { .. } => 12,
                        KlineChartKind::Candles => 60,
                    }
                });

                let low_rounded = scale_low.round_to_side_step(true, step);
                let high_rounded = scale_high.round_to_side_step(false, step);

                let y_ticks = Price::steps_between_inclusive(low_rounded, high_rounded, step)
                    .map(|n| n.saturating_sub(1))
                    .unwrap_or(1)
                    .max(1) as f32;

                let cell_width = match kind {
                    KlineChartKind::Footprint { .. } => 80.0,
                    KlineChartKind::Candles => 4.0,
                };
                let cell_height = match kind {
                    KlineChartKind::Footprint { .. } => 800.0 / y_ticks,
                    KlineChartKind::Candles => 200.0 / y_ticks,
                };

                let mut chart = ViewState::new(
                    basis,
                    step,
                    step.decimal_places(),
                    ticker_info,
                    ViewConfig {
                        splits: layout.splits.clone(),
                        autoscale: Some(Autoscale::FitToVisible),
                    },
                    cell_width,
                    cell_height,
                );
                chart.base_price_y = base_price_y;
                chart.latest_x = latest_x;

                let x_translation = match &kind {
                    KlineChartKind::Footprint { .. } => {
                        0.5 * (chart.bounds.width / chart.scaling)
                            - (chart.cell_width / chart.scaling)
                    }
                    KlineChartKind::Candles => {
                        0.5 * (chart.bounds.width / chart.scaling)
                            - (8.0 * chart.cell_width / chart.scaling)
                    }
                };
                chart.translation.x = x_translation;

                let data_source = PlotData::TimeBased(timeseries);

                let mut indicators = EnumMap::default();
                for &i in enabled_indicators {
                    let mut indi = indicator::kline::make_empty(i);
                    indi.rebuild_from_source(&data_source);
                    indicators[i] = Some(indi);
                }

                KlineChart {
                    chart,
                    visual_config,
                    data_source,
                    raw_trades,
                    indicators,
                    fetching_trades: (false, None),
                    request_handler: RequestHandler::default(),
                    kind: kind.clone(),
                    study_configurator: study::Configurator::new(),
                    last_tick: Instant::now(),
                    drawing_tool: data::chart::drawing::DrawingType::Cursor,
                    drawings: Default::default(),
                }
            }
            Basis::Tick(interval) => {
                let cell_width = match kind {
                    KlineChartKind::Footprint { .. } => 80.0,
                    KlineChartKind::Candles => 4.0,
                };
                let cell_height = match kind {
                    KlineChartKind::Footprint { .. } => 90.0,
                    KlineChartKind::Candles => 8.0,
                };

                let mut chart = ViewState::new(
                    basis,
                    step,
                    step.decimal_places(),
                    ticker_info,
                    ViewConfig {
                        splits: layout.splits.clone(),
                        autoscale: Some(Autoscale::FitToVisible),
                    },
                    cell_width,
                    cell_height,
                );

                let x_translation = match &kind {
                    KlineChartKind::Footprint { .. } => {
                        0.5 * (chart.bounds.width / chart.scaling)
                            - (chart.cell_width / chart.scaling)
                    }
                    KlineChartKind::Candles => {
                        0.5 * (chart.bounds.width / chart.scaling)
                            - (8.0 * chart.cell_width / chart.scaling)
                    }
                };
                chart.translation.x = x_translation;

                let data_source = PlotData::TickBased(TickAggr::new(interval, step, &raw_trades));

                let mut indicators = EnumMap::default();
                for &i in enabled_indicators {
                    let mut indi = indicator::kline::make_empty(i);
                    indi.rebuild_from_source(&data_source);
                    indicators[i] = Some(indi);
                }

                KlineChart {
                    chart,
                    visual_config,
                    data_source,
                    raw_trades,
                    indicators,
                    fetching_trades: (false, None),
                    request_handler: RequestHandler::default(),
                    kind: kind.clone(),
                    study_configurator: study::Configurator::new(),
                    last_tick: Instant::now(),
                    drawing_tool: data::chart::drawing::DrawingType::Cursor,
                    drawings: Default::default(),
                }
            }
        }
    }

    pub fn update_latest_kline(&mut self, kline: &Kline) {
        match self.data_source {
            PlotData::TimeBased(ref mut timeseries) => {
                timeseries.insert_klines(&[*kline]);

                self.indicators
                    .values_mut()
                    .filter_map(Option::as_mut)
                    .for_each(|indi| indi.on_insert_klines(&[*kline], &self.data_source));

                let chart = self.mut_state();

                if kline.time.as_u64() > chart.latest_x {
                    chart.latest_x = kline.time.as_u64();
                }

                chart.last_price = Some(PriceInfoLabel::new(kline.close, kline.open));
            }
            PlotData::TickBased(_) => {}
        }
    }

    pub fn kind(&self) -> &KlineChartKind {
        &self.kind
    }

    fn fetch_missing_data(&mut self) -> Option<Action> {
        match &self.data_source {
            PlotData::TimeBased(timeseries) => {
                let timeframe_ms = timeseries.interval.to_milliseconds();

                if timeseries.datapoints.is_empty() {
                    let latest = chrono::Utc::now().timestamp_millis() as u64;
                    let earliest = latest.saturating_sub(450 * timeframe_ms);

                    let range = FetchRange::Kline(UnixMs::new(earliest), UnixMs::new(latest));
                    if let Some(action) = request_fetch(&mut self.request_handler, range) {
                        return Some(action);
                    }
                }

                let (visible_earliest, visible_latest) = self.visible_timerange()?;
                let (kline_earliest, kline_latest) = timeseries.timerange();
                let visible_earliest_ms = UnixMs::new(visible_earliest);
                let visible_latest_ms = UnixMs::new(visible_latest);
                let visible_span = visible_latest.saturating_sub(visible_earliest);
                let prefetch_earliest = visible_earliest.saturating_sub(visible_span);

                // priority 1, initial klines for visible range
                if visible_earliest_ms < kline_earliest {
                    let range = FetchRange::Kline(UnixMs::new(prefetch_earliest), kline_earliest);

                    if let Some(action) = request_fetch(&mut self.request_handler, range) {
                        return Some(action);
                    }
                }

                // priority 2, trades
                if let KlineChartKind::Footprint { .. } = self.kind
                    && !self.fetching_trades.0
                    && is_trade_fetch_enabled()
                    && let Some((fetch_from, fetch_to)) =
                        timeseries.suggest_trade_fetch_range(visible_earliest_ms, visible_latest_ms)
                {
                    let range = FetchRange::Trades(fetch_from, fetch_to);
                    if let Some(action) = request_fetch(&mut self.request_handler, range) {
                        self.fetching_trades = (true, None);
                        return Some(action);
                    }
                }

                // priority 3, indicators
                // (e.g. open interest needs external fetch as it's not derived from klines)
                let ctx = indicator::kline::FetchCtx {
                    main_chart: &self.chart,
                    timeframe: timeseries.interval,
                    visible_earliest: visible_earliest_ms,
                    kline_latest,
                    prefetch_earliest: UnixMs::new(prefetch_earliest),
                };
                for indi in self.indicators.values_mut().filter_map(Option::as_mut) {
                    if let Some(range) = indi.fetch_range(&ctx)
                        && let Some(action) = request_fetch(&mut self.request_handler, range)
                    {
                        return Some(action);
                    }
                }

                // priority 4, missing klines & integrity check
                let check_earliest = UnixMs::new(prefetch_earliest).max(kline_earliest);
                let check_latest = visible_latest_ms.saturating_add(timeframe_ms);

                if let Some(missing_keys) =
                    timeseries.check_kline_integrity(check_earliest, check_latest)
                {
                    let latest = missing_keys
                        .iter()
                        .max()
                        .unwrap_or(&visible_latest_ms)
                        .saturating_add(timeframe_ms);
                    let earliest = missing_keys
                        .iter()
                        .min()
                        .unwrap_or(&visible_earliest_ms)
                        .saturating_sub(timeframe_ms);

                    let range = FetchRange::Kline(earliest, latest);
                    if let Some(action) = request_fetch(&mut self.request_handler, range) {
                        return Some(action);
                    }
                }
            }
            PlotData::TickBased(_) => {
                // TODO: implement trade fetch
            }
        }

        None
    }

    pub fn reset_request_handler(&mut self) {
        self.request_handler = RequestHandler::default();
        self.fetching_trades = (false, None);
    }

    pub fn raw_trades(&self) -> Vec<Trade> {
        self.raw_trades.clone()
    }

    pub fn set_handle(&mut self, handle: Handle) {
        self.fetching_trades.1 = Some(handle);
    }

    pub fn tick_size(&self) -> PriceStep {
        self.chart.tick_size
    }

    pub fn study_configurator(&self) -> &study::Configurator<FootprintStudy> {
        &self.study_configurator
    }

    pub fn update_study_configurator(&mut self, message: study::Message<FootprintStudy>) {
        let KlineChartKind::Footprint {
            ref mut studies, ..
        } = self.kind
        else {
            return;
        };

        match self.study_configurator.update(message) {
            Some(study::Action::ToggleStudy(study, is_selected)) => {
                if is_selected {
                    let already_exists = studies.iter().any(|s| s.is_same_type(&study));
                    if !already_exists {
                        studies.push(study);
                    }
                } else {
                    studies.retain(|s| !s.is_same_type(&study));
                }
            }
            Some(study::Action::ConfigureStudy(study)) => {
                if let Some(existing_study) = studies.iter_mut().find(|s| s.is_same_type(&study)) {
                    *existing_study = study;
                }
            }
            None => {}
        }

        self.invalidate(None);
    }

    pub fn chart_layout(&self) -> ViewConfig {
        self.chart.layout()
    }



    pub fn set_drawing_tool(&mut self, tool: data::chart::drawing::DrawingType) {
        self.drawing_tool = tool;
    }

    pub fn clear_drawings(&mut self) {
        self.drawings.clear();
        self.invalidate_all();
    }

    pub fn visual_config(&self) -> Config {
        self.visual_config
    }

    pub fn set_visual_config(&mut self, visual_config: Config) {
        self.visual_config = visual_config;
        self.chart.cache.clear_all();
        self.indicators
            .values_mut()
            .filter_map(Option::as_mut)
            .for_each(|indi| indi.clear_all_caches());
    }

    pub fn set_cluster_kind(&mut self, new_kind: ClusterKind) {
        if let KlineChartKind::Footprint {
            ref mut clusters, ..
        } = self.kind
        {
            *clusters = new_kind;
        }

        self.invalidate(None);
    }

    pub fn set_cluster_scaling(&mut self, new_scaling: ClusterScaling) {
        if let KlineChartKind::Footprint {
            ref mut scaling, ..
        } = self.kind
        {
            *scaling = new_scaling;
        }

        self.invalidate(None);
    }

    pub fn basis(&self) -> Basis {
        self.chart.basis
    }

    pub fn change_tick_size(&mut self, new_step: PriceStep) {
        let chart = self.mut_state();

        chart.cell_height *= (new_step.units as f32) / (chart.tick_size.units as f32);
        chart.tick_size = new_step;

        match self.data_source {
            PlotData::TickBased(ref mut tick_aggr) => {
                tick_aggr.change_tick_size(new_step, &self.raw_trades);
            }
            PlotData::TimeBased(ref mut timeseries) => {
                timeseries.change_tick_size(new_step, &self.raw_trades);
            }
        }

        self.indicators
            .values_mut()
            .filter_map(Option::as_mut)
            .for_each(|indi| indi.on_ticksize_change(&self.data_source));

        self.invalidate(None);
    }

    pub fn set_basis(&mut self, new_basis: Basis) -> Option<Action> {
        self.chart.last_price = None;
        self.chart.basis = new_basis;

        match new_basis {
            Basis::Time(interval) => {
                let step = self.chart.tick_size;
                let timeseries = TimeSeries::<KlineDataPoint>::new(interval, step, &[]);
                self.data_source = PlotData::TimeBased(timeseries);
            }
            Basis::Tick(tick_count) => {
                let step = self.chart.tick_size;
                let tick_aggr = TickAggr::new(tick_count, step, &self.raw_trades);
                self.data_source = PlotData::TickBased(tick_aggr);
            }
        }

        self.indicators
            .values_mut()
            .filter_map(Option::as_mut)
            .for_each(|indi| indi.on_basis_change(&self.data_source));

        self.reset_request_handler();
        self.invalidate(Some(Instant::now()))
    }

    pub fn studies(&self) -> Option<Vec<FootprintStudy>> {
        match &self.kind {
            KlineChartKind::Footprint { studies, .. } => Some(studies.clone()),
            _ => None,
        }
    }

    pub fn set_studies(&mut self, new_studies: Vec<FootprintStudy>) {
        if let KlineChartKind::Footprint {
            ref mut studies, ..
        } = self.kind
        {
            *studies = new_studies;
        }

        self.invalidate(None);
    }

    pub fn insert_trades(&mut self, buffer: &[Trade]) {
        self.raw_trades.extend_from_slice(buffer);

        match self.data_source {
            PlotData::TickBased(ref mut tick_aggr) => {
                let old_dp_len = tick_aggr.datapoints.len();
                tick_aggr.insert_trades(buffer);

                if let Some(last_dp) = tick_aggr.datapoints.last() {
                    self.chart.last_price =
                        Some(PriceInfoLabel::new(last_dp.kline.close, last_dp.kline.open));
                } else {
                    self.chart.last_price = None;
                }

                self.indicators
                    .values_mut()
                    .filter_map(Option::as_mut)
                    .for_each(|indi| indi.on_insert_trades(buffer, old_dp_len, &self.data_source));

                self.invalidate(None);
            }
            PlotData::TimeBased(ref mut timeseries) => {
                timeseries.insert_trades_existing_buckets(buffer);

                self.indicators
                    .values_mut()
                    .filter_map(Option::as_mut)
                    .for_each(|indi| indi.on_insert_trades(buffer, 0, &self.data_source));

                self.invalidate(None);
            }
        }
    }

    pub fn insert_raw_trades(&mut self, raw_trades: Vec<Trade>, is_batches_done: bool) {
        match self.data_source {
            PlotData::TickBased(ref mut tick_aggr) => {
                tick_aggr.insert_trades(&raw_trades);
            }
            PlotData::TimeBased(ref mut timeseries) => {
                timeseries.insert_trades_existing_buckets(&raw_trades);
            }
        }

        self.raw_trades.extend_from_slice(&raw_trades);

        self.indicators
            .values_mut()
            .filter_map(Option::as_mut)
            .for_each(|indi| indi.on_insert_trades(&raw_trades, 0, &self.data_source));

        if is_batches_done {
            self.fetching_trades = (false, None);
        }

        self.invalidate(None);
    }

    pub fn insert_hist_klines(&mut self, req_id: uuid::Uuid, klines_raw: &[Kline]) {
        match self.data_source {
            PlotData::TimeBased(ref mut timeseries) => {
                timeseries.insert_klines(klines_raw);
                timeseries.insert_trades_existing_buckets(&self.raw_trades);

                self.indicators
                    .values_mut()
                    .filter_map(Option::as_mut)
                    .for_each(|indi| indi.on_insert_klines(klines_raw, &self.data_source));

                if klines_raw.is_empty() {
                    self.request_handler
                        .mark_failed(req_id, "No data received".to_string());
                } else {
                    self.request_handler.mark_completed(req_id);
                }
                self.invalidate(None);
            }
            PlotData::TickBased(_) => {}
        }
    }

    pub fn insert_open_interest(&mut self, req_id: Option<uuid::Uuid>, oi_data: &[OIData]) {
        if let Some(req_id) = req_id {
            if oi_data.is_empty() {
                self.request_handler
                    .mark_failed(req_id, "No data received".to_string());
            } else {
                self.request_handler.mark_completed(req_id);
            }
        }

        if let Some(indi) = self.indicators[KlineIndicator::OpenInterest].as_mut() {
            indi.on_open_interest(oi_data);
        }
    }

    fn calc_qty_scales(
        &self,
        earliest: u64,
        latest: u64,
        highest: Price,
        lowest: Price,
        step: PriceStep,
        cluster_kind: ClusterKind,
    ) -> f32 {
        let rounded_highest = highest.round_to_side_step(false, step).add_steps(1, step);
        let rounded_lowest = lowest.round_to_side_step(true, step).add_steps(-1, step);

        match &self.data_source {
            PlotData::TimeBased(timeseries) => timeseries
                .max_qty_ts_range(
                    cluster_kind,
                    UnixMs::new(earliest),
                    UnixMs::new(latest),
                    rounded_highest,
                    rounded_lowest,
                )
                .into(),
            PlotData::TickBased(tick_aggr) => {
                let earliest = earliest as usize;
                let latest = latest as usize;

                tick_aggr
                    .max_qty_idx_range(
                        cluster_kind,
                        earliest,
                        latest,
                        rounded_highest,
                        rounded_lowest,
                    )
                    .into()
            }
        }
    }

    pub fn last_update(&self) -> Instant {
        self.last_tick
    }

    pub fn invalidate(&mut self, now: Option<Instant>) -> Option<Action> {
        let chart = &mut self.chart;

        if let Some(autoscale) = chart.layout.autoscale {
            match autoscale {
                super::Autoscale::CenterLatest => {
                    let x_translation = match &self.kind {
                        KlineChartKind::Footprint { .. } => {
                            0.5 * (chart.bounds.width / chart.scaling)
                                - (chart.cell_width / chart.scaling)
                        }
                        KlineChartKind::Candles => {
                            0.5 * (chart.bounds.width / chart.scaling)
                                - (8.0 * chart.cell_width / chart.scaling)
                        }
                    };
                    chart.translation.x = x_translation;

                    let calculate_target_y = |kline: exchange::Kline| -> f32 {
                        let y_low = chart.price_to_y(kline.low);
                        let y_high = chart.price_to_y(kline.high);
                        let y_close = chart.price_to_y(kline.close);

                        let mut target_y_translation = -(y_low + y_high) / 2.0;

                        if chart.bounds.height > f32::EPSILON && chart.scaling > f32::EPSILON {
                            let visible_half_height = (chart.bounds.height / chart.scaling) / 2.0;

                            let view_center_y_centered = -target_y_translation;

                            let visible_y_top = view_center_y_centered - visible_half_height;
                            let visible_y_bottom = view_center_y_centered + visible_half_height;

                            let padding = chart.cell_height;

                            if y_close < visible_y_top {
                                target_y_translation = -(y_close - padding + visible_half_height);
                            } else if y_close > visible_y_bottom {
                                target_y_translation = -(y_close + padding - visible_half_height);
                            }
                        }
                        target_y_translation
                    };

                    chart.translation.y = self.data_source.latest_y_midpoint(calculate_target_y);
                }
                super::Autoscale::FitToVisible => {
                    let visible_region = chart.visible_region(chart.bounds.size());
                    let (start_interval, end_interval) = chart.interval_range(&visible_region);

                    if let Some((lowest, highest)) = self
                        .data_source
                        .visible_price_range(start_interval, end_interval)
                    {
                        let chart_height = chart.bounds.height;
                        let tick_size = chart.tick_size.to_f32_lossy();

                        if chart_height > f32::EPSILON && tick_size > 0.0 {
                            let (fit_lowest, fit_highest) =
                                if let KlineChartKind::Footprint { .. } = self.kind {
                                    if let Some((footprint_low, footprint_high)) = self
                                        .data_source
                                        .visible_footprint_price_range(start_interval, end_interval)
                                    {
                                        let half_tick = tick_size * 0.5;
                                        (
                                            footprint_low.to_f32_lossy() - half_tick,
                                            footprint_high.to_f32_lossy() + half_tick,
                                        )
                                    } else {
                                        (lowest, highest)
                                    }
                                } else {
                                    (lowest, highest)
                                };

                            let visible_span = (fit_highest - fit_lowest).max(tick_size);
                            let base_padding = visible_span * 0.05; // 5% padding on top and bottom

                            let mut top_padding = base_padding;
                            let mut bottom_padding = base_padding;

                            if let KlineChartKind::Footprint { clusters, .. } = self.kind {
                                let provisional_span = visible_span + top_padding + bottom_padding;
                                if provisional_span > 0.0 {
                                    let provisional_cell_height =
                                        (chart_height * tick_size) / provisional_span;

                                    let outer_padding = price_padding_from_pixels(
                                        provisional_cell_height,
                                        tick_size,
                                    );

                                    top_padding += outer_padding;
                                    bottom_padding += outer_padding;

                                    bottom_padding = bottom_padding.max(footprint_summary_padding(
                                        provisional_cell_height,
                                        chart.scaling,
                                        chart.cell_width,
                                        tick_size,
                                        clusters,
                                    ));
                                }
                            }

                            let padded_span = visible_span + top_padding + bottom_padding;
                            if padded_span > 0.0 {
                                chart.cell_height = (chart_height * tick_size) / padded_span;
                                chart.base_price_y = Price::from_f32(fit_highest + top_padding);
                                chart.translation.y = -chart_height / 2.0;
                            }
                        }
                    }
                }
            }
        }

        chart.cache.clear_all();
        for indi in self.indicators.values_mut().filter_map(Option::as_mut) {
            indi.clear_all_caches();
        }

        if let Some(t) = now {
            self.last_tick = t;
            self.fetch_missing_data()
        } else {
            None
        }
    }

    pub fn toggle_indicator(&mut self, indicator: KlineIndicator) {
        let prev_indi_count = self
            .indicators
            .values()
            .filter(|v| v.as_ref().is_some_and(|i| !i.is_overlay()))
            .count();

        if self.indicators[indicator].is_some() {
            self.indicators[indicator] = None;
        } else {
            let mut box_indi = indicator::kline::make_empty(indicator);
            box_indi.rebuild_from_source(&self.data_source);
            self.indicators[indicator] = Some(box_indi);
        }

        if let Some(main_split) = self.chart.layout.splits.first() {
            let current_indi_count = self
                .indicators
                .values()
                .filter(|v| v.as_ref().is_some_and(|i| !i.is_overlay()))
                .count();
            self.chart.layout.splits = data::util::calc_panel_splits(
                *main_split,
                current_indi_count,
                Some(prev_indi_count),
            );
        }
    }
}

impl canvas::Program<Message> for KlineChart {
    type State = Interaction;

    fn update(
        &self,
        interaction: &mut Interaction,
        event: &Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<canvas::Action<Message>> {
        super::canvas_interaction(self, interaction, event, bounds, cursor)
    }

    fn draw(
        &self,
        interaction: &Interaction,
        renderer: &Renderer,
        theme: &Theme,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let chart = self.state();

        if chart.bounds.width == 0.0 {
            return vec![];
        }

        let bounds_size = bounds.size();
        let palette = theme.extended_palette();

        let klines = chart.cache.main.draw(renderer, bounds_size, |frame| {
            let center = Vector::new(bounds.width / 2.0, bounds.height / 2.0);

            frame.translate(center);
            frame.scale(chart.scaling);
            frame.translate(chart.translation);

            let region = chart.visible_region(frame.size());
            let (earliest, latest) = chart.interval_range(&region);

            let price_to_y = |price| chart.price_to_y(price);
            let interval_to_x = |interval| chart.interval_to_x(interval);

            match &self.kind {
                KlineChartKind::Footprint {
                    clusters,
                    scaling,
                    studies,
                } => {
                    let (highest, lowest) = chart.price_range(&region);

                    let max_cluster_qty = self.calc_qty_scales(
                        earliest,
                        latest,
                        highest,
                        lowest,
                        chart.tick_size,
                        *clusters,
                    );

                    let cell_height_unscaled = chart.cell_height * chart.scaling;
                    let cell_width_unscaled = chart.cell_width * chart.scaling;

                    let text_size =
                        footprint_cluster_text_size(cell_height_unscaled, cell_width_unscaled);

                    let candle_width = 0.1 * chart.cell_width;
                    let content_spacing = ContentGaps::from_view(candle_width, chart.scaling);

                    let imbalance = studies.iter().find_map(|study| {
                        if let FootprintStudy::Imbalance {
                            threshold,
                            color_scale,
                            ignore_zeros,
                        } = study
                        {
                            Some((*threshold, *color_scale, *ignore_zeros))
                        } else {
                            None
                        }
                    });

                    let show_text = should_show_text(
                        cell_height_unscaled,
                        cell_width_unscaled,
                        footprint_cluster_min_width(*clusters),
                    );

                    draw_all_npocs(
                        &self.data_source,
                        frame,
                        price_to_y,
                        interval_to_x,
                        candle_width,
                        chart.cell_width,
                        chart.cell_height,
                        palette,
                        studies,
                        earliest,
                        latest,
                        *clusters,
                        content_spacing,
                        imbalance.is_some(),
                    );

                    draw_stacked_imbalances(
                        &self.data_source,
                        frame,
                        price_to_y,
                        interval_to_x,
                        candle_width,
                        chart.cell_width,
                        chart.cell_height,
                        palette,
                        studies,
                        earliest,
                        latest,
                        *clusters,
                        content_spacing,
                        self.chart.tick_size,
                    );

                    render_data_source(
                        &self.data_source,
                        frame,
                        earliest,
                        latest,
                        interval_to_x,
                        |frame, x_position, kline, trades| {
                            let cluster_scaling =
                                effective_cluster_qty(*scaling, max_cluster_qty, trades, *clusters);

                            draw_clusters(
                                frame,
                                price_to_y,
                                x_position,
                                chart.cell_width,
                                chart.cell_height,
                                candle_width,
                                cluster_scaling,
                                palette,
                                text_size,
                                self.tick_size(),
                                show_text,
                                imbalance,
                                kline,
                                trades,
                                *clusters,
                                content_spacing,
                                self.visual_config.big_trade_filter,
                            );

                            draw_candle_stats(
                                frame,
                                price_to_y,
                                x_position,
                                kline,
                                trades,
                                text_size,
                                palette,
                                show_text,
                            );
                        },
                    );
                }
                KlineChartKind::Candles => {
                    let candle_width = (chart.cell_width * 0.8).max(2.0);

                    render_data_source(
                        &self.data_source,
                        frame,
                        earliest,
                        latest,
                        interval_to_x,
                        |frame, x_position, kline, footprint| {
                            draw_candle_dp(
                                frame,
                                &price_to_y,
                                candle_width,
                                palette,
                                x_position,
                                kline,
                                footprint,
                                self.visual_config.big_trade_filter,
                            );
                        },
                    );
                }
            }

            chart.draw_last_price_line(frame, palette, region);

            for indi in self.indicators.values().filter_map(Option::as_ref) {
                if indi.is_overlay() {
                    let range = earliest..=latest;
                    indi.draw_overlay(frame, chart, theme, range);
                }
            }

            if self.visual_config.show_session_profile {
                draw_session_profile(
                    &self.data_source,
                    frame,
                    chart,
                    &region,
                    palette,
                    chart.tick_size.to_f32_lossy(),
                );
            }

            for drawing in &self.drawings.drawings {
                let color = if drawing.selected {
                    iced::Color::from_rgb8(255, 255, 255) // White when selected
                } else {
                    iced::Color::from_rgb8(255, 255, 0) // Yellow default
                };
                let stroke = canvas::Stroke::default().with_color(color).with_width(2.0);

                let mut points = match &drawing.state {
                    data::chart::drawing::DrawingState::Initial => continue,
                    data::chart::drawing::DrawingState::OnePoint(p) => vec![*p],
                    data::chart::drawing::DrawingState::Completed(p1, p2) => vec![*p1, *p2],
                };

                if points.len() == 1 {
                    if let Some(cursor) = self.drawings.current_cursor {
                        points.push(cursor);
                    }
                }

                let mapped_points: Vec<Point> = points
                    .iter()
                    .map(|p| Point::new(interval_to_x(p.time), price_to_y(p.price)))
                    .collect();

                match drawing.kind {
                    data::chart::drawing::DrawingType::TrendLine => {
                        if mapped_points.len() == 2 {
                            let path = canvas::Path::new(|b| {
                                b.move_to(mapped_points[0]);
                                b.line_to(mapped_points[1]);
                            });
                            frame.stroke(&path, stroke);
                        } else if mapped_points.len() == 1 {
                            frame.fill(&canvas::Path::circle(mapped_points[0], 3.0), color);
                        }
                    }
                    data::chart::drawing::DrawingType::HorizontalLine => {
                        let y = mapped_points[0].y;
                        let path = canvas::Path::new(|b| {
                            b.move_to(Point::new(region.x, y));
                            b.line_to(Point::new(region.x + region.width, y));
                        });
                        frame.stroke(&path, stroke);
                    }
                    data::chart::drawing::DrawingType::Ray => {
                        if mapped_points.len() == 2 {
                            let dx = mapped_points[1].x - mapped_points[0].x;
                            let dy = mapped_points[1].y - mapped_points[0].y;
                            let distance = (dx * dx + dy * dy).sqrt();
                            if distance > 0.0 {
                                let extended_x = mapped_points[0].x + (dx / distance) * 10000.0;
                                let extended_y = mapped_points[0].y + (dy / distance) * 10000.0;
                                let path = canvas::Path::new(|b| {
                                    b.move_to(mapped_points[0]);
                                    b.line_to(Point::new(extended_x, extended_y));
                                });
                                frame.stroke(&path, stroke);
                            }
                        } else if mapped_points.len() == 1 {
                            frame.fill(&canvas::Path::circle(mapped_points[0], 3.0), color);
                        }
                    }
                    data::chart::drawing::DrawingType::Rectangle => {
                        if mapped_points.len() == 2 {
                            let x = mapped_points[0].x.min(mapped_points[1].x);
                            let y = mapped_points[0].y.min(mapped_points[1].y);
                            let w = (mapped_points[0].x - mapped_points[1].x).abs();
                            let h = (mapped_points[0].y - mapped_points[1].y).abs();
                            frame.stroke(
                                &canvas::Path::rectangle(Point::new(x, y), Size::new(w, h)),
                                stroke,
                            );
                        } else if mapped_points.len() == 1 {
                            frame.fill(&canvas::Path::circle(mapped_points[0], 3.0), color);
                        }
                    }
                    _ => {}
                }

                if drawing.selected {
                    for p in &mapped_points {
                        frame.fill(
                            &canvas::Path::circle(*p, 4.0),
                            iced::Color::from_rgb8(255, 255, 255),
                        );
                    }
                }
            }
        });

        let crosshair = chart.cache.crosshair.draw(renderer, bounds_size, |frame| {
            let visible_region = chart.visible_region(bounds_size);
            let visible_range = chart.interval_range(&visible_region);

            if let Some(cursor_position) = cursor.position_in(bounds) {
                let (_, rounded_aggregation) =
                    chart.draw_crosshair(frame, theme, bounds_size, cursor_position, interaction);

                draw_crosshair_tooltip(
                    &self.data_source,
                    &chart.ticker_info,
                    frame,
                    palette,
                    chart.basis,
                    Some(rounded_aggregation),
                    visible_range,
                );
            } else if self.visual_config.data_labels_always_visible {
                draw_crosshair_tooltip(
                    &self.data_source,
                    &chart.ticker_info,
                    frame,
                    palette,
                    chart.basis,
                    None,
                    visible_range,
                );
            }
        });

        vec![klines, crosshair]
    }

    fn mouse_interaction(
        &self,
        interaction: &Interaction,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        match interaction {
            Interaction::Panning { .. } => mouse::Interaction::Grabbing,
            Interaction::Zoomin { .. } => mouse::Interaction::ZoomIn,
            Interaction::None | Interaction::Ruler { .. } => {
                if cursor.is_over(bounds) {
                    mouse::Interaction::Crosshair
                } else {
                    mouse::Interaction::default()
                }
            }
        }
    }
}

fn draw_footprint_kline(
    frame: &mut canvas::Frame,
    price_to_y: impl Fn(Price) -> f32,
    x_position: f32,
    candle_width: f32,
    kline: &Kline,
    palette: &Extended,
) {
    let y_open = price_to_y(kline.open);
    let y_high = price_to_y(kline.high);
    let y_low = price_to_y(kline.low);
    let y_close = price_to_y(kline.close);

    let up_color = palette.success.weak.color;
    let down_color = palette.danger.weak.color;

    let body_color = if kline.close >= kline.open {
        up_color
    } else {
        down_color
    };
    
    // Thicker, more prominent body
    let body_w = candle_width * 0.8;
    frame.fill_rectangle(
        Point::new(x_position - (body_w / 2.0), y_open.min(y_close)),
        Size::new(body_w, (y_open - y_close).abs().max(1.0)),
        body_color,
    );

    let wick_color = if kline.close >= kline.open {
        up_color
    } else {
        down_color
    };
    let marker_line = Stroke::with_color(
        Stroke {
            width: 1.5,
            ..Default::default()
        },
        wick_color.scale_alpha(0.85),
    );
    frame.stroke(
        &Path::line(
            Point::new(x_position, y_high),
            Point::new(x_position, y_low),
        ),
        marker_line,
    );
}

fn draw_candle_dp(
    frame: &mut canvas::Frame,
    price_to_y: impl Fn(Price) -> f32,
    candle_width: f32,
    palette: &Extended,
    x_position: f32,
    kline: &Kline,
    footprint: &KlineTrades,
    big_trade_filter: Option<f32>,
) {
    let y_open = price_to_y(kline.open);
    let y_high = price_to_y(kline.high);
    let y_low = price_to_y(kline.low);
    let y_close = price_to_y(kline.close);

    let up_color = palette.success.base.color;
    let down_color = palette.danger.base.color;

    let is_up = kline.close >= kline.open;
    let body_color = if is_up { up_color } else { down_color };

    // Body
    frame.fill_rectangle(
        Point::new(x_position - (candle_width / 2.0), y_open.min(y_close)),
        Size::new(candle_width, (y_open - y_close).abs().max(1.0)),
        body_color,
    );

    // Wick
    let marker_line = Stroke::with_color(
        Stroke {
            width: 1.5,
            ..Default::default()
        },
        body_color.scale_alpha(0.85),
    );
    frame.stroke(
        &Path::line(
            Point::new(x_position, y_high),
            Point::new(x_position, y_low),
        ),
        marker_line,
    );

    // Only draw Delta if candle is wide enough (prevent clutter when zoomed out)
    if candle_width > 6.0 {
        let (total_buy, total_sell) = footprint.trades.values().fold((0i64, 0i64), |acc, tg| (acc.0 + f32::from(tg.buy_qty) as i64, acc.1 + f32::from(tg.sell_qty) as i64));
        let delta = total_buy - total_sell;
        let delta_text = format!("{}", delta);

        let font_size = 11.0_f32.min(candle_width);
        
        // Approximate text bounds
        let text_width = (delta_text.len() as f32) * (font_size * 0.6);
        let text_height = font_size + 4.0;
        
        let label_y = if is_up { y_high - text_height - 2.0 } else { y_low + 2.0 };
        
        let bg_rect = Path::rectangle(
            Point::new(x_position - (text_width / 2.0) - 2.0, label_y),
            Size::new(text_width + 4.0, text_height),
        );
        frame.fill(&bg_rect, body_color);

        frame.fill_text(canvas::Text {
            content: delta_text,
            position: Point::new(x_position, label_y + (text_height / 2.0)),
            color: iced::Color::WHITE,
            size: font_size.into(),
            align_x: iced::alignment::Horizontal::Center.into(),
            align_y: iced::alignment::Vertical::Center.into(),
            ..Default::default()
        });
    }

    if let Some(filter) = big_trade_filter {
        for (price, group) in &footprint.trades {
            let buy_qty = f32::from(group.buy_qty);
            let sell_qty = f32::from(group.sell_qty);
            let y = price_to_y(*price);
            if buy_qty >= filter && buy_qty > 0.0 {
                draw_big_trade_bubble(frame, x_position + 15.0, y, buy_qty, true, palette);
            }
            if sell_qty >= filter && sell_qty > 0.0 {
                draw_big_trade_bubble(frame, x_position - 15.0, y, sell_qty, false, palette);
            }
        }
    }
}

fn render_data_source<F>(
    data_source: &PlotData<KlineDataPoint>,
    frame: &mut canvas::Frame,
    earliest: u64,
    latest: u64,
    interval_to_x: impl Fn(u64) -> f32,
    draw_fn: F,
) where
    F: Fn(&mut canvas::Frame, f32, &Kline, &KlineTrades),
{
    match data_source {
        PlotData::TickBased(tick_aggr) => {
            let earliest = earliest as usize;
            let latest = latest as usize;

            tick_aggr
                .datapoints
                .iter()
                .rev()
                .enumerate()
                .filter(|(index, _)| *index <= latest && *index >= earliest)
                .for_each(|(index, tick_aggr)| {
                    let x_position = interval_to_x(index as u64);

                    draw_fn(frame, x_position, &tick_aggr.kline, &tick_aggr.footprint);
                });
        }
        PlotData::TimeBased(timeseries) => {
            if latest < earliest {
                return;
            }

            timeseries
                .datapoints
                .range(UnixMs::new(earliest)..=UnixMs::new(latest))
                .for_each(|(timestamp, dp)| {
                    let x_position = interval_to_x(timestamp.as_u64());

                    draw_fn(frame, x_position, &dp.kline, &dp.footprint);
                });
        }
    }
}

fn draw_all_npocs(
    data_source: &PlotData<KlineDataPoint>,
    frame: &mut canvas::Frame,
    price_to_y: impl Fn(Price) -> f32,
    interval_to_x: impl Fn(u64) -> f32,
    candle_width: f32,
    cell_width: f32,
    cell_height: f32,
    palette: &Extended,
    studies: &[FootprintStudy],
    visible_earliest: u64,
    visible_latest: u64,
    cluster_kind: ClusterKind,
    spacing: ContentGaps,
    imb_study_on: bool,
) {
    let Some(lookback) = studies.iter().find_map(|study| {
        if let FootprintStudy::NPoC { lookback } = study {
            Some(*lookback)
        } else {
            None
        }
    }) else {
        return;
    };

    let (filled_color, naked_color) = (
        palette.background.strong.color,
        if palette.is_dark {
            palette.warning.weak.color.scale_alpha(0.5)
        } else {
            palette.warning.strong.color
        },
    );

    let line_height = cell_height.min(1.0);

    let bar_width_factor: f32 = 0.9;
    let inset = (cell_width * (1.0 - bar_width_factor)) / 2.0;

    let candle_lane_factor: f32 = match cluster_kind {
        ClusterKind::VolumeProfile | ClusterKind::DeltaProfile => 0.25,
        ClusterKind::ProDeltaCandle => 0.15,
        ClusterKind::BidAsk => 1.0,
    };

    let start_x_for = |cell_center_x: f32| -> f32 {
        match cluster_kind {
            ClusterKind::BidAsk => cell_center_x + (candle_width / 2.0) + spacing.candle_to_cluster,
            ClusterKind::VolumeProfile | ClusterKind::DeltaProfile | ClusterKind::ProDeltaCandle => {
                let content_left = (cell_center_x - (cell_width / 2.0)) + inset;
                let candle_lane_left = content_left
                    + if imb_study_on {
                        candle_width + spacing.marker_to_candle
                    } else {
                        0.0
                    };
                candle_lane_left + candle_width * candle_lane_factor + spacing.candle_to_cluster
            }
        }
    };

    let wick_x_for = |cell_center_x: f32| -> f32 {
        match cluster_kind {
            ClusterKind::BidAsk => cell_center_x, // not used for BidAsk clustering
            ClusterKind::VolumeProfile | ClusterKind::DeltaProfile | ClusterKind::ProDeltaCandle => {
                let content_left = (cell_center_x - (cell_width / 2.0)) + inset;
                let candle_lane_left = content_left
                    + if imb_study_on {
                        candle_width + spacing.marker_to_candle
                    } else {
                        0.0
                    };
                candle_lane_left + (candle_width * candle_lane_factor) / 2.0
                    - (spacing.candle_to_cluster * 0.5)
            }
        }
    };

    let end_x_for = |cell_center_x: f32| -> f32 {
        match cluster_kind {
            ClusterKind::BidAsk => cell_center_x - (candle_width / 2.0) - spacing.candle_to_cluster,
            ClusterKind::VolumeProfile | ClusterKind::DeltaProfile | ClusterKind::ProDeltaCandle => wick_x_for(cell_center_x),
        }
    };

    let rightmost_cell_center_x = {
        let earliest_x = interval_to_x(visible_earliest);
        let latest_x = interval_to_x(visible_latest);
        if earliest_x > latest_x {
            earliest_x
        } else {
            latest_x
        }
    };

    let mut draw_the_line = |interval: u64, poc: &PointOfControl| {
        let start_x = start_x_for(interval_to_x(interval));

        let (line_width, color) = match poc.status {
            NPoc::Naked => {
                let end_x = end_x_for(rightmost_cell_center_x);
                let line_width = end_x - start_x;
                if line_width.abs() <= cell_width {
                    return;
                }
                (line_width, naked_color)
            }
            NPoc::Filled { at } => {
                let end_x = end_x_for(interval_to_x(at));
                let line_width = end_x - start_x;
                if line_width.abs() <= cell_width {
                    return;
                }
                (line_width, filled_color)
            }
            _ => return,
        };

        frame.fill_rectangle(
            Point::new(start_x, price_to_y(poc.price) - line_height / 2.0),
            Size::new(line_width, line_height),
            color,
        );
    };

    match data_source {
        PlotData::TickBased(tick_aggr) => {
            tick_aggr
                .datapoints
                .iter()
                .rev()
                .enumerate()
                .take(lookback)
                .filter_map(|(index, dp)| dp.footprint.poc.as_ref().map(|poc| (index as u64, poc)))
                .for_each(|(interval, poc)| draw_the_line(interval, poc));
        }
        PlotData::TimeBased(timeseries) => {
            timeseries
                .datapoints
                .iter()
                .rev()
                .take(lookback)
                .filter_map(|(timestamp, dp)| {
                    dp.footprint
                        .poc
                        .as_ref()
                        .map(|poc| (timestamp.as_u64(), poc))
                })
                .for_each(|(interval, poc)| draw_the_line(interval, poc));
        }
    }
}

fn draw_stacked_imbalances(
    data_source: &PlotData<KlineDataPoint>,
    frame: &mut canvas::Frame,
    price_to_y: impl Fn(Price) -> f32,
    interval_to_x: impl Fn(u64) -> f32,
    candle_width: f32,
    cell_width: f32,
    cell_height: f32,
    palette: &Extended,
    studies: &[FootprintStudy],
    visible_earliest: u64,
    visible_latest: u64,
    cluster_kind: ClusterKind,
    spacing: ContentGaps,
    step: PriceStep,
) {
    let Some(study) = studies.iter().find_map(|s| {
        if let FootprintStudy::StackedImbalance { consecutive, threshold } = s {
            Some((*consecutive, *threshold))
        } else {
            None
        }
    }) else {
        return;
    };
    
    let (consecutive, threshold) = study;
    let lookback = 150; // Scan up to 150 datapoints back

    let (buy_color, sell_color) = (
        palette.success.base.color,
        palette.danger.base.color,
    );

    let bar_width_factor: f32 = 0.9;
    let inset = (cell_width * (1.0 - bar_width_factor)) / 2.0;

    let start_x_for = |cell_center_x: f32| -> f32 {
        match cluster_kind {
            ClusterKind::BidAsk => cell_center_x + (candle_width / 2.0) + spacing.candle_to_cluster,
            ClusterKind::VolumeProfile | ClusterKind::DeltaProfile | ClusterKind::ProDeltaCandle => {
                let content_left = (cell_center_x - (cell_width / 2.0)) + inset;
                let candle_lane_left = content_left + candle_width + spacing.marker_to_candle;
                candle_lane_left + (candle_width * 0.25) + spacing.candle_to_cluster
            }
        }
    };

    #[derive(Clone)]
    struct ActiveZone {
        interval: u64,
        is_buy: bool,
        min_price: Price,
        max_price: Price,
        mitigated_at: Option<u64>,
    }

    let mut active_zones: Vec<ActiveZone> = Vec::new();

    let extract_zones = |footprint: &KlineTrades| -> Vec<(bool, Price, Price)> {
        let mut zones = Vec::new();
        let mut prices: Vec<Price> = footprint.trades.keys().copied().collect();
        prices.sort_by(|a, b| a.partial_cmp(b).unwrap());

        let mut buy_imbalances = std::collections::BTreeSet::new();
        let mut sell_imbalances = std::collections::BTreeSet::new();

        for &price in &prices {
            let group = footprint.trades.get(&price).unwrap();
            let sell_qty = f32::from(group.sell_qty);
            let higher_price = Price::from_f32(price.to_f32() + step.to_f32_lossy()).round_to_step(step);

            if let Some(higher_group) = footprint.trades.get(&higher_price) {
                let diagonal_buy_qty = f32::from(higher_group.buy_qty);
                
                if diagonal_buy_qty >= sell_qty {
                    let required_qty = sell_qty * (100 + threshold) as f32 / 100.0;
                    if diagonal_buy_qty > required_qty {
                        buy_imbalances.insert(higher_price);
                    }
                } else {
                    let required_qty = diagonal_buy_qty * (100 + threshold) as f32 / 100.0;
                    if sell_qty > required_qty {
                        sell_imbalances.insert(price);
                    }
                }
            }
        }

        let mut extract = |imbalances: &std::collections::BTreeSet<Price>, is_buy: bool| {
            if imbalances.is_empty() { return; }
            let imb_vec: Vec<Price> = imbalances.iter().copied().collect();
            let mut count = 1;
            let mut start_idx = 0;
            
            for i in 1..imb_vec.len() {
                let expected_price = Price::from_f32(imb_vec[i-1].to_f32() + step.to_f32_lossy()).round_to_step(step);
                if imb_vec[i] == expected_price {
                    count += 1;
                } else {
                    if count >= consecutive {
                        zones.push((is_buy, imb_vec[start_idx], imb_vec[i-1]));
                    }
                    count = 1;
                    start_idx = i;
                }
            }
            if count >= consecutive {
                zones.push((is_buy, imb_vec[start_idx], imb_vec[imb_vec.len()-1]));
            }
        };

        extract(&buy_imbalances, true);
        extract(&sell_imbalances, false);

        if zones.len() > 1 {
            let mut best_zone: Option<(bool, Price, Price)> = None;
            let mut best_score = 0.0;

            for &(is_buy, min_price, max_price) in &zones {
                let levels = ((max_price.to_f32() - min_price.to_f32()) / step.to_f32_lossy()).round() as i32 + 1;
                let mut total_vol = 0.0;
                let mut p = min_price;
                while p <= max_price {
                    if let Some(group) = footprint.trades.get(&p) {
                        total_vol += f32::from(group.buy_qty) + f32::from(group.sell_qty);
                    }
                    p = Price::from_f32(p.to_f32() + step.to_f32_lossy()).round_to_step(step);
                }
                // Score = number of levels + volume as tiebreaker
                let score = (levels as f32) * 1_000_000.0 + total_vol;
                if score > best_score {
                    best_score = score;
                    best_zone = Some((is_buy, min_price, max_price));
                }
            }
            
            if let Some(best) = best_zone {
                return vec![best];
            }
        }

        zones
    };

    let mut process_datapoint = |interval: u64, kline: &exchange::Kline, footprint: &KlineTrades| {
        let high = kline.high;
        let low = kline.low;
        
        for zone in &mut active_zones {
            if zone.mitigated_at.is_some() { continue; }
            if zone.is_buy && low <= zone.max_price {
                zone.mitigated_at = Some(interval);
            } else if !zone.is_buy && high >= zone.min_price {
                zone.mitigated_at = Some(interval);
            }
        }
        
        for (is_buy, min_price, max_price) in extract_zones(footprint) {
            active_zones.push(ActiveZone {
                interval,
                is_buy,
                min_price,
                max_price,
                mitigated_at: None,
            });
        }
    };

    let mut latest_interval = None;

    match data_source {
        PlotData::TickBased(tick_aggr) => {
            let start_idx = tick_aggr.datapoints.len().saturating_sub(lookback);
            for (i, dp) in tick_aggr.datapoints[start_idx..].iter().enumerate() {
                let interval = (start_idx + i) as u64;
                latest_interval = Some(interval);
                process_datapoint(interval, &dp.kline, &dp.footprint);
            }
        }
        PlotData::TimeBased(timeseries) => {
            let mut iter = timeseries.datapoints.iter().rev().take(lookback).collect::<Vec<_>>();
            iter.reverse();
            for (timestamp, dp) in iter {
                let interval = timestamp.as_u64();
                latest_interval = Some(interval);
                process_datapoint(interval, &dp.kline, &dp.footprint);
            }
        }
    }

    let right_bound = latest_interval.unwrap_or(visible_latest); // End at current candle
    
    // Find the strongest unmitigated buy and sell zones
    let mut best_buy = None;
    let mut best_buy_score = 0.0;
    let mut best_sell = None;
    let mut best_sell_score = 0.0;

    for zone in &active_zones {
        if zone.mitigated_at.is_none() {
            let levels = ((zone.max_price.to_f32() - zone.min_price.to_f32()) / step.to_f32_lossy()).round() as f32 + 1.0;
            // Optionally could use duration (how long it has survived) as a tiebreaker
            let survival = (right_bound.saturating_sub(zone.interval)) as f32 * 0.0001;
            let score = levels + survival;
            
            if zone.is_buy {
                if score > best_buy_score {
                    best_buy_score = score;
                    best_buy = Some(zone.clone());
                }
            } else {
                if score > best_sell_score {
                    best_sell_score = score;
                    best_sell = Some(zone.clone());
                }
            }
        }
    }

    let mut top_zones = Vec::new();
    if let Some(buy) = best_buy { top_zones.push(buy); }
    if let Some(sell) = best_sell { top_zones.push(sell); }

    for zone in top_zones {
        // Skip rendering if the candle is the current active (unclosed) candle
        if Some(zone.interval) == latest_interval {
            continue;
        }

        // Skip rendering if mitigated before visible area
        if let Some(mitigated_at) = zone.mitigated_at {
            if mitigated_at < visible_earliest {
                continue;
            }
        }
        if zone.interval > visible_latest {
            continue;
        }

        let start_x = interval_to_x(zone.interval);
        let end_interval = zone.mitigated_at.unwrap_or(right_bound);
        let end_x = interval_to_x(end_interval);
        
        let center_x = start_x_for(start_x);
        
        // Stacked Imbalance zone bounds
        let y_top = price_to_y(zone.max_price) - (cell_height / 2.0);
        let y_bottom = price_to_y(zone.min_price) + (cell_height / 2.0);
        let height = (y_bottom - y_top).max(1.0);
        
        let rect_width = (end_x - center_x).max(1.0);
        if rect_width <= 0.0 {
            continue;
        }

        let base_color = if zone.is_buy { buy_color } else { sell_color };
        let fill_color = base_color.scale_alpha(0.12); // Subtle background like ATAS
        let line_color = base_color.scale_alpha(0.6); // Thin boundary lines

        // Fill inner glow
        frame.fill_rectangle(Point::new(center_x, y_top), Size::new(rect_width, height), fill_color);

        // Top line
        frame.stroke(
            &canvas::Path::line(Point::new(center_x, y_top), Point::new(center_x + rect_width, y_top)),
            canvas::Stroke::default().with_color(line_color).with_width(1.0),
        );
        // Bottom line
        frame.stroke(
            &canvas::Path::line(Point::new(center_x, y_bottom), Point::new(center_x + rect_width, y_bottom)),
            canvas::Stroke::default().with_color(line_color).with_width(1.0),
        );
    }
}

fn effective_cluster_qty(
    scaling: ClusterScaling,
    visible_max: f32,
    footprint: &KlineTrades,
    cluster_kind: ClusterKind,
) -> f32 {
    let individual_max = match cluster_kind {
        ClusterKind::BidAsk => footprint
            .trades
            .values()
            .map(|group| group.buy_qty.max(group.sell_qty))
            .max()
            .unwrap_or_default(),
        ClusterKind::DeltaProfile => footprint
            .trades
            .values()
            .map(|group| group.buy_qty.abs_diff(group.sell_qty))
            .max()
            .unwrap_or_default(),
        ClusterKind::VolumeProfile | ClusterKind::ProDeltaCandle => footprint
            .trades
            .values()
            .map(|group| group.buy_qty + group.sell_qty)
            .max()
            .unwrap_or_default(),
    };
    let individual_max_f32 = f32::from(individual_max);

    match scaling {
        ClusterScaling::VisibleRange => Qty::scale_or_one(visible_max),
        ClusterScaling::Datapoint => individual_max.to_scale_or_one(),
        ClusterScaling::Hybrid { weight } => {
            let w = weight.clamp(0.0, 1.0);
            Qty::scale_or_one(visible_max * w + individual_max_f32 * (1.0 - w))
        }
    }
}

fn draw_clusters(
    frame: &mut canvas::Frame,
    price_to_y: impl Fn(Price) -> f32,
    x_position: f32,
    cell_width: f32,
    cell_height: f32,
    candle_width: f32,
    max_cluster_qty: f32,
    palette: &Extended,
    text_size: f32,
    step: PriceStep,
    show_text: bool,
    imbalance: Option<(usize, Option<usize>, bool)>,
    kline: &Kline,
    footprint: &KlineTrades,
    cluster_kind: ClusterKind,
    spacing: ContentGaps,
    big_trade_filter: Option<f32>,
) {
    let text_color = palette.background.weakest.text;

    let bar_width_factor: f32 = 0.9;
    let inset = (cell_width * (1.0 - bar_width_factor)) / 2.0;

    let cell_left = x_position - (cell_width / 2.0);
    let content_left = cell_left + inset;
    let content_right = x_position + (cell_width / 2.0) - inset;

    match cluster_kind {
        ClusterKind::VolumeProfile | ClusterKind::DeltaProfile => {
            let area = ProfileArea::new(
                content_left,
                content_right,
                candle_width,
                spacing,
                imbalance.is_some(),
            );
            let bar_alpha = if show_text { 0.25 } else { 1.0 };

            for (price, group) in &footprint.trades {
                let buy_qty = f32::from(group.buy_qty);
                let sell_qty = f32::from(group.sell_qty);
                let y = price_to_y(*price);

                match cluster_kind {
                    ClusterKind::VolumeProfile => {
                        super::draw_volume_bar(
                            frame,
                            area.bars_left,
                            y,
                            buy_qty,
                            sell_qty,
                            max_cluster_qty,
                            area.bars_width,
                            cell_height,
                            palette.success.base.color,
                            palette.danger.base.color,
                            bar_alpha,
                            true,
                        );

                        if show_text {
                            draw_cluster_text(
                                frame,
                                &abbr_large_numbers(f32::from(group.total_qty())),
                                Point::new(area.bars_left, y),
                                text_size,
                                text_color,
                                Alignment::Start,
                                Alignment::Center,
                            );
                        }
                    }
                    ClusterKind::DeltaProfile => {
                        let delta = f32::from(group.delta_qty());
                        if show_text {
                            draw_cluster_text(
                                frame,
                                &abbr_large_numbers(delta),
                                Point::new(area.bars_left, y),
                                text_size,
                                text_color,
                                Alignment::Start,
                                Alignment::Center,
                            );
                        }

                        let bar_width = (delta.abs() / max_cluster_qty) * area.bars_width;
                        if bar_width > 0.0 {
                            let color = if delta >= 0.0 {
                                palette.success.base.color.scale_alpha(bar_alpha)
                            } else {
                                palette.danger.base.color.scale_alpha(bar_alpha)
                            };
                            frame.fill_rectangle(
                                Point::new(area.bars_left, y - (cell_height / 2.0)),
                                Size::new(bar_width, cell_height),
                                color,
                            );
                        }
                    }
                    _ => {}
                }

                if let Some(filter) = big_trade_filter {
                    if buy_qty >= filter && buy_qty > 0.0 {
                        draw_big_trade_bubble(frame, area.candle_center_x + 15.0, y, buy_qty, true, palette);
                    }
                    if sell_qty >= filter && sell_qty > 0.0 {
                        draw_big_trade_bubble(frame, area.candle_center_x - 15.0, y, sell_qty, false, palette);
                    }
                }

                if let Some((threshold, color_scale, ignore_zeros)) = imbalance {
                    let higher_price =
                        Price::from_f32(price.to_f32() + step.to_f32_lossy()).round_to_step(step);

                    let start_x_buy = area.bars_left;
                    let width_buy = area.bars_width;
                    let start_x_sell = area.bars_left;
                    let width_sell = area.bars_width;

                    draw_imbalance_markers(
                        frame,
                        &price_to_y,
                        footprint,
                        *price,
                        sell_qty,
                        higher_price,
                        threshold,
                        color_scale,
                        ignore_zeros,
                        cell_height,
                        palette,
                        start_x_buy,
                        width_buy,
                        start_x_sell,
                        width_sell,
                    );
                }
            }

            draw_footprint_kline(
                frame,
                &price_to_y,
                area.candle_center_x,
                candle_width,
                kline,
                palette,
            );
        }
        ClusterKind::ProDeltaCandle => {
            let candle_lane_left = if imbalance.is_some() {
                content_left + candle_width + spacing.marker_to_candle
            } else {
                content_left
            };
            let candle_lane_width = candle_width * 0.30;
            let candle_center_x = candle_lane_left + (candle_lane_width / 2.0);

            let bars_left = candle_lane_left + candle_lane_width + spacing.candle_to_cluster;
            let total_bars_width = (content_right - bars_left).max(0.0);
            let section_width = ((total_bars_width - spacing.candle_to_cluster) / 2.0).max(0.0);
            
            let delta_left = bars_left;
            let delta_center_x = delta_left + (section_width / 2.0);
            let volume_left = delta_left + section_width + spacing.candle_to_cluster;

            // 1. Draw the candlestick
            draw_footprint_kline(
                frame,
                &price_to_y,
                candle_center_x,
                candle_lane_width,
                kline,
                palette,
            );
            
            // max delta calculation for scaling the delta bars
            let max_delta_qty = footprint.trades.values().map(|group| group.buy_qty.abs_diff(group.sell_qty)).max().unwrap_or_default();
            let max_delta_f32 = f32::from(max_delta_qty);
            let total_footprint_vol: f32 = footprint.trades.values().map(|g| f32::from(g.total_qty())).sum();
            
            // Calculate Value Area (VAH / VAL)
            let mut val = footprint.poc_price().unwrap_or(Price::from_f32(0.0));
            let mut vah = val;
            let target_va_vol = total_footprint_vol * 0.70;
            
            if let Some(poc) = footprint.poc_price() {
                let mut prices: Vec<Price> = footprint.trades.keys().copied().collect();
                prices.sort();
                
                if let Ok(poc_idx) = prices.binary_search(&poc) {
                    let mut current_vol = f32::from(footprint.trades.get(&poc).unwrap().total_qty());
                    let mut upper_idx = poc_idx;
                    let mut lower_idx = poc_idx;
                    
                    while current_vol < target_va_vol && (upper_idx < prices.len() - 1 || lower_idx > 0) {
                        let vol_up = if upper_idx < prices.len() - 1 {
                            f32::from(footprint.trades.get(&prices[upper_idx + 1]).unwrap().total_qty())
                        } else {
                            -1.0
                        };
                        let vol_down = if lower_idx > 0 {
                            f32::from(footprint.trades.get(&prices[lower_idx - 1]).unwrap().total_qty())
                        } else {
                            -1.0
                        };
                        
                        if vol_up > vol_down {
                            upper_idx += 1;
                            vah = prices[upper_idx];
                            current_vol += vol_up;
                        } else if vol_down >= vol_up && vol_down >= 0.0 {
                            lower_idx -= 1;
                            val = prices[lower_idx];
                            current_vol += vol_down;
                        } else {
                            break;
                        }
                    }
                }
            }

            // Draw a dashed zero-line for delta profile
            if let (Some(first), Some(last)) = (footprint.trades.keys().next(), footprint.trades.keys().last()) {
                let y1 = price_to_y(*first) - (cell_height / 2.0);
                let y2 = price_to_y(*last) + (cell_height / 2.0);
                let mut zero_line_path = canvas::path::Builder::new();
                zero_line_path.move_to(Point::new(delta_center_x, y1));
                zero_line_path.line_to(Point::new(delta_center_x, y2));
                
                frame.stroke(
                    &zero_line_path.build(),
                    Stroke {
                        width: 1.0,
                        style: canvas::stroke::Style::Solid(palette.background.weakest.text.scale_alpha(0.5)),
                        line_dash: canvas::LineDash { segments: &[4.0, 4.0], offset: 0 },
                        ..Default::default()
                    }
                );
            }
            
            for (price, group) in &footprint.trades {
                let total_qty = f32::from(group.buy_qty) + f32::from(group.sell_qty);
                let delta = f32::from(group.delta_qty());
                let y = price_to_y(*price);
                
                let padding = if cell_height > 4.0 { 1.0 } else { 0.0 };
                let bar_height = (cell_height - padding * 2.0).max(1.0);
                let bar_y = y - (cell_height / 2.0) + padding;

                // --- Draw Delta Profile ---
                let delta_bar_width = if max_delta_f32 > 0.0 { (delta.abs() / max_delta_f32) * (section_width / 2.0) } else { 0.0 };
                if delta_bar_width > 0.0 {
                    let (color, x_start) = if delta >= 0.0 {
                        (palette.success.base.color, delta_center_x)
                    } else {
                        (palette.danger.base.color, delta_center_x - delta_bar_width)
                    };
                    
                    frame.stroke(
                        &Path::rectangle(
                            Point::new(x_start, bar_y),
                            Size::new(delta_bar_width, bar_height)
                        ),
                        Stroke::with_color(Stroke { width: 1.0, ..Default::default() }, color)
                    );
                }

                // --- Draw Volume Profile ---
                let vol_bar_width = if max_cluster_qty > 0.0 { (total_qty / max_cluster_qty) * section_width } else { 0.0 };
                if vol_bar_width > 0.0 {
                    let in_va = *price >= val && *price <= vah;
                    
                    let fill_color = if in_va {
                        iced::Color::from_rgb8(30, 80, 160) // dark blue for Value Area
                    } else {
                        iced::Color::from_rgb8(100, 180, 200) // light cyan for outside
                    };
                    let border_color = if in_va {
                        iced::Color::from_rgb8(60, 160, 255) // bright blue border for VA
                    } else {
                        iced::Color::from_rgb8(50, 130, 160) // darker border for outside
                    };
                    
                    let rect = Path::rectangle(
                        Point::new(volume_left, bar_y),
                        Size::new(vol_bar_width, bar_height)
                    );
                    
                    frame.fill(&rect, fill_color);
                    frame.stroke(&rect, Stroke::with_color(Stroke { width: 1.0, ..Default::default() }, border_color));
                }
            }

            // Draw VAL / VAH Dashed Lines
            let line_stroke = Stroke {
                width: 1.0,
                style: canvas::stroke::Style::Solid(iced::Color::from_rgb8(100, 180, 200).scale_alpha(0.8)),
                line_dash: canvas::LineDash { segments: &[4.0, 4.0], offset: 0 },
                ..Default::default()
            };
            
            if val > Price::from_f32(0.0) {
                let y_val = price_to_y(val) + (cell_height / 2.0);
                frame.stroke(
                    &Path::line(Point::new(candle_lane_left, y_val), Point::new(content_right, y_val)),
                    line_stroke.clone()
                );
            }
            if vah > Price::from_f32(0.0) {
                let y_vah = price_to_y(vah) - (cell_height / 2.0);
                frame.stroke(
                    &Path::line(Point::new(candle_lane_left, y_vah), Point::new(content_right, y_vah)),
                    line_stroke
                );
            }
        }
        ClusterKind::BidAsk => {
            let area = BidAskArea::new(
                x_position,
                content_left,
                content_right,
                candle_width,
                spacing,
            );

            let bar_alpha = if show_text { 0.25 } else { 1.0 };

            let imb_marker_reserve = 0.0;

            let right_max_x =
                area.bid_area_right - imb_marker_reserve - (2.0 * spacing.marker_to_bars);
            let right_area_width = (right_max_x - area.bid_area_left).max(0.0);

            let left_min_x =
                area.ask_area_left + imb_marker_reserve + (2.0 * spacing.marker_to_bars);
            let left_area_width = (area.ask_area_right - left_min_x).max(0.0);

            for (price, group) in &footprint.trades {
                let buy_qty = f32::from(group.buy_qty);
                let sell_qty = f32::from(group.sell_qty);
                let y = price_to_y(*price);

                if buy_qty > 0.0 && right_area_width > 0.0 {
                    let ratio = (buy_qty / max_cluster_qty).min(1.0);
                    let alpha = 0.1 + (ratio * 0.7);

                    frame.fill_rectangle(
                        Point::new(area.bid_area_left, y - (cell_height / 2.0)),
                        Size::new(right_area_width, cell_height),
                        palette.success.base.color.scale_alpha(alpha * bar_alpha),
                    );

                    if show_text {
                        let dynamic_text_color = if alpha > 0.5 { palette.background.base.color } else { text_color };
                        draw_cluster_text(
                            frame,
                            &abbr_large_numbers(buy_qty),
                            Point::new(area.bid_area_left + 2.0, y),
                            text_size,
                            dynamic_text_color,
                            Alignment::Start,
                            Alignment::Center,
                        );
                    }
                }
                if sell_qty > 0.0 && left_area_width > 0.0 {
                    let ratio = (sell_qty / max_cluster_qty).min(1.0);
                    let alpha = 0.1 + (ratio * 0.7);

                    frame.fill_rectangle(
                        Point::new(area.ask_area_right, y - (cell_height / 2.0)),
                        Size::new(-left_area_width, cell_height),
                        palette.danger.base.color.scale_alpha(alpha * bar_alpha),
                    );

                    if show_text {
                        let dynamic_text_color = if alpha > 0.5 { palette.background.base.color } else { text_color };
                        draw_cluster_text(
                            frame,
                            &abbr_large_numbers(sell_qty),
                            Point::new(area.ask_area_right - 2.0, y),
                            text_size,
                            dynamic_text_color,
                            Alignment::End,
                            Alignment::Center,
                        );
                    }
                }

                if let Some(filter) = big_trade_filter {
                    if buy_qty >= filter && buy_qty > 0.0 {
                        draw_big_trade_bubble(frame, area.candle_center_x + 15.0, y, buy_qty, true, palette);
                    }
                    if sell_qty >= filter && sell_qty > 0.0 {
                        draw_big_trade_bubble(frame, area.candle_center_x - 15.0, y, sell_qty, false, palette);
                    }
                }

                if let Some((threshold, color_scale, ignore_zeros)) = imbalance
                    && area.imb_marker_width > 0.0
                {
                    let higher_price =
                        Price::from_f32(price.to_f32() + step.to_f32_lossy()).round_to_step(step);

                    let start_x_buy = area.bid_area_left;
                    let width_buy = right_area_width;
                    let start_x_sell = area.ask_area_right;
                    let width_sell = -left_area_width;

                    draw_imbalance_markers(
                        frame,
                        &price_to_y,
                        footprint,
                        *price,
                        sell_qty,
                        higher_price,
                        threshold,
                        color_scale,
                        ignore_zeros,
                        cell_height,
                        palette,
                        start_x_buy,
                        width_buy,
                        start_x_sell,
                        width_sell,
                    );
                }
            }

            draw_footprint_kline(
                frame,
                &price_to_y,
                area.candle_center_x,
                candle_width,
                kline,
                palette,
            );
        }
    }

    if show_text {
        let mut total_buy = Qty::zero();
        let mut total_sell = Qty::zero();
        let mut total_delta = Qty::zero();

        for group in footprint.trades.values() {
            total_buy += group.buy_qty;
            total_sell += group.sell_qty;
            total_delta += group.delta_qty();
        }

        let summary_y = price_to_y(kline.low) + cell_height * 1.5;
        let summary_text_size = (text_size * 1.5).clamp(12.0, 18.0);
        let line_spacing = summary_text_size * 1.2;

        let total_vol = total_buy + total_sell;

        draw_cluster_text(
            frame,
            &format!("V: {}", abbr_large_numbers(total_vol.to_f32_lossy())),
            Point::new(x_position, summary_y),
            summary_text_size,
            palette.background.weakest.text,
            Alignment::Center,
            Alignment::Start,
        );

        let delta_color = if total_delta >= Qty::zero() {
            palette.success.base.color
        } else {
            palette.danger.base.color
        };

        draw_cluster_text(
            frame,
            &format!("Δ: {}", abbr_large_numbers(total_delta.to_f32_lossy())),
            Point::new(x_position, summary_y + line_spacing),
            summary_text_size,
            delta_color,
            Alignment::Center,
            Alignment::Start,
        );
    }
}

fn draw_imbalance_markers(
    frame: &mut canvas::Frame,
    price_to_y: &impl Fn(Price) -> f32,
    footprint: &KlineTrades,
    price: Price,
    sell_qty: f32,
    higher_price: Price,
    threshold: usize,
    color_scale: Option<usize>,
    ignore_zeros: bool,
    cell_height: f32,
    palette: &Extended,
    start_x_buy: f32,
    width_buy: f32,
    start_x_sell: f32,
    width_sell: f32,
) {
    if ignore_zeros && sell_qty <= 0.0 {
        return;
    }

    if let Some(group) = footprint.trades.get(&higher_price) {
        let diagonal_buy_qty = f32::from(group.buy_qty);

        if ignore_zeros && diagonal_buy_qty <= 0.0 {
            return;
        }

        let rect_height = cell_height;

        let alpha_from_ratio = |ratio: f32| -> f32 {
            if let Some(scale) = color_scale {
                let divisor = (scale as f32 / 10.0) - 1.0;
                (0.5 + 0.5 * ((ratio - 1.0) / divisor).min(1.0)).min(1.0)
            } else {
                1.0
            }
        };

        if diagonal_buy_qty >= sell_qty {
            let required_qty = sell_qty * (100 + threshold) as f32 / 100.0;
            if diagonal_buy_qty > required_qty {
                let ratio = diagonal_buy_qty / required_qty;
                let alpha = alpha_from_ratio(ratio);

                let y = price_to_y(higher_price);
                frame.stroke(
                    &canvas::Path::rectangle(
                        Point::new(start_x_buy, y - (rect_height / 2.0)),
                        Size::new(width_buy, rect_height),
                    ),
                    canvas::Stroke::default()
                        .with_color(palette.success.base.color.scale_alpha(alpha))
                        .with_width(2.0),
                );
            }
        } else {
            let required_qty = diagonal_buy_qty * (100 + threshold) as f32 / 100.0;
            if sell_qty > required_qty {
                let ratio = sell_qty / required_qty;
                let alpha = alpha_from_ratio(ratio);

                let y = price_to_y(price);
                frame.stroke(
                    &canvas::Path::rectangle(
                        Point::new(start_x_sell, y - (rect_height / 2.0)),
                        Size::new(width_sell, rect_height),
                    ),
                    canvas::Stroke::default()
                        .with_color(palette.danger.base.color.scale_alpha(alpha))
                        .with_width(2.0),
                );
            }
        }
    }
}

impl ContentGaps {
    fn from_view(candle_width: f32, scaling: f32) -> Self {
        let px = |p: f32| p / scaling;
        let base = (candle_width * 0.2).max(px(2.0));
        Self {
            marker_to_candle: base,
            candle_to_cluster: base,
            marker_to_bars: px(2.0),
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct ContentGaps {
    /// Space between imb. markers candle body
    marker_to_candle: f32,
    /// Space between candle body and clusters
    candle_to_cluster: f32,
    /// Inner space reserved between imb. markers and clusters (used for BidAsk)
    marker_to_bars: f32,
}

fn draw_big_trade_bubble(
    frame: &mut canvas::Frame,
    x: f32,
    y: f32,
    qty: f32,
    is_buy: bool,
    palette: &Extended,
) {
    let text = data::util::abbr_large_numbers(qty);
    let text_size = 11.0;
    let radius = 10.0 + (text.len() as f32 * 2.0);
    
    let bg_color = if is_buy { palette.success.base.color } else { palette.danger.base.color };
    let border_color = if is_buy { iced::Color::from_rgb8(60, 160, 255) } else { palette.background.base.color }; // Provide some contrast
    
    let center = Point::new(x, y);
    frame.fill(&canvas::Path::circle(center, radius), bg_color);
    frame.stroke(
        &canvas::Path::circle(center, radius),
        canvas::Stroke::default().with_color(border_color).with_width(1.5),
    );
    
    frame.fill_text(canvas::Text {
        content: text,
        position: center,
        size: iced::Pixels(text_size),
        color: iced::Color::WHITE,
        align_x: iced::alignment::Horizontal::Center.into(),
        align_y: iced::alignment::Vertical::Center.into(),
        font: style::AZERET_MONO,
        ..Default::default()
    });
}

fn draw_cluster_text(
    frame: &mut canvas::Frame,
    text: &str,
    position: Point,
    text_size: f32,
    color: iced::Color,
    align_x: Alignment,
    align_y: Alignment,
) {
    frame.fill_text(canvas::Text {
        content: text.to_string(),
        position,
        size: iced::Pixels(text_size),
        color,
        align_x: align_x.into(),
        align_y: align_y.into(),
        font: style::AZERET_MONO,
        ..canvas::Text::default()
    });
}

fn draw_crosshair_tooltip(
    data: &PlotData<KlineDataPoint>,
    ticker_info: &TickerInfo,
    frame: &mut canvas::Frame,
    palette: &Extended,
    basis: Basis,
    at_interval: Option<u64>,
    visible_range: (u64, u64),
) {
    let (visible_earliest, visible_latest) = visible_range;

    let kline_opt = match (data, at_interval) {
        (PlotData::TimeBased(timeseries), Some(at_interval)) => {
            let in_visible = at_interval >= visible_earliest && at_interval <= visible_latest;

            timeseries
                .datapoints
                .get(&UnixMs::new(at_interval))
                .map(|dp| &dp.kline)
                .or_else(|| {
                    if in_visible {
                        let search_end = at_interval.min(visible_latest);
                        timeseries
                            .datapoints
                            .range(UnixMs::new(visible_earliest)..=UnixMs::new(search_end))
                            .next_back()
                            .map(|(_, dp)| &dp.kline)
                    } else {
                        None
                    }
                })
                .or_else(|| {
                    let right_of_latest = match basis {
                        Basis::Time(_) => at_interval > visible_latest,
                        Basis::Tick(_) => at_interval < visible_earliest,
                    };

                    if right_of_latest {
                        timeseries
                            .datapoints
                            .range(UnixMs::new(visible_earliest)..=UnixMs::new(visible_latest))
                            .next_back()
                            .map(|(_, dp)| &dp.kline)
                    } else {
                        None
                    }
                })
                .or_else(|| {
                    let (last_time, dp) = timeseries.datapoints.last_key_value()?;
                    (at_interval > last_time.as_u64()).then_some(&dp.kline)
                })
        }
        (PlotData::TickBased(tick_aggr), Some(at_interval)) => {
            let kline_at = |interval: u64| {
                let index = (interval / u64::from(tick_aggr.interval.0)) as usize;
                (index < tick_aggr.datapoints.len())
                    .then(|| &tick_aggr.datapoints[tick_aggr.datapoints.len() - 1 - index].kline)
            };

            let in_visible = at_interval >= visible_earliest && at_interval <= visible_latest;

            kline_at(at_interval).or_else(|| {
                let right_of_latest = match basis {
                    Basis::Time(_) => at_interval > visible_latest,
                    Basis::Tick(_) => at_interval < visible_earliest,
                };

                if in_visible || right_of_latest {
                    kline_at(visible_earliest)
                } else {
                    None
                }
            })
        }
        (PlotData::TimeBased(timeseries), None) => timeseries
            .datapoints
            .last_key_value()
            .map(|(_, dp)| &dp.kline),
        (PlotData::TickBased(tick_aggr), None) => tick_aggr.datapoints.last().map(|dp| &dp.kline),
    };

    if let Some(kline) = kline_opt {
        let change_pct = ((kline.close - kline.open).to_f32() / kline.open.to_f32()) * 100.0;
        let change_color = if change_pct >= 0.0 {
            palette.success.base.color
        } else {
            palette.danger.base.color
        };

        let base_color = palette.background.base.text;
        let precision = ticker_info.min_ticksize;

        let segments = [
            ("O", base_color, false),
            (&kline.open.to_string(precision), change_color, true),
            ("H", base_color, false),
            (&kline.high.to_string(precision), change_color, true),
            ("L", base_color, false),
            (&kline.low.to_string(precision), change_color, true),
            ("C", base_color, false),
            (&kline.close.to_string(precision), change_color, true),
            (&format!("{change_pct:+.2}%"), change_color, true),
        ];

        let total_width: f32 = segments
            .iter()
            .map(|(s, _, _)| s.len() as f32 * (TEXT_SIZE * 0.8))
            .sum();

        let position = Point::new(8.0, 8.0);

        let tooltip_rect = Rectangle {
            x: position.x,
            y: position.y,
            width: total_width,
            height: 16.0,
        };

        frame.fill_rectangle(
            tooltip_rect.position(),
            tooltip_rect.size(),
            palette.background.weakest.color.scale_alpha(0.9),
        );

        let mut x = position.x;
        for (text, seg_color, is_value) in segments {
            frame.fill_text(canvas::Text {
                content: text.to_string(),
                position: Point::new(x, position.y),
                size: iced::Pixels(crate::style::text_size::BODY),
                color: seg_color,
                font: style::AZERET_MONO,
                ..canvas::Text::default()
            });
            x += text.len() as f32 * 8.0;
            x += if is_value { 6.0 } else { 2.0 };
        }
    }
}

struct ProfileArea {
    imb_marker_left: f32,
    imb_marker_width: f32,
    bars_left: f32,
    bars_width: f32,
    candle_center_x: f32,
}

impl ProfileArea {
    fn new(
        content_left: f32,
        content_right: f32,
        candle_width: f32,
        gaps: ContentGaps,
        has_imbalance: bool,
    ) -> Self {
        let candle_lane_left = if has_imbalance {
            content_left + candle_width + gaps.marker_to_candle
        } else {
            content_left
        };
        let candle_lane_width = candle_width * 0.25;

        let bars_left = candle_lane_left + candle_lane_width + gaps.candle_to_cluster;
        let bars_width = (content_right - bars_left).max(0.0);

        let candle_center_x = candle_lane_left + (candle_lane_width / 2.0);

        Self {
            imb_marker_left: content_left,
            imb_marker_width: if has_imbalance { candle_width } else { 0.0 },
            bars_left,
            bars_width,
            candle_center_x,
        }
    }
}

struct BidAskArea {
    bid_area_left: f32,
    bid_area_right: f32,
    ask_area_left: f32,
    ask_area_right: f32,
    candle_center_x: f32,
    imb_marker_width: f32,
}

impl BidAskArea {
    fn new(
        x_position: f32,
        content_left: f32,
        content_right: f32,
        candle_width: f32,
        spacing: ContentGaps,
    ) -> Self {
        let candle_body_width = candle_width * 0.25;

        let candle_left = x_position - (candle_body_width / 2.0);
        let candle_right = x_position + (candle_body_width / 2.0);

        let ask_area_right = candle_left - spacing.candle_to_cluster;
        let bid_area_left = candle_right + spacing.candle_to_cluster;

        Self {
            bid_area_left,
            bid_area_right: content_right,
            ask_area_left: content_left,
            ask_area_right,
            candle_center_x: x_position,
            imb_marker_width: candle_width,
        }
    }
}

#[inline]
fn footprint_cluster_min_width(cluster_kind: ClusterKind) -> f32 {
    match cluster_kind {
        ClusterKind::VolumeProfile | ClusterKind::DeltaProfile => 80.0,
        ClusterKind::ProDeltaCandle => 140.0,
        ClusterKind::BidAsk => 120.0,
    }
}

#[inline]
fn footprint_cluster_text_size(cell_height_unscaled: f32, cell_width_unscaled: f32) -> f32 {
    let text_size_from_height = cell_height_unscaled.round().min(18.0) - 1.5;
    let text_size_from_width = (cell_width_unscaled * 0.15).round().min(18.0) - 1.5;

    text_size_from_height.min(text_size_from_width)
}

#[inline]
fn price_padding_from_pixels(cell_height: f32, tick_size: f32) -> f32 {
    const OUTER_BOUND_PADDING_PX: f32 = 4.0;

    if cell_height <= f32::EPSILON {
        return 0.0;
    }

    (OUTER_BOUND_PADDING_PX / cell_height) * tick_size
}

fn footprint_summary_padding(
    cell_height: f32,
    scaling: f32,
    cell_width: f32,
    tick_size: f32,
    cluster_kind: ClusterKind,
) -> f32 {
    if cell_height <= f32::EPSILON {
        return 0.0;
    }

    let cell_height_unscaled = cell_height * scaling;
    let cell_width_unscaled = cell_width * scaling;

    if !should_show_text(
        cell_height_unscaled,
        cell_width_unscaled,
        footprint_cluster_min_width(cluster_kind),
    ) {
        return 0.0;
    }

    let text_size = footprint_cluster_text_size(cell_height_unscaled, cell_width_unscaled);
    let line_spacing = text_size * 1.2;

    let summary_text_height_px = text_size * 0.9;
    let summary_y_start_px = cell_height * 1.5;

    let second_line_y_start_px = summary_y_start_px + line_spacing;
    let summary_y_end_px = second_line_y_start_px + summary_text_height_px;

    let extra_bottom_padding_px = summary_text_height_px;
    let summary_y_end_with_padding_px = summary_y_end_px + extra_bottom_padding_px;
    let summary_ticks = summary_y_end_with_padding_px / cell_height;

    summary_ticks * tick_size
}

#[inline]
fn should_show_text(cell_height_unscaled: f32, cell_width_unscaled: f32, min_w: f32) -> bool {
    cell_height_unscaled > 8.0 && cell_width_unscaled > min_w
}

fn draw_candle_stats(
    frame: &mut canvas::Frame,
    price_to_y: impl Fn(Price) -> f32,
    x_position: f32,
    kline: &Kline,
    trades: &data::chart::kline::KlineTrades,
    text_size: f32,
    palette: &Extended,
    show_text: bool,
) {
    if !show_text {
        return;
    }

    let total_vol = trades.total_qty();
    let delta = trades.delta_qty();

    if total_vol == Qty::zero() {
        return;
    }

    let delta_pct = (f32::from(delta) / f32::from(total_vol)) * 100.0;
    
    let text_color = palette.background.weakest.text;
    let delta_color = if delta >= Qty::zero() {
        palette.success.base.color
    } else {
        palette.danger.base.color
    };

    let y_high = price_to_y(kline.high);
    let summary_text_size = (text_size * 1.5).clamp(12.0, 18.0);
    let mut current_y = y_high - (summary_text_size * 1.2);

    let stats = [
        (format!("{}", data::util::abbr_large_numbers(f32::from(total_vol))), text_color),
        (format!("D:{}", data::util::abbr_large_numbers(f32::from(delta))), delta_color),
        (format!("{:+.1}%", delta_pct), delta_color),
    ];

    for (text, color) in stats.iter().rev() {
        draw_cluster_text(
            frame,
            text,
            Point::new(x_position, current_y),
            summary_text_size,
            *color,
            Alignment::Center,
            Alignment::End,
        );
        current_y -= summary_text_size * 1.2;
    }
}

fn draw_session_profile(
    data_source: &PlotData<KlineDataPoint>,
    frame: &mut canvas::Frame,
    chart: &ViewState,
    region: &Rectangle,
    palette: &Extended,
    step: f32,
) {
    let mut session_trades = std::collections::BTreeMap::new();
    let mut total_vol = Qty::zero();
    let mut total_delta = Qty::zero();

    match data_source {
        PlotData::TickBased(tick_aggr) => {
            for dp in &tick_aggr.datapoints {
                for (price, group) in &dp.footprint.trades {
                    let entry = session_trades.entry(*price).or_insert_with(data::chart::kline::GroupedTrades::default);
                    entry.buy_qty += group.buy_qty;
                    entry.sell_qty += group.sell_qty;
                    total_vol += group.total_qty();
                    total_delta += group.delta_qty();
                }
            }
        }
        PlotData::TimeBased(timeseries) => {
            for (_, dp) in &timeseries.datapoints {
                for (price, group) in &dp.footprint.trades {
                    let entry = session_trades.entry(*price).or_insert_with(data::chart::kline::GroupedTrades::default);
                    entry.buy_qty += group.buy_qty;
                    entry.sell_qty += group.sell_qty;
                    total_vol += group.total_qty();
                    total_delta += group.delta_qty();
                }
            }
        }
    }

    if session_trades.is_empty() { return; }

    let max_vol = session_trades.values().map(|g| g.total_qty()).max().unwrap_or_default();
    let max_delta = session_trades.values().map(|g| g.buy_qty.abs_diff(g.sell_qty)).max().unwrap_or_default();

    let max_vol_f32 = f32::from(max_vol);
    let max_delta_f32 = f32::from(max_delta);
    let total_vol_f32 = f32::from(total_vol);

    let poc = session_trades.iter().max_by_key(|(_, g)| g.total_qty()).map(|(p, _)| *p).unwrap();

    let target_va_vol = total_vol_f32 * 0.70;
    let mut val = poc;
    let mut vah = poc;
    let mut current_vol = f32::from(session_trades.get(&poc).unwrap().total_qty());

    let prices: Vec<Price> = session_trades.keys().copied().collect();
    if let Ok(poc_idx) = prices.binary_search(&poc) {
        let mut upper_idx = poc_idx;
        let mut lower_idx = poc_idx;
        while current_vol < target_va_vol && (upper_idx < prices.len() - 1 || lower_idx > 0) {
            let vol_up = if upper_idx < prices.len() - 1 {
                f32::from(session_trades.get(&prices[upper_idx + 1]).unwrap().total_qty())
            } else {
                -1.0
            };
            let vol_down = if lower_idx > 0 {
                f32::from(session_trades.get(&prices[lower_idx - 1]).unwrap().total_qty())
            } else {
                -1.0
            };
            if vol_up > vol_down {
                upper_idx += 1;
                vah = prices[upper_idx];
                current_vol += vol_up;
            } else if vol_down >= vol_up && vol_down >= 0.0 {
                lower_idx -= 1;
                val = prices[lower_idx];
                current_vol += vol_down;
            } else {
                break;
            }
        }
    }

    let total_width = 160.0 / chart.scaling;
    let delta_width = total_width * 0.5;
    let vp_width = total_width * 0.5;

    let right_edge = region.x + region.width;
    let profile_start_x = right_edge - total_width;
    let delta_center_x = profile_start_x + (delta_width / 2.0);
    let vp_right_x = right_edge;

    let y1 = chart.price_to_y(Price::from_f32(0.0));
    let y2 = chart.price_to_y(Price::from_f32(step));
    let cell_height = (y1 - y2).abs();
    let bar_height = (cell_height * 0.85).max(1.0);

    let y_vah = chart.price_to_y(vah) - (cell_height / 2.0);
    let y_val = chart.price_to_y(val) + (cell_height / 2.0);

    let dash_stroke = Stroke {
        width: 1.0 / chart.scaling,
        style: canvas::stroke::Style::Solid(palette.background.weakest.text.scale_alpha(0.5)),
        line_dash: canvas::LineDash { segments: &[4.0, 4.0], offset: 0 },
        ..Default::default()
    };
    
    let mut vah_line = canvas::path::Builder::new();
    vah_line.move_to(Point::new(profile_start_x, y_vah));
    vah_line.line_to(Point::new(right_edge, y_vah));
    frame.stroke(&vah_line.build(), dash_stroke.clone());

    let mut val_line = canvas::path::Builder::new();
    val_line.move_to(Point::new(profile_start_x, y_val));
    val_line.line_to(Point::new(right_edge, y_val));
    frame.stroke(&val_line.build(), dash_stroke);

    let y_top = chart.price_to_y(*prices.last().unwrap());
    let y_bottom = chart.price_to_y(*prices.first().unwrap());
    let mut zero_line = canvas::path::Builder::new();
    zero_line.move_to(Point::new(delta_center_x, y_top));
    zero_line.line_to(Point::new(delta_center_x, y_bottom));
    frame.stroke(&zero_line.build(), Stroke {
        width: 1.0 / chart.scaling,
        style: canvas::stroke::Style::Solid(palette.background.weakest.text.scale_alpha(0.3)),
        ..Default::default()
    });

    for (price, group) in &session_trades {
        let y = chart.price_to_y(*price);
        let bar_y = y - (bar_height / 2.0);

        let delta = f32::from(group.delta_qty());
        let d_width = if max_delta_f32 > 0.0 { (delta.abs() / max_delta_f32) * (delta_width / 2.0) } else { 0.0 };
        if d_width > 0.0 {
            let (color, x_start) = if delta >= 0.0 {
                (palette.success.base.color, delta_center_x)
            } else {
                (palette.danger.base.color, delta_center_x - d_width)
            };
            frame.fill_rectangle(
                Point::new(x_start, bar_y),
                Size::new(d_width, bar_height),
                color,
            );
        }

        let vol = f32::from(group.buy_qty) + f32::from(group.sell_qty);
        let v_width = if max_vol_f32 > 0.0 { (vol / max_vol_f32) * vp_width } else { 0.0 };
        if v_width > 0.0 {
            let in_va = *price >= val && *price <= vah;
            
            let fill_color = if in_va {
                iced::Color::from_rgb8(30, 80, 160) // dark blue for Value Area
            } else {
                iced::Color::from_rgb8(100, 180, 200) // light cyan for outside
            };

            frame.fill_rectangle(
                Point::new(vp_right_x - v_width, bar_y),
                Size::new(v_width, bar_height),
                fill_color,
            );
        }
    }

    let poc_y = chart.price_to_y(poc);
    let mut poc_line = canvas::path::Builder::new();
    poc_line.move_to(Point::new(profile_start_x, poc_y));
    poc_line.line_to(Point::new(right_edge, poc_y));
    frame.stroke(&poc_line.build(), Stroke {
        width: 1.0 / chart.scaling,
        style: canvas::stroke::Style::Solid(palette.danger.base.color),
        ..Default::default()
    });

    let text_size = 14.0 / chart.scaling;
    let summary_y = y_bottom + (20.0 / chart.scaling);
    
    frame.fill_text(canvas::Text {
        content: format!("V: {}", abbr_large_numbers(total_vol_f32)),
        position: Point::new(delta_center_x, summary_y),
        size: iced::Pixels(text_size),
        color: palette.background.weakest.text,
        align_x: Alignment::Center.into(),
        align_y: Alignment::Start.into(),
        font: crate::style::AZERET_MONO,
        ..Default::default()
    });

    let delta_f32 = f32::from(total_delta);
    let delta_color = if delta_f32 >= 0.0 { palette.success.base.color } else { palette.danger.base.color };
    frame.fill_text(canvas::Text {
        content: format!("Δ: {}", abbr_large_numbers(delta_f32)),
        position: Point::new(delta_center_x, summary_y + (text_size * 1.5)),
        size: iced::Pixels(text_size),
        color: delta_color,
        align_x: Alignment::Center.into(),
        align_y: Alignment::Start.into(),
        font: crate::style::AZERET_MONO,
        ..Default::default()
    });
}
