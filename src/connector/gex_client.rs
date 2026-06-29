use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tokio::time::sleep;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{client::IntoClientRequest, Message as WsMessage},
};

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct GexLevel {
    pub id: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub symbol: String,
    #[serde(default)]
    pub strike_ndx: f64,
    #[serde(default)]
    pub value: f64,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub ts_ms: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum GexEvent {
    Connected,
    Set(GexLevel),
    Remove(String),
    Clear,
    Disconnected,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum Payload {
    Set(GexLevel),
    Remove { id: String },
    Clear,
    Hello { backoff_hint_sec: Option<f64> },
    Pong,
}

pub fn subscribe(url: String, token: String) -> iced::Subscription<GexEvent> {
    struct GexWs;

    iced::Subscription::run_with(
        (std::any::TypeId::of::<GexWs>(), url, token),
        |(_, url, token)| {
            let url = url.clone();
            let token = token.clone();
            iced::stream::channel(
                100,
                move |mut output: iced::futures::channel::mpsc::Sender<GexEvent>| {
                    let url = url.clone();
                    let token = token.clone();
                    async move {
                        let mut backoff = 2.0f64;
                        let min_backoff = 2.0f64;
                        let max_backoff = 60.0f64;

                        loop {
                            let ws_url = format!("{}?token={}", url, urlencoding::encode(&token));
                            let mut request = match ws_url.into_client_request() {
                                Ok(req) => req,
                                Err(e) => {
                                    log::error!("Invalid Gex URL: {}", e);
                                    sleep(Duration::from_secs(60)).await;
                                    continue;
                                }
                            };
                            
                            match connect_async(request).await {
                                Ok((mut ws_stream, _)) => {
                                    log::info!("Connected to GEX WebSocket");
                                    let _ = output.send(GexEvent::Connected).await;
                                    backoff = min_backoff;

                                    while let Some(msg) = ws_stream.next().await {
                                        match msg {
                                            Ok(WsMessage::Text(text)) => {
                                                if let Ok(payload) = serde_json::from_str::<Payload>(&text) {
                                                    match payload {
                                                        Payload::Set(level) => {
                                                            let _ = output.send(GexEvent::Set(level)).await;
                                                        }
                                                        Payload::Remove { id } => {
                                                            let _ = output.send(GexEvent::Remove(id)).await;
                                                        }
                                                        Payload::Clear => {
                                                            let _ = output.send(GexEvent::Clear).await;
                                                        }
                                                        Payload::Hello { .. } | Payload::Pong => {}
                                                    }
                                                }
                                            }
                                            Ok(WsMessage::Ping(_)) => {
                                                let _ = ws_stream.send(WsMessage::Pong(vec![].into())).await;
                                            }
                                            Ok(WsMessage::Close(_)) => break,
                                            Err(_) => break,
                                            _ => {}
                                        }
                                    }
                                }
                                Err(e) => {
                                    log::error!("GEX WebSocket error: {}", e);
                                }
                            }
                            
                            let _ = output.send(GexEvent::Disconnected).await;

                            // Exponential backoff
                            let jitter = rand::random::<f64>() * min_backoff.min(backoff);
                            sleep(Duration::from_secs_f64((backoff + jitter).min(max_backoff))).await;
                            backoff = (backoff * 2.0).min(max_backoff);
                        }
                    }
                },
            )
        },
    )
}
