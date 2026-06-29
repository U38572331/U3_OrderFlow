use serde::{Deserialize, Serialize};

#[derive(Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct GWTradeConfig {
    pub token: String,
}
