//! `cache warm|snapshots`, `worker` and `tasks`: the background side of
//! `cache.db`.

use super::*;
use crate::db::{CapTier, SnapshotFilter, Task};
use crate::worker::{self, TaskKind};

fn need_store<T>(v: Option<T>) -> Result<T> {
    v.context("The cache database (cache.db) is off or cannot be opened")
}

fn age(secs: i64) -> String {
    let secs = (chrono::Utc::now().timestamp() - secs).max(0);
    match secs {
        0..=59 => format!("{secs}s ago"),
        60..=3599 => format!("{}m ago", secs / 60),
        3600..=86399 => format!("{}h ago", secs / 3600),
        _ => format!("{}d ago", secs / 86400),
    }
}

fn task_json(t: &Task) -> serde_json::Value {
    let parse = |s: &Option<String>| {
        s.as_deref()
            .map(|v| serde_json::from_str(v).unwrap_or(serde_json::Value::String(v.into())))
    };
    serde_json::json!({
        "id": t.id,
        "kind": t.kind,
        "status": t.status,
        "progress_pct": t.progress_pct,
        "title": t.title,
        "payload": serde_json::from_str::<serde_json::Value>(&t.payload_json).ok(),
        "result": parse(&t.result_json),
        "error": t.error,
        "created_at": t.created_at,
        "updated_at": t.updated_at,
    })
}

// ── cache warm / snapshots ──

/// `cache warm`: refresh the market universe plus tracked symbols into
/// `market_snapshots`, now or as a queued task.
pub async fn cmd_cache_warm(market: Market, background: bool) -> Result<()> {
    if background {
        let id = worker::enqueue(TaskKind::Warm, market, &[])?;
        println!(
            "\n  {} Queued task #{id}. A running worker (`stockwise worker start`) picks it up; see `stockwise tasks`.\n",
            "✓".green()
        );
        return Ok(());
    }
    print_header(&format!("Warming {} snapshots", market.label()));
    let client = YahooClient::new().await?;
    let symbols = worker::warm_symbols(market);
    let total = symbols.len();
    let n = worker::refresh_snapshots(&client, market, &symbols, |pct| {
        eprint!("\r  {pct:>3.0}%");
        true
    })
    .await?;
    eprintln!();
    println!(
        "  {} Stored {n} of {total} symbols in cache.db.\n",
        "✓".green()
    );
    Ok(())
}

pub struct SnapshotQuery {
    pub market: Option<Market>,
    pub cap: Option<CapTier>,
    pub sector: Option<String>,
    pub min_pe: Option<f64>,
    pub max_pe: Option<f64>,
    pub min_roe: Option<f64>,
    pub min_div_yield: Option<f64>,
    pub min_rvol: Option<f64>,
    pub sort: Option<String>,
    pub ascending: bool,
    pub limit: usize,
}

/// `cache snapshots`: query `market_snapshots` with SQL, offline.
pub fn cmd_cache_snapshots(q: SnapshotQuery, json: bool) -> Result<()> {
    let filter = SnapshotFilter {
        market: q.market.map(crate::value_name),
        cap_tier: q.cap,
        sector: q.sector,
        min_pe: q.min_pe,
        max_pe: q.max_pe,
        min_roe: q.min_roe,
        min_div_yield: q.min_div_yield,
        min_rvol: q.min_rvol,
        updated_since: None,
        sort_by: q.sort,
        descending: !q.ascending,
        limit: Some(q.limit),
    };
    let rows = need_store(crate::db::with_db(|d| d.query_snapshots(&filter)))?;
    if json {
        let v: Vec<_> = rows
            .iter()
            .map(|r| {
                serde_json::json!({
                    "symbol": r.symbol, "market": r.market, "name": r.name,
                    "sector": r.sector, "industry": r.industry, "cap_tier": r.cap_tier,
                    "mcap": r.mcap, "price": r.price, "change_1d": r.change_1d,
                    "rvol": r.rvol, "pe": r.pe, "roe": r.roe, "div_yield": r.div_yield,
                    "updated_at": r.updated_at,
                })
            })
            .collect();
        return output::print_json(&v);
    }
    print_header("Cached market snapshots");
    if rows.is_empty() {
        println!(
            "  {}\n",
            "No snapshots match. Run `stockwise cache warm` to fill them.".dimmed()
        );
        return Ok(());
    }
    println!(
        "  {:<14} {:<6} {:>10} {:>8} {:>7} {:>7} {:>6}  {}",
        "Symbol".bold(),
        "Cap".bold(),
        "Price".bold(),
        "1d %".bold(),
        "P/E".bold(),
        "RVOL".bold(),
        "ROE %".bold(),
        "Updated".bold()
    );
    let f = |v: Option<f64>, p: usize| v.map_or("—".to_string(), |v| format!("{v:.p$}"));
    for r in &rows {
        println!(
            "  {:<14} {:<6} {:>10} {:>8} {:>7} {:>7} {:>6}  {}",
            r.symbol.cyan(),
            r.cap_tier.as_deref().unwrap_or("—"),
            f(r.price, 2),
            f(r.change_1d, 2),
            f(r.pe, 1),
            f(r.rvol, 1),
            f(r.roe, 1),
            age(r.updated_at).dimmed()
        );
    }
    println!();
    Ok(())
}

// ── worker ──

pub async fn cmd_worker(action: &str, market: Market) -> Result<()> {
    match action {
        "start" => {
            let pid = worker::start(market)?;
            println!("\n  {} Worker started (pid {pid}).", "✓".green());
            println!(
                "  {}\n",
                "Refreshes held and watched quotes while the market is open, runs queued tasks, checks alerts and syncs after the close."
                    .dimmed()
            );
        }
        "stop" => {
            if worker::stop().await? {
                println!("\n  {} Worker stopped.\n", "✓".green());
            } else {
                println!("\n  {}\n", "No worker is running.".dimmed());
            }
        }
        "run" => worker::run_worker(market).await?,
        _ => {
            let s = worker::status();
            print_header("Worker");
            match s.pid {
                Some(pid) => print_kv("State", &format!("{} (pid {pid})", "running".green())),
                None => print_kv("State", &"not running".dimmed().to_string()),
            }
            print_kv("Tasks queued", &s.queued.to_string());
            print_kv("Tasks running", &s.running.to_string());
            print_kv(
                "Last end-of-day sync",
                s.last_eod.as_deref().unwrap_or("never"),
            );
            println!();
        }
    }
    Ok(())
}

// ── tasks ──

fn status_cell(t: &Task) -> String {
    match t.status.as_str() {
        "done" => "done".green().to_string(),
        "failed" => "failed".red().to_string(),
        "running" => format!("{:.0}%", t.progress_pct).yellow().to_string(),
        s => s.dimmed().to_string(),
    }
}

pub fn cmd_tasks_list(limit: usize, json: bool) -> Result<()> {
    let tasks = need_store(crate::db::with_db(|d| d.list_tasks(limit)))?;
    if json {
        return output::print_json(&tasks.iter().map(task_json).collect::<Vec<_>>());
    }
    print_header("Background tasks");
    if tasks.is_empty() {
        println!(
            "  {}\n",
            "No tasks. Queue one with `stockwise tasks add warm`.".dimmed()
        );
        return Ok(());
    }
    for t in &tasks {
        println!(
            "  #{:<4} {:<8} {:<9} {}  {}",
            t.id,
            t.kind,
            status_cell(t),
            t.title,
            age(t.updated_at).dimmed()
        );
    }
    println!();
    Ok(())
}

pub fn cmd_tasks_show(id: i64, json: bool) -> Result<()> {
    let task = need_store(crate::db::with_db(|d| d.get_task(id)))?
        .with_context(|| format!("No task #{id} (see `stockwise tasks`)"))?;
    if json {
        return output::print_json(&task_json(&task));
    }
    print_header(&format!("Task #{}", task.id));
    print_kv("Kind", &task.kind);
    print_kv("Title", &task.title);
    print_kv("Status", &status_cell(&task));
    print_kv("Created", &age(task.created_at));
    print_kv("Updated", &age(task.updated_at));
    if let Some(e) = &task.error {
        print_kv("Error", &e.red().to_string());
    }
    if let Some(r) = &task.result_json {
        println!("\n{}", r);
    }
    println!();
    Ok(())
}

pub fn cmd_tasks_cancel(id: i64) -> Result<()> {
    let ok = need_store(crate::db::with_db(|d| d.cancel_task(id)))?;
    if ok {
        println!("\n  {} Task #{id} cancelled.\n", "✓".green());
        Ok(())
    } else {
        bail!("Task #{id} does not exist or already finished")
    }
}

pub fn cmd_tasks_add(kind: TaskKind, args: &[String], market: Market) -> Result<()> {
    let id = worker::enqueue(kind, market, args)?;
    println!(
        "\n  {} Queued task #{id}. Run `stockwise worker start` to execute it, then `stockwise tasks show {id}`.\n",
        "✓".green()
    );
    Ok(())
}
