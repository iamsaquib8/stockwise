//! Investor-profile onboarding: a handful of plain questions, and a fixed
//! mapping from the answers to `[profile]` settings. An assistant asks the
//! questions (MCP prompt `onboard_profile`, tool `profile_questions`) and
//! saves the result with `profile_onboard`; the mapping is here, not in the
//! model, so the same answers always give the same profile.

use crate::config::{ProfileMode, ProfileUpdate, RiskLevel, TaxRegime};
use serde::Serialize;
use serde_json::{Value, json};

/// One question and its answer choices (the value to send back, and what
/// it means).
#[derive(Debug, Clone, Serialize)]
pub struct Question {
    pub id: &'static str,
    pub question: &'static str,
    pub choices: &'static [(&'static str, &'static str)],
    /// A number the user types instead of choosing.
    pub number: Option<(&'static str, f64, f64)>,
    pub required: bool,
}

pub const QUESTIONS: &[Question] = &[
    Question {
        id: "involvement",
        question: "How closely do you want to follow your investments?",
        choices: &[
            ("rarely", "A few times a year: mostly funds and long holds"),
            ("regularly", "Every week or two: my own stock picks too"),
            ("daily", "Most days: short-term positions and trades"),
        ],
        number: None,
        required: true,
    },
    Question {
        id: "fall_reaction",
        question: "If your portfolio fell 20% in a month, what would you most likely do?",
        choices: &[
            (
                "move_to_safety",
                "Move to safer investments to stop further losses",
            ),
            ("wait", "Wait it out and change nothing"),
            ("invest_more", "Invest more while prices are lower"),
        ],
        number: None,
        required: true,
    },
    Question {
        id: "horizon_years",
        question: "In how many years do you expect to need most of this money?",
        choices: &[],
        number: Some(("years", 1.0, 60.0)),
        required: true,
    },
    Question {
        id: "experience",
        question: "How long have you been investing in stocks or funds?",
        choices: &[
            ("new", "Less than a year"),
            ("some", "One to five years"),
            ("seasoned", "More than five years"),
        ],
        number: None,
        required: false,
    },
    Question {
        id: "tax_slab",
        question: "What is your income-tax slab, in percent (0, 5, 10, 15, 20, 30)?",
        choices: &[],
        number: Some(("percent", 0.0, 50.0)),
        required: false,
    },
    Question {
        id: "tax_regime",
        question: "Indian tax regime: new or old? (skip outside India)",
        choices: &[("new", "New regime"), ("old", "Old regime")],
        number: None,
        required: false,
    },
];

/// The questions as a tool returns them.
pub fn questions_json() -> Value {
    json!({
        "questions": QUESTIONS,
        "how": "Ask these one at a time in plain words, then send the answers to profile_onboard. \
                Required: involvement, fall_reaction, horizon_years.",
    })
}

/// Answers given to [`derive`].
#[derive(Debug, Clone, Default)]
pub struct Answers {
    pub involvement: String,
    pub fall_reaction: String,
    pub horizon_years: u32,
    pub experience: Option<String>,
    pub tax_slab: Option<f64>,
    pub tax_regime: Option<String>,
}

/// The profile the answers lead to, and the reason for each setting.
pub fn derive(a: &Answers) -> Result<(ProfileUpdate, Vec<String>), String> {
    let mut why = Vec::new();
    let mode = match a.involvement.as_str() {
        "rarely" => ProfileMode::Passive,
        "regularly" => ProfileMode::Active,
        "daily" => ProfileMode::Ultra,
        other => {
            return Err(format!(
                "involvement must be rarely, regularly or daily, not `{other}`"
            ));
        }
    };
    why.push(format!(
        "mode {}: you follow your investments {}",
        mode.as_str(),
        a.involvement
    ));
    // Comfort with a fall sets the starting level; a short horizon or
    // little experience lowers it one step.
    let mut level: i32 = match a.fall_reaction.as_str() {
        "move_to_safety" => 0,
        "wait" => 1,
        "invest_more" => 2,
        other => {
            return Err(format!(
                "fall_reaction must be move_to_safety, wait or invest_more, not `{other}`"
            ));
        }
    };
    if a.horizon_years < 3 {
        level -= 1;
        why.push(format!(
            "a horizon of {} years lowers the risk level one step",
            a.horizon_years
        ));
    }
    if a.experience.as_deref() == Some("new") {
        level -= 1;
        why.push("less than a year of investing lowers the risk level one step".into());
    }
    let risk = match level.clamp(0, 2) {
        0 => RiskLevel::Low,
        1 => RiskLevel::Med,
        _ => RiskLevel::High,
    };
    why.push(format!(
        "risk {}: after a 20% fall you would {}",
        risk.as_str(),
        a.fall_reaction.replace('_', " ")
    ));
    let tax_regime = match a.tax_regime.as_deref() {
        None | Some("") => None,
        Some(r) => Some(TaxRegime::parse(r)?),
    };
    if !(1..=60).contains(&a.horizon_years) {
        return Err("horizon_years must be between 1 and 60".into());
    }
    Ok((
        ProfileUpdate {
            mode: Some(mode),
            risk: Some(risk),
            tax_slab: a.tax_slab,
            tax_regime,
            horizon_years: Some(a.horizon_years),
            // The mode and risk set the guardrails' defaults.
            max_stock_weight: None,
            max_smallcap_weight: None,
            risk_per_trade: None,
            residence: None,
        },
        why,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answers(inv: &str, fall: &str, years: u32) -> Answers {
        Answers {
            involvement: inv.into(),
            fall_reaction: fall.into(),
            horizon_years: years,
            ..Default::default()
        }
    }

    #[test]
    fn answers_map_to_a_fixed_profile() {
        let (u, _) = derive(&answers("rarely", "wait", 15)).unwrap();
        assert_eq!(
            (u.mode, u.risk),
            (Some(ProfileMode::Passive), Some(RiskLevel::Med))
        );
        let (u, _) = derive(&answers("daily", "invest_more", 10)).unwrap();
        assert_eq!(
            (u.mode, u.risk),
            (Some(ProfileMode::Ultra), Some(RiskLevel::High))
        );
        // A short horizon and a new investor each lower the level.
        let mut a = answers("regularly", "invest_more", 2);
        a.experience = Some("new".into());
        let (u, why) = derive(&a).unwrap();
        assert_eq!(u.risk, Some(RiskLevel::Low));
        assert_eq!(why.len(), 4);
        assert!(derive(&answers("sometimes", "wait", 5)).is_err());
        assert!(derive(&answers("rarely", "wait", 0)).is_err());
    }

    #[test]
    fn questions_are_complete_and_neutral() {
        let v = questions_json();
        let text = v.to_string();
        crate::action::testing::assert_action_well_formed(&text);
        for q in QUESTIONS {
            assert!(q.number.is_some() || !q.choices.is_empty(), "{}", q.id);
        }
    }
}
