use super::tickers_table::{self, TickersTable};
use crate::{
    TooltipPosition,
    layout::SavedState,
    style::{Icon, icon_text},
    widget::button_with_tooltip,
};
use data::sidebar;

use iced::{
    Alignment, Element, Subscription, Task,
    widget::responsive,
    widget::{column, row, space},
};
use rustc_hash::FxHashMap;

#[derive(Debug, Clone)]
pub enum Message {
    ToggleSidebarMenu(Option<sidebar::Menu>),
    SetSidebarPosition(sidebar::Position),
    TickersTable(super::tickers_table::Message),
    SelectDrawingTool(data::chart::drawing::DrawingType),
    ClearDrawings,
}

pub struct Sidebar {
    pub state: data::Sidebar,
    pub tickers_table: TickersTable,
}

pub enum Action {
    TickerSelected(
        exchange::TickerInfo,
        Option<data::layout::pane::ContentKind>,
    ),
    ErrorOccurred(data::InternalError),
    SelectDrawingTool(data::chart::drawing::DrawingType),
    ClearDrawings,
}

impl Sidebar {
    pub fn new(
        state: &SavedState,
        handles: exchange::adapter::AdapterHandles,
    ) -> (Self, Task<Message>) {
        let (tickers_table, initial_fetch) =
            if let Some(settings) = state.sidebar.tickers_table.as_ref() {
                TickersTable::new_with_settings(settings, handles.clone())
            } else {
                TickersTable::new(handles)
            };

        (
            Self {
                state: state.sidebar.clone(),
                tickers_table,
            },
            initial_fetch.map(Message::TickersTable),
        )
    }

    pub fn update(&mut self, message: Message) -> (Task<Message>, Option<Action>) {
        match message {
            Message::ToggleSidebarMenu(menu) => {
                self.set_menu(menu.filter(|&m| !self.is_menu_active(m)));
            }
            Message::SetSidebarPosition(pos) => {
                self.state.position = pos;
                return (Task::none(), None);
            }
            Message::SelectDrawingTool(tool) => {
                return (Task::none(), Some(Action::SelectDrawingTool(tool)));
            }
            Message::ClearDrawings => {
                return (Task::none(), Some(Action::ClearDrawings));
            }
            Message::TickersTable(msg) => {
                let action = self.tickers_table.update(msg);

                match action {
                    Some(tickers_table::Action::TickerSelected(ticker_info, content)) => {
                        return (
                            Task::none(),
                            Some(Action::TickerSelected(ticker_info, content)),
                        );
                    }
                    Some(tickers_table::Action::Fetch(task)) => {
                        return (task.map(Message::TickersTable), None);
                    }
                    Some(tickers_table::Action::ErrorOccurred(error)) => {
                        return (Task::none(), Some(Action::ErrorOccurred(error)));
                    }
                    Some(tickers_table::Action::FocusWidget(id)) => {
                        return (iced::widget::operation::focus(id), None);
                    }
                    None => {}
                }
            }
        }

        (Task::none(), None)
    }

    pub fn view(&self, audio_volume: Option<f32>, selected_drawing_tool: data::chart::drawing::DrawingType) -> Element<'_, Message> {
        let state = &self.state;

        let tooltip_position = if state.position == sidebar::Position::Left {
            TooltipPosition::Right
        } else {
            TooltipPosition::Left
        };

        let is_table_open = self.tickers_table.is_shown;

        let nav_buttons = self.nav_buttons(is_table_open, audio_volume, tooltip_position, selected_drawing_tool);

        let tickers_table = if is_table_open {
            column![responsive(move |size| self
                .tickers_table
                .view(size)
                .map(Message::TickersTable))]
            .width(200)
        } else {
            column![]
        };

        match state.position {
            sidebar::Position::Left => row![nav_buttons, tickers_table],
            sidebar::Position::Right => row![tickers_table, nav_buttons],
        }
        .spacing(if is_table_open { 8 } else { 4 })
        .into()
    }

    pub fn subscription(&self) -> Subscription<Message> {
        self.tickers_table.subscription().map(Message::TickersTable)
    }

    fn nav_buttons(
        &self,
        is_table_open: bool,
        audio_volume: Option<f32>,
        tooltip_position: TooltipPosition,
        selected_drawing_tool: data::chart::drawing::DrawingType,
    ) -> iced::widget::Column<'_, Message> {
        let settings_modal_button = {
            let is_active = self.is_menu_active(sidebar::Menu::Settings)
                || self.is_menu_active(sidebar::Menu::ThemeEditor)
                || self.is_menu_active(sidebar::Menu::Network);

            button_with_tooltip(
                icon_text(Icon::Cog, 16)
                    .width(32)
                    .align_x(Alignment::Center),
                Message::ToggleSidebarMenu(Some(sidebar::Menu::Settings)),
                None,
                tooltip_position,
                move |theme, status| crate::style::button::transparent(theme, status, is_active),
            )
        };

        let layout_modal_button = {
            let is_active = self.is_menu_active(sidebar::Menu::Layout);

            button_with_tooltip(
                icon_text(Icon::Layout, 16)
                    .width(32)
                    .align_x(Alignment::Center),
                Message::ToggleSidebarMenu(Some(sidebar::Menu::Layout)),
                None,
                tooltip_position,
                move |theme, status| crate::style::button::transparent(theme, status, is_active),
            )
        };

        let ticker_search_button = {
            button_with_tooltip(
                icon_text(Icon::Search, 16)
                    .width(32)
                    .align_x(Alignment::Center),
                Message::TickersTable(super::tickers_table::Message::ToggleTable),
                None,
                tooltip_position,
                move |theme, status| {
                    crate::style::button::transparent(theme, status, is_table_open)
                },
            )
        };

        let audio_btn = {
            let is_active = self.is_menu_active(sidebar::Menu::Audio);

            let icon = match audio_volume.unwrap_or(0.0) {
                v if v >= 40.0 => Icon::SpeakerHigh,
                v if v > 0.0 => Icon::SpeakerLow,
                _ => Icon::SpeakerOff,
            };

            button_with_tooltip(
                icon_text(icon, 16).width(32).align_x(Alignment::Center),
                Message::ToggleSidebarMenu(Some(sidebar::Menu::Audio)),
                None,
                tooltip_position,
                move |theme, status| crate::style::button::transparent(theme, status, is_active),
            )
        };

        let drawing_tools = {
            use data::chart::drawing::DrawingType;
            let tools = vec![
                (Icon::DragHandle, DrawingType::Cursor, "Cursor"),
                (Icon::Edit, DrawingType::TrendLine, "Trend Line"),
                (Icon::Sort, DrawingType::HorizontalLine, "Horizontal Line"),
                (Icon::Layout, DrawingType::Rectangle, "Rectangle"),
            ];

            let mut col = column![].spacing(12);
            for (icon, kind, tooltip) in tools {
                let is_active = selected_drawing_tool == kind;
                let btn = button_with_tooltip(
                    icon_text(icon, 16).width(32).align_x(Alignment::Center),
                    Message::SelectDrawingTool(kind),
                    Some(tooltip),
                    tooltip_position,
                    move |theme, status| crate::style::button::transparent(theme, status, is_active),
                );
                col = col.push(btn);
            }
            
            // Clear All Button
            let clear_btn = button_with_tooltip(
                icon_text(Icon::TrashBin, 16).width(32).align_x(Alignment::Center),
                Message::ClearDrawings,
                Some("Clear All Drawings"),
                tooltip_position,
                move |theme, status| crate::style::button::transparent(theme, status, false),
            );
            col = col.push(clear_btn);

            col
        };

        column![
            ticker_search_button,
            layout_modal_button,
            audio_btn,
            space::vertical(),
            drawing_tools,
            space::vertical(),
            settings_modal_button,
        ]
        .width(44)
        .padding(iced::Padding::new(6.0).top(12.0).bottom(12.0))
        .spacing(16)
    }

    pub fn hide_tickers_table(&mut self) -> bool {
        let table = &mut self.tickers_table;

        if table.expand_ticker_card.is_some() {
            table.expand_ticker_card = None;
            return true;
        } else if table.is_shown {
            table.is_shown = false;
            return true;
        }

        false
    }

    pub fn is_menu_active(&self, menu: sidebar::Menu) -> bool {
        self.state.active_menu == Some(menu)
    }

    pub fn active_menu(&self) -> Option<sidebar::Menu> {
        self.state.active_menu
    }

    pub fn position(&self) -> sidebar::Position {
        self.state.position
    }

    pub fn set_menu(&mut self, menu: Option<sidebar::Menu>) {
        self.state.active_menu = menu;
    }

    pub fn sync_tickers_table_settings(&mut self) {
        let settings = &self.tickers_table.settings();
        self.state.tickers_table = Some(settings.clone());
    }

    pub fn tickers_info(&self) -> &FxHashMap<exchange::Ticker, Option<exchange::TickerInfo>> {
        &self.tickers_table.tickers_info
    }
}
