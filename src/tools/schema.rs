//! Output schemas (MCP `outputSchema`) for the main tools, so hosts can
//! read `structuredContent` without guessing its shape.
//!
//! Each schema is written from the Rust type the handler serializes
//! (`commands::QuoteRow`, `verdict::DecisionVerdict`, `picks::Run`,
//! `commands::PortfolioReport`, `rebalance::RebalancePlan`, ...) with the
//! small builders below. They name the fields a host can rely on and leave
//! room for more (`additionalProperties` stays open), so adding a field to a
//! result type never breaks a client. Every `f64` is nullable, because
//! `serde_json` writes a NaN as `null`. The tests serialize real results and
//! check them against these schemas with [`validate`], so a renamed or
//! retyped field fails the build's tests rather than a host.

use serde_json::{Map, Value, json};

fn t(kind: &str) -> Value {
    json!({ "type": kind })
}

fn string() -> Value {
    t("string")
}

fn int() -> Value {
    t("integer")
}

fn boolean() -> Value {
    t("boolean")
}

fn object() -> Value {
    t("object")
}

/// A number that may be `null` (an `Option<f64>`, or a NaN).
fn num() -> Value {
    json!({ "type": ["number", "null"] })
}

fn nullable(kind: &str) -> Value {
    json!({ "type": [kind, "null"] })
}

fn arr(items: Value) -> Value {
    json!({ "type": "array", "items": items })
}

fn strings() -> Value {
    arr(string())
}

fn one_of(values: &[&str]) -> Value {
    json!({ "type": "string", "enum": values })
}

/// An object with `required` fields that must be present and `optional`
/// ones that are checked when present. Other fields are allowed.
fn obj(required: &[(&str, Value)], optional: &[(&str, Value)]) -> Value {
    let mut props = Map::new();
    for (k, v) in required.iter().chain(optional) {
        props.insert((*k).to_string(), v.clone());
    }
    json!({
        "type": "object",
        "properties": props,
        "required": required.iter().map(|(k, _)| *k).collect::<Vec<_>>(),
    })
}

fn quote_row() -> Value {
    obj(
        &[
            ("symbol", nullable("string")),
            ("price", num()),
            ("cap_tier", string()),
        ],
        &[
            ("name", nullable("string")),
            ("currency", nullable("string")),
            ("change", num()),
            ("change_percent", num()),
            ("open", num()),
            ("day_low", num()),
            ("day_high", num()),
            ("year_low", num()),
            ("year_high", num()),
            ("volume", nullable("integer")),
            ("avg_volume_3m", nullable("integer")),
            ("market_cap", num()),
            ("pe_ttm", num()),
            ("eps_ttm", num()),
            ("dividend_yield", num()),
        ],
    )
}

fn pillar() -> Value {
    obj(
        &[
            ("name", string()),
            ("score", num()),
            ("weight", num()),
            ("band", one_of(&["strong", "moderate", "weak", "no_data"])),
            ("inputs_present", int()),
            ("inputs_possible", int()),
            ("summary", string()),
        ],
        &[],
    )
}

fn verdict() -> Value {
    obj(
        &[
            ("symbol", string()),
            ("name", string()),
            ("price", num()),
            ("currency", string()),
            ("overall_score", num()),
            ("data_coverage", num()),
            (
                "pillar_bands",
                obj(
                    &[
                        ("strong", int()),
                        ("moderate", int()),
                        ("weak", int()),
                        ("no_data", int()),
                    ],
                    &[],
                ),
            ),
            ("pillars", arr(pillar())),
            ("bull_points", strings()),
            ("bear_points", strings()),
            ("warnings", strings()),
            ("disclaimer", string()),
        ],
        &[
            ("note", nullable("string")),
            ("next_catalyst", nullable("string")),
            ("profile_notes", strings()),
        ],
    )
}

/// One row of the batch `verdicts` tool.
fn verdict_row() -> Value {
    obj(
        &[
            ("symbol", string()),
            ("overall_score", num()),
            ("data_coverage", num()),
            ("low_coverage", boolean()),
            (
                "pillars",
                arr(obj(
                    &[
                        ("name", string()),
                        ("score", num()),
                        ("band", one_of(&["strong", "moderate", "weak", "no_data"])),
                    ],
                    &[],
                )),
            ),
        ],
        &[
            ("name", string()),
            ("price", num()),
            ("currency", string()),
            ("warnings", strings()),
        ],
    )
}

fn pick() -> Value {
    obj(
        &[("symbol", string()), ("score", num()), ("coverage", num())],
        &[
            ("name", nullable("string")),
            ("price", num()),
            ("change_percent", num()),
            ("currency", nullable("string")),
            ("sector", nullable("string")),
            ("market_cap", num()),
            ("value", num()),
            ("quality", num()),
            ("growth", num()),
            ("trend", num()),
            ("risk", num()),
            ("flags", strings()),
        ],
    )
}

fn review() -> Value {
    obj(
        &[
            ("since", string()),
            (
                "rows",
                arr(obj(
                    &[
                        ("symbol", string()),
                        ("picked_on", string()),
                        ("days", int()),
                        ("rank", int()),
                        ("score", num()),
                        ("entry_price", num()),
                        ("price", num()),
                        ("return_pct", num()),
                        ("benchmark_pct", num()),
                        ("excess_pct", num()),
                    ],
                    &[("currency", nullable("string"))],
                )),
            ),
            ("unpriced", strings()),
            ("methodologies", strings()),
            ("hit_rate", num()),
            ("beat_rate", num()),
            ("mean_return", num()),
            ("median_return", num()),
            ("mean_excess", num()),
            ("median_excess", num()),
        ],
        &[
            ("benchmark", nullable("string")),
            ("market", string()),
            ("horizon", one_of(&["long", "short"])),
        ],
    )
}

fn holding() -> Value {
    obj(
        &[("symbol", string()), ("shares", num())],
        &[
            ("avg_cost", num()),
            ("cost_basis", num()),
            ("price", num()),
            ("value", num()),
            ("pnl", num()),
            ("pnl_percent", num()),
            ("weight_percent", num()),
            ("xirr_percent", num()),
            ("currency", nullable("string")),
            ("name", nullable("string")),
            ("nav_date", nullable("string")),
            ("cap_tier", string()),
        ],
    )
}

/// The schema of a tool's `data`, for the tools that publish one.
pub fn data_schema(tool: &str) -> Option<Value> {
    Some(match tool {
        "quote" => obj(&[("quotes", arr(quote_row())), ("missing", strings())], &[]),
        "history" => obj(
            &[
                ("symbol", string()),
                ("period", string()),
                ("interval", string()),
                ("bar_count", int()),
                ("bars_shown", int()),
                (
                    "bars",
                    arr(obj(
                        &[],
                        &[
                            ("time", nullable("string")),
                            ("open", num()),
                            ("high", num()),
                            ("low", num()),
                            ("close", num()),
                            ("volume", nullable("integer")),
                        ],
                    )),
                ),
            ],
            &[
                ("currency", nullable("string")),
                ("first_close", num()),
                ("last_close", num()),
                ("change_percent", num()),
                ("period_high", num()),
                ("period_low", num()),
            ],
        ),
        "verdict" => verdict(),
        "verdicts" => obj(
            &[
                ("verdicts", arr(verdict_row())),
                (
                    "failed",
                    arr(obj(&[("symbol", string()), ("error", string())], &[])),
                ),
                ("disclaimer", string()),
            ],
            &[],
        ),
        "picks" => obj(
            &[
                ("scanned", int()),
                ("below_floor", int()),
                ("thin_data", int()),
                (
                    "flagged_check_data",
                    arr(obj(&[("symbol", string()), ("flags", strings())], &[])),
                ),
                ("picks", arr(pick())),
                ("methodology", string()),
                ("horizon", one_of(&["long", "short"])),
                ("method", string()),
            ],
            &[],
        ),
        "picks_review" => review(),
        "portfolio_summary" => obj(
            &[
                ("holdings", arr(holding())),
                (
                    "totals",
                    obj(
                        &[("value", num()), ("cost_basis", num()), ("pnl", num())],
                        &[("pnl_percent", num()), ("currency", nullable("string"))],
                    ),
                ),
                ("unpriced", strings()),
            ],
            &[("unconverted", strings()), ("notes", strings())],
        ),
        // `AnalyzeReport` flattens its `Analysis` into the top level.
        "portfolio_analyze" => obj(
            &[
                ("base_currency", string()),
                ("value", num()),
                ("cost", num()),
                ("pnl", num()),
                ("positions", arr(object())),
                ("disclaimer", string()),
            ],
            &[
                ("pnl_pct", num()),
                ("day_change", num()),
                ("day_change_pct", num()),
                ("by_market", arr(object())),
                ("by_currency", arr(object())),
                ("by_cap_tier", arr(object())),
                ("by_sector", arr(object())),
                ("terms", object()),
                ("concentration", object()),
                ("risk", object()),
                ("observations", arr(object())),
                ("left_out", strings()),
                ("valuation", object()),
                ("tax_if_sold_today", nullable("object")),
                ("warnings", strings()),
            ],
        ),
        "portfolio_checkup" => obj(
            &[
                ("base_currency", string()),
                ("total_value", num()),
                (
                    "findings",
                    arr(obj(
                        &[("observed", string()), ("inspect", string())],
                        &[
                            ("symbol", nullable("string")),
                            ("exposure", num()),
                            ("at_stake", num()),
                            ("weight_pct", num()),
                            ("impact", num()),
                        ],
                    )),
                ),
                ("checks", arr(object())),
                ("clean", int()),
                ("evaluated", int()),
                ("skipped", strings()),
                ("disclaimer", string()),
            ],
            &[("herfindahl", num()), ("effective_bets", num())],
        ),
        "rebalance" => obj(
            &[
                ("total_value", num()),
                ("currency", string()),
                (
                    "items",
                    arr(obj(
                        &[
                            ("symbol", string()),
                            ("current_pct", num()),
                            ("target_pct", num()),
                            ("diff_value", num()),
                        ],
                        &[
                            ("current_value", num()),
                            ("projected_value", num()),
                            ("projected_pct", num()),
                            ("price", num()),
                            ("diff_shares", num()),
                            ("action_display", string()),
                            ("is_fund", boolean()),
                            ("sector", nullable("string")),
                        ],
                    )),
                ),
            ],
            &[
                // Left out when empty.
                ("warnings", strings()),
                ("projected_value", num()),
                ("inflow", num()),
                ("drift_tolerance", num()),
            ],
        ),
        "tax_estimate" => obj(
            &[
                ("country", string()),
                ("tax_year", string()),
                ("currency", string()),
                ("sales", arr(object())),
                ("holdings", arr(object())),
                ("realized", object()),
                ("if_sold_today", object()),
                ("unpriced", strings()),
            ],
            &[("ltcg_exemption_left", num()), ("intraday_gain", num())],
        ),
        "market_conditions" => obj(
            &[
                ("market", string()),
                ("index", string()),
                ("breadth", object()),
                ("valuation", object()),
                ("scanned", int()),
            ],
            &[
                ("trend", nullable("object")),
                ("volatility", nullable("object")),
                ("as_of", nullable("string")),
                ("history", nullable("object")),
            ],
        ),
        "whats_hot" => {
            let mover = obj(
                &[("symbol", string()), ("note", string())],
                &[
                    ("change_pct", num()),
                    ("price", num()),
                    ("rvol", num()),
                    ("gap_pct", num()),
                ],
            );
            let group = obj(
                &[("name", string()), ("priced", int()), ("members", int())],
                &[("change_pct", num())],
            );
            obj(
                &[
                    ("as_of", int()),
                    ("market", string()),
                    (
                        "yours",
                        arr(obj(
                            &[
                                ("symbol", string()),
                                ("held", boolean()),
                                ("weight", num()),
                                ("why", string()),
                                ("heat", obj(&[("score", num()), ("raw", num())], &[])),
                            ],
                            &[("change_pct", num()), ("sigma", num()), ("volume_x", num())],
                        )),
                    ),
                    ("top_movers", arr(mover.clone())),
                    ("unusual_volume", arr(mover.clone())),
                    ("gaps", arr(mover.clone())),
                    ("new_highs", arr(mover.clone())),
                    ("new_lows", arr(mover.clone())),
                    ("sectors", arr(group.clone())),
                    ("themes", arr(group)),
                    ("cross_market", arr(mover)),
                    ("warnings", strings()),
                ],
                &[("flows", nullable("object")), ("fo_ban", strings())],
            )
        }
        "today" => obj(
            &[
                (
                    "pulse",
                    obj(
                        &[
                            ("as_of", int()),
                            ("base_currency", string()),
                            (
                                "rows",
                                arr(obj(
                                    &[
                                        ("symbol", string()),
                                        ("held", boolean()),
                                        ("weight", num()),
                                        ("why", object()),
                                    ],
                                    &[
                                        ("change_pct", num()),
                                        ("day_pnl", num()),
                                        ("sigma", num()),
                                        ("volume_x", num()),
                                    ],
                                )),
                            ),
                            ("backdrop", arr(object())),
                        ],
                        &[("value", num()), ("day_pnl", num()), ("day_pnl_pct", num())],
                    ),
                ),
                ("feed", arr(news_item())),
            ],
            &[],
        ),
        "news_feed" => obj(
            &[
                ("days", int()),
                ("symbols", strings()),
                ("items", arr(news_item())),
                ("failures", strings()),
            ],
            &[],
        ),
        "digest" => obj(
            &[
                (
                    "digest",
                    obj(
                        &[
                            ("date", string()),
                            ("base_currency", string()),
                            ("up", arr(object())),
                            ("down", arr(object())),
                            ("signals", arr(object())),
                            ("alerts", strings()),
                            ("upcoming", strings()),
                        ],
                        &[("day_pnl", num()), ("day_pnl_pct", num()), ("value", num())],
                    ),
                ),
                ("text", string()),
            ],
            &[],
        ),
        _ => return None,
    })
}

/// One headline or filing (`news::NewsItem`).
fn news_item() -> Value {
    obj(
        &[
            ("title", string()),
            ("publisher", string()),
            ("source", string()),
            ("official", boolean()),
            ("importance", num()),
        ],
        &[
            ("symbol", nullable("string")),
            ("link", nullable("string")),
            ("ts", nullable("integer")),
            ("kind", string()),
        ],
    )
}

/// The whole `outputSchema` of a tool: the envelope every tool returns
/// (`data`, the note, and `truncated` when lists were cut) around its data.
pub fn output_schema(tool: &str) -> Option<Value> {
    let data = data_schema(tool)?;
    Some(json!({
        "type": "object",
        "properties": {
            "data": data,
            "truncated": { "type": "string" },
            "note": { "type": "string" },
        },
        "required": ["data", "note"],
    }))
}

/// Check `value` against the subset of JSON Schema the builders above
/// emit: `type` (one or several), `properties`, `required`, `items` and
/// `enum`. Returns the first mismatch with its JSON path.
#[cfg(test)]
pub fn validate(schema: &Value, value: &Value) -> Result<(), String> {
    check(schema, value, "$")
}

#[cfg(test)]
fn check(schema: &Value, value: &Value, path: &str) -> Result<(), String> {
    let type_ok = |kind: &str| match kind {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "number" => value.is_number(),
        "integer" => value.is_i64() || value.is_u64(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        _ => false,
    };
    match schema.get("type") {
        Some(Value::String(k)) if !type_ok(k) => {
            return Err(format!("{path}: expected {k}, got {value}"));
        }
        Some(Value::Array(ks)) if !ks.iter().filter_map(Value::as_str).any(type_ok) => {
            return Err(format!("{path}: expected one of {ks:?}, got {value}"));
        }
        _ => {}
    }
    if let Some(Value::Array(allowed)) = schema.get("enum")
        && !allowed.contains(value)
    {
        return Err(format!("{path}: {value} is not one of {allowed:?}"));
    }
    if let Value::Object(o) = value {
        if let Some(Value::Array(req)) = schema.get("required") {
            for k in req.iter().filter_map(Value::as_str) {
                if !o.contains_key(k) {
                    return Err(format!("{path}: missing required field '{k}'"));
                }
            }
        }
        if let Some(Value::Object(props)) = schema.get("properties") {
            for (k, s) in props {
                if let Some(v) = o.get(k) {
                    check(s, v, &format!("{path}.{k}"))?;
                }
            }
        }
    }
    if let (Value::Array(items), Some(s)) = (value, schema.get("items")) {
        for (i, v) in items.iter().enumerate() {
            check(s, v, &format!("{path}[{i}]"))?;
        }
    }
    Ok(())
}
