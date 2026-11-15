//! Your notes on a stock: why you own or watch it, and the limits you set
//! for it yourself (P/E, ROE, weight). Kept in `notes.json`; `fit_check`
//! measures a stock against them.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The notes file in the data directory.
pub const FILE: &str = "notes.json";

/// One stock's note.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Note {
    /// Why you hold or watch it, in your words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thesis: Option<String>,
    /// Highest trailing P/E you accept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_pe: Option<f64>,
    /// Lowest return on equity you accept, percent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_roe_pct: Option<f64>,
    /// Largest share of the portfolio you want it to be, percent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_weight_pct: Option<f64>,
    #[serde(default)]
    pub updated: String,
}

/// Every note, by upper-case symbol.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct NoteStore {
    #[serde(default)]
    pub notes: BTreeMap<String, Note>,
}

impl NoteStore {
    pub fn get(&self, symbol: &str) -> Option<&Note> {
        self.notes.get(&symbol.to_ascii_uppercase())
    }
}
