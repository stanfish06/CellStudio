use serde::{Deserialize, Serialize};

/// Annotated cell state. Independent of graph structure: a `Dividing` annotation is
/// stored as given regardless of child count.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CellState {
    Normal,
    Dividing,
    Death,
}

impl CellState {
    pub fn as_str(&self) -> &'static str {
        match self {
            CellState::Normal => "normal",
            CellState::Dividing => "dividing",
            CellState::Death => "death",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "normal" => Some(CellState::Normal),
            "dividing" => Some(CellState::Dividing),
            "death" => Some(CellState::Death),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LinkRef {
    pub id: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
}

/// One detection in a tracking file. `seg_id` is the label value in the mask at frame
/// `t`; `track_id` is a pre-existing identity from an upstream tracker.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CellRecord {
    pub id: u32,
    pub t: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seg_id: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track_id: Option<u32>,
    /// `[z, y, x]` in pixel units.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub centroid: Option<[f64; 3]>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<LinkRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<LinkRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<CellState>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub features: serde_json::Map<String, serde_json::Value>,
}
