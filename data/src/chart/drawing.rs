use exchange::unit::Price;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChartPoint {
    pub time: u64,
    pub price: Price,
}

impl ChartPoint {
    pub fn new(time: u64, price: Price) -> Self {
        Self { time, price }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DrawingEvent {
    MouseDown(ChartPoint),
    MouseMove(ChartPoint),
    MouseUp(ChartPoint),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum DrawingType {
    #[default]
    Cursor, // Not a drawing, just normal interaction
    TrendLine,
    Ray,
    HorizontalLine,
    Rectangle,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DrawingState {
    /// Waiting for the first click
    Initial,
    /// One point has been placed, waiting for the second
    OnePoint(ChartPoint),
    /// The drawing is complete with both points
    Completed(ChartPoint, ChartPoint),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Drawing {
    pub id: Uuid,
    pub kind: DrawingType,
    pub state: DrawingState,
    #[serde(default)]
    pub selected: bool,
    #[serde(skip)]
    pub hovered_point: Option<usize>, // 0 for start, 1 for end
}

impl Drawing {
    pub fn new(kind: DrawingType) -> Self {
        Self {
            id: Uuid::new_v4(),
            kind,
            state: DrawingState::Initial,
            selected: false,
            hovered_point: None,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChartDrawings {
    pub drawings: Vec<Drawing>,
    #[serde(skip)]
    pub current_cursor: Option<ChartPoint>,
}

impl ChartDrawings {
    pub fn add_drawing(&mut self, drawing: Drawing) {
        self.drawings.push(drawing);
    }

    pub fn active_drawing_mut(&mut self) -> Option<&mut Drawing> {
        self.drawings.last_mut().filter(|d| !matches!(d.state, DrawingState::Completed(_, _)))
    }

    pub fn active_drawing(&self) -> Option<&Drawing> {
        self.drawings.last().filter(|d| !matches!(d.state, DrawingState::Completed(_, _)))
    }

    pub fn delete_selected(&mut self) -> bool {
        let len = self.drawings.len();
        self.drawings.retain(|d| !d.selected);
        self.drawings.len() < len
    }

    pub fn clear(&mut self) {
        self.drawings.clear();
        self.current_cursor = None;
    }

    pub fn handle_event(&mut self, event: DrawingEvent, current_tool: DrawingType) -> bool {
        if current_tool == DrawingType::Cursor {
            return false;
        }

        match event {
            DrawingEvent::MouseDown(point) => {
                if let Some(active) = self.active_drawing_mut() {
                    match active.state {
                        DrawingState::Initial => {
                            active.state = DrawingState::OnePoint(point);
                        }
                        DrawingState::OnePoint(_) => {
                            active.state = DrawingState::Completed(active.state.clone().into_one_point().unwrap(), point);
                        }
                        _ => {}
                    }
                } else {
                    let mut d = Drawing::new(current_tool);
                    if current_tool == DrawingType::HorizontalLine {
                        d.state = DrawingState::Completed(point, point);
                    } else {
                        d.state = DrawingState::OnePoint(point);
                    }
                    self.add_drawing(d);
                }
                true
            }
            DrawingEvent::MouseMove(point) => {
                self.current_cursor = Some(point);
                // We only need to redraw if there is an active drawing
                self.active_drawing().is_some()
            }
            DrawingEvent::MouseUp(point) => {
                let mut completed = false;
                if let Some(active) = self.active_drawing_mut() {
                    if let DrawingState::OnePoint(p) = active.state {
                        // If it's a drag (not a pure click), complete it
                        if p.time != point.time || p.price != point.price {
                            active.state = DrawingState::Completed(p, point);
                            completed = true;
                        }
                    }
                }
                completed
            }
        }
    }
}

impl DrawingState {
    pub fn into_one_point(self) -> Option<ChartPoint> {
        if let DrawingState::OnePoint(p) = self {
            Some(p)
        } else {
            None
        }
    }
}
