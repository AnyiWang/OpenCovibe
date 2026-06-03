use crate::models::{DailyAggregate, ModelAggregate, RunUsageSummary, UsageOverview};
use crate::storage;
use crate::storage::changelog::ChangelogEntry;
use std::collections::{BTreeMap, HashMap};

/// Parse a started_at timestamp to a UTC NaiveDate.
/// Handles RFC 3339 with timezone, or legacy "YYYY-MM-DD" (no time).
fn parse_started_date_utc(started_at: &str) -> Option<chrono::NaiveDate> {
    chrono::DateTime::parse_from_rfc3339(started_at)
        .ok()
        .map(|dt| dt.with_timezone(&chrono::Utc).date_naive())
        .or_else(|| {
            started_at
                .get(..10)
                .and_then(|s| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
        })
}

#[tauri::command]
pub fn get_global_usage_overview(days: Option<u32>) -> Result<UsageOverview, String> {
    log::debug!("[stats] get_global_usage_overview: days={:?}", days);
    let claude = storage::claude_usage::read_global_usage(days)?;
    // Codex sessions live in ~/.codex/sessions (parallel to ~/.claude/projects). Merge so
    // Global covers both agents. Codex failures degrade gracefully to Claude-only.
    match storage::codex_usage::read_global_codex_usage(days) {
        Ok(codex) => Ok(merge_overviews(claude, codex)),
        Err(e) => {
            log::warn!("[stats] codex global usage failed: {}, claude-only", e);
            Ok(claude)
        }
    }
}

/// Merge two global UsageOverviews (Claude + Codex) into one. Totals add; by-model and
/// daily aggregate by key. Activity/streaks keep Claude's (primary) — Codex-only-active
/// days are not yet folded into the streak count.
fn merge_overviews(a: UsageOverview, b: UsageOverview) -> UsageOverview {
    let total_cost = a.total_cost_usd + b.total_cost_usd;
    let total_tokens = a.total_tokens + b.total_tokens;
    let total_runs = a.total_runs + b.total_runs;

    // by_model: aggregate by model name (Claude/Codex names are distinct in practice).
    let mut model_map: HashMap<String, ModelAggregate> = HashMap::new();
    for m in a.by_model.into_iter().chain(b.by_model) {
        let e = model_map
            .entry(m.model.clone())
            .or_insert_with(|| ModelAggregate {
                model: m.model.clone(),
                runs: 0,
                input_tokens: 0,
                output_tokens: 0,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                cost_usd: 0.0,
                pct: 0.0,
            });
        e.runs += m.runs;
        e.input_tokens += m.input_tokens;
        e.output_tokens += m.output_tokens;
        e.cache_read_tokens += m.cache_read_tokens;
        e.cache_write_tokens += m.cache_write_tokens;
        e.cost_usd += m.cost_usd;
    }
    let mut by_model: Vec<ModelAggregate> = model_map.into_values().collect();
    for m in &mut by_model {
        m.pct = if total_cost > 0.0 {
            m.cost_usd / total_cost * 100.0
        } else {
            0.0
        };
    }
    by_model.sort_by(|x, y| {
        y.cost_usd
            .partial_cmp(&x.cost_usd)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // daily: merge by date. Keep Claude's message/session/tool counts + model_breakdown
    // (Codex daily has none); sum cost/tokens so the daily-trend chart covers both.
    let mut day_map: HashMap<String, DailyAggregate> = HashMap::new();
    for d in a.daily.into_iter().chain(b.daily) {
        match day_map.get_mut(&d.date) {
            Some(e) => {
                e.cost_usd += d.cost_usd;
                e.runs += d.runs;
                e.input_tokens += d.input_tokens;
                e.output_tokens += d.output_tokens;
                if e.message_count.is_none() {
                    e.message_count = d.message_count;
                }
                if e.session_count.is_none() {
                    e.session_count = d.session_count;
                }
                if e.tool_call_count.is_none() {
                    e.tool_call_count = d.tool_call_count;
                }
                if e.model_breakdown.is_none() {
                    e.model_breakdown = d.model_breakdown;
                }
            }
            None => {
                day_map.insert(d.date.clone(), d);
            }
        }
    }
    let mut daily: Vec<DailyAggregate> = day_map.into_values().collect();
    daily.sort_by(|x, y| x.date.cmp(&y.date));

    UsageOverview {
        total_cost_usd: total_cost,
        total_tokens,
        total_runs,
        avg_cost_per_run: if total_runs > 0 {
            total_cost / total_runs as f64
        } else {
            0.0
        },
        by_model,
        daily,
        runs: a.runs,
        scan_mode: a.scan_mode,
        active_days: a.active_days,
        current_streak: a.current_streak,
        longest_streak: a.longest_streak,
    }
}

/// Per-model aggregate builder (internal, not serialized).
#[derive(Default)]
struct ModelAggBuilder {
    runs: u32,
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    cache_write_tokens: u64,
    cost_usd: f64,
}

/// Daily aggregate builder (internal, not serialized).
#[derive(Default)]
struct DailyBuilder {
    cost_usd: f64,
    runs: u32,
    input_tokens: u64,
    output_tokens: u64,
}

#[tauri::command]
pub fn get_usage_overview(days: Option<u32>) -> Result<UsageOverview, String> {
    log::debug!("[stats] get_usage_overview: days={:?}", days);

    let metas = storage::runs::list_all_run_metas();
    let cutoff_date = days.map(|d| {
        chrono::Utc::now().date_naive() - chrono::Duration::days(d.saturating_sub(1) as i64)
    });

    let mut run_summaries: Vec<RunUsageSummary> = Vec::new();
    let mut total_cost = 0.0f64;
    let mut total_tokens = 0u64;
    let mut model_map: HashMap<String, ModelAggBuilder> = HashMap::new();
    let mut daily_map: BTreeMap<String, DailyBuilder> = BTreeMap::new();

    for meta in &metas {
        let Some(started_date) = parse_started_date_utc(&meta.started_at) else {
            log::debug!(
                "[stats] skip run {}: bad started_at {:?}",
                meta.id,
                meta.started_at
            );
            continue;
        };

        if let Some(cutoff) = cutoff_date {
            if started_date < cutoff {
                continue;
            }
        }

        // Extract usage from events.jsonl
        let usage = storage::events::extract_run_usage(&meta.id);

        let mut cost = usage.as_ref().map(|u| u.total_cost_usd).unwrap_or(0.0);
        // total_tokens = input + output (billable tokens only, not cache)
        let tokens = usage
            .as_ref()
            .map(|u| u.input_tokens + u.output_tokens)
            .unwrap_or(0);
        let mut cost_estimated = false;

        // Build per-model aggregates
        if let Some(ref u) = usage {
            if !u.model_usage.is_empty() {
                // Claude: CLI provides per-model breakdown
                for (model, mu) in &u.model_usage {
                    let agg = model_map.entry(model.clone()).or_default();
                    agg.runs += 1;
                    agg.input_tokens += mu.input_tokens;
                    agg.output_tokens += mu.output_tokens;
                    agg.cache_read_tokens += mu.cache_read_tokens;
                    agg.cache_write_tokens += mu.cache_write_tokens;
                    agg.cost_usd += mu.cost_usd;
                }
            } else if meta.agent == "codex"
                && (u.input_tokens > 0 || u.output_tokens > 0)
                && meta.model.is_some()
            {
                // Codex with known model: estimate cost from tokens + pricing table.
                // try_estimate_cost returns None for unknown models (e.g. gpt-oss-*)
                // so we don't produce wrong estimates via the Sonnet fallback.
                let model_name = meta.model.as_deref().unwrap();
                let estimated = crate::pricing::try_estimate_cost(
                    model_name,
                    u.input_tokens,
                    u.output_tokens,
                    u.cache_read_tokens,
                    u.cache_write_tokens,
                );
                if let Some(est) = estimated {
                    if cost < 0.000001 && est > 0.0 {
                        cost = est;
                        cost_estimated = true;
                    }
                    // Synthesize single-model entry for by-model table
                    let agg = model_map.entry(model_name.to_string()).or_default();
                    agg.runs += 1;
                    agg.input_tokens += u.input_tokens;
                    agg.output_tokens += u.output_tokens;
                    agg.cache_read_tokens += u.cache_read_tokens;
                    agg.cache_write_tokens += u.cache_write_tokens;
                    agg.cost_usd += est;
                } else {
                    // Unknown model: still show in by-model table but with $0 cost
                    let agg = model_map.entry(model_name.to_string()).or_default();
                    agg.runs += 1;
                    agg.input_tokens += u.input_tokens;
                    agg.output_tokens += u.output_tokens;
                    agg.cache_read_tokens += u.cache_read_tokens;
                    agg.cache_write_tokens += u.cache_write_tokens;
                }
            }
        }

        total_cost += cost;
        total_tokens += tokens;

        // Build daily aggregates
        let date = started_date.format("%Y-%m-%d").to_string();
        let day = daily_map.entry(date).or_default();
        day.cost_usd += cost;
        day.runs += 1;
        day.input_tokens += usage.as_ref().map(|u| u.input_tokens).unwrap_or(0);
        day.output_tokens += usage.as_ref().map(|u| u.output_tokens).unwrap_or(0);

        // Build run summary (merge RunMeta + RawRunUsage)
        let name = meta.name.clone().unwrap_or_else(|| {
            if meta.prompt.chars().count() > 80 {
                meta.prompt.chars().take(80).collect::<String>() + "..."
            } else {
                meta.prompt.clone()
            }
        });

        run_summaries.push(RunUsageSummary {
            run_id: meta.id.clone(),
            name,
            agent: meta.agent.clone(),
            model: meta.model.clone(),
            status: meta.status.clone(),
            started_at: meta.started_at.clone(),
            ended_at: meta.ended_at.clone(),
            total_cost_usd: cost,
            input_tokens: usage.as_ref().map(|u| u.input_tokens).unwrap_or(0),
            output_tokens: usage.as_ref().map(|u| u.output_tokens).unwrap_or(0),
            cache_read_tokens: usage.as_ref().map(|u| u.cache_read_tokens).unwrap_or(0),
            cache_write_tokens: usage.as_ref().map(|u| u.cache_write_tokens).unwrap_or(0),
            duration_ms: usage.as_ref().map(|u| u.duration_ms).unwrap_or(0),
            num_turns: usage.as_ref().map(|u| u.num_turns).unwrap_or(0),
            model_usage: usage
                .as_ref()
                .map(|u| u.model_usage.clone())
                .unwrap_or_default(),
            cost_estimated,
        });
    }

    // Sort runs by date descending
    run_summaries.sort_by(|a, b| b.started_at.cmp(&a.started_at));

    let total_runs = run_summaries.len() as u32;
    let avg_cost = if total_runs > 0 {
        total_cost / total_runs as f64
    } else {
        0.0
    };

    // Build per-model aggregates with percentages, sorted by cost descending
    let mut by_model: Vec<ModelAggregate> = model_map
        .into_iter()
        .map(|(model, agg)| {
            let pct = if total_cost > 0.0 {
                agg.cost_usd / total_cost * 100.0
            } else {
                0.0
            };
            ModelAggregate {
                model,
                runs: agg.runs,
                input_tokens: agg.input_tokens,
                output_tokens: agg.output_tokens,
                cache_read_tokens: agg.cache_read_tokens,
                cache_write_tokens: agg.cache_write_tokens,
                cost_usd: agg.cost_usd,
                pct,
            }
        })
        .collect();
    by_model.sort_by(|a, b| {
        b.cost_usd
            .partial_cmp(&a.cost_usd)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // Build daily aggregates (BTreeMap → sorted by date ascending)
    let daily: Vec<DailyAggregate> = daily_map
        .into_iter()
        .map(|(date, d)| DailyAggregate {
            date,
            cost_usd: d.cost_usd,
            runs: d.runs,
            input_tokens: d.input_tokens,
            output_tokens: d.output_tokens,
            message_count: None,
            session_count: None,
            tool_call_count: None,
            model_breakdown: None,
        })
        .collect();

    log::debug!(
        "[stats] get_usage_overview: {} runs, ${:.4} total, {} models, {} days",
        total_runs,
        total_cost,
        by_model.len(),
        daily.len()
    );

    let (active_days, current_streak, longest_streak) =
        crate::storage::claude_usage::compute_streaks(&daily, chrono::Utc::now().date_naive());

    Ok(UsageOverview {
        total_cost_usd: total_cost,
        total_tokens,
        total_runs,
        avg_cost_per_run: avg_cost,
        by_model,
        daily,
        runs: run_summaries,
        scan_mode: None,
        active_days,
        current_streak,
        longest_streak,
    })
}

#[tauri::command]
pub fn clear_usage_cache() -> Result<(), String> {
    log::debug!("[stats] clear_usage_cache");
    storage::claude_usage::clear_cache();
    Ok(())
}

/// Lightweight daily builder for heatmap aggregation (app scope).
#[derive(Default)]
struct HeatmapDayBuilder {
    cost_usd: f64,
    runs: u32,
    input_tokens: u64,
    output_tokens: u64,
}

/// Strip model_breakdown, sort by date ascending, truncate to at most 365 entries.
fn prepare_heatmap_daily(mut daily: Vec<DailyAggregate>) -> Vec<DailyAggregate> {
    for d in &mut daily {
        d.model_breakdown = None;
    }
    daily.sort_by(|a, b| a.date.cmp(&b.date));
    if daily.len() > 365 {
        daily = daily.split_off(daily.len() - 365);
    }
    daily
}

fn get_app_heatmap_daily() -> Result<Vec<DailyAggregate>, String> {
    let metas = storage::runs::list_all_run_metas();
    let cutoff_date = chrono::Utc::now().date_naive() - chrono::Duration::days(364);
    let mut daily_map: BTreeMap<String, HeatmapDayBuilder> = BTreeMap::new();

    for meta in &metas {
        let Some(d) = parse_started_date_utc(&meta.started_at) else {
            log::debug!(
                "[stats] heatmap skip run {} bad timestamp {:?}",
                meta.id,
                meta.started_at
            );
            continue;
        };
        if d < cutoff_date {
            continue;
        }

        let date = d.format("%Y-%m-%d").to_string();
        let day = daily_map.entry(date).or_default();
        let usage = storage::events::extract_run_usage(&meta.id);
        day.cost_usd += usage.as_ref().map(|u| u.total_cost_usd).unwrap_or(0.0);
        day.runs += 1;
        day.input_tokens += usage.as_ref().map(|u| u.input_tokens).unwrap_or(0);
        day.output_tokens += usage.as_ref().map(|u| u.output_tokens).unwrap_or(0);
    }

    Ok(daily_map
        .into_iter()
        .map(|(date, d)| DailyAggregate {
            date,
            cost_usd: d.cost_usd,
            runs: d.runs,
            input_tokens: d.input_tokens,
            output_tokens: d.output_tokens,
            message_count: None,
            session_count: None,
            tool_call_count: None,
            model_breakdown: None,
        })
        .collect())
}

#[tauri::command]
pub fn get_heatmap_daily(scope: String) -> Result<Vec<DailyAggregate>, String> {
    log::debug!("[stats] get_heatmap_daily: scope={}", scope);
    let raw = match scope.as_str() {
        "global" => {
            // Merge Claude + Codex daily so the global heatmap reflects both agents.
            let overview = get_global_usage_overview(Some(365))?;
            overview.daily
        }
        "app" => get_app_heatmap_daily()?,
        _ => return Err(format!("invalid scope: {}", scope)),
    };
    Ok(prepare_heatmap_daily(raw))
}

#[tauri::command]
pub async fn get_changelog() -> Result<Vec<ChangelogEntry>, String> {
    log::debug!("[stats] get_changelog");
    storage::changelog::get_changelog().await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_started_date_utc_rfc3339() {
        let d = parse_started_date_utc("2026-02-25T10:30:00+08:00");
        assert_eq!(
            d,
            Some(chrono::NaiveDate::from_ymd_opt(2026, 2, 25).unwrap())
        );
    }

    #[test]
    fn test_parse_started_date_utc_cross_day_forward() {
        // +14:00 timezone, 00:30 local -> 2026-02-24 in UTC
        let d = parse_started_date_utc("2026-02-25T00:30:00+14:00");
        assert_eq!(
            d,
            Some(chrono::NaiveDate::from_ymd_opt(2026, 2, 24).unwrap())
        );
    }

    #[test]
    fn test_parse_started_date_utc_cross_day_negative() {
        // -12:00 timezone, 23:30 local -> 2026-02-26 in UTC
        let d = parse_started_date_utc("2026-02-25T23:30:00-12:00");
        assert_eq!(
            d,
            Some(chrono::NaiveDate::from_ymd_opt(2026, 2, 26).unwrap())
        );
    }

    #[test]
    fn test_parse_started_date_utc_legacy() {
        let d = parse_started_date_utc("2026-02-25");
        assert_eq!(
            d,
            Some(chrono::NaiveDate::from_ymd_opt(2026, 2, 25).unwrap())
        );
    }

    #[test]
    fn test_parse_started_date_utc_invalid() {
        assert_eq!(parse_started_date_utc("bad"), None);
    }

    #[test]
    fn test_prepare_heatmap_max_365() {
        let mut daily = Vec::new();
        for i in 0..400 {
            daily.push(DailyAggregate {
                date: format!("2025-{:02}-{:02}", (i / 28) % 12 + 1, i % 28 + 1),
                cost_usd: 0.0,
                runs: 1,
                input_tokens: 0,
                output_tokens: 0,
                message_count: None,
                session_count: None,
                tool_call_count: None,
                model_breakdown: None,
            });
        }
        let result = prepare_heatmap_daily(daily);
        assert_eq!(result.len(), 365);
    }

    #[test]
    fn test_prepare_heatmap_unsorted_input() {
        let daily = vec![
            DailyAggregate {
                date: "2026-02-03".to_string(),
                cost_usd: 0.0,
                runs: 1,
                input_tokens: 0,
                output_tokens: 0,
                message_count: None,
                session_count: None,
                tool_call_count: None,
                model_breakdown: None,
            },
            DailyAggregate {
                date: "2026-02-01".to_string(),
                cost_usd: 0.0,
                runs: 1,
                input_tokens: 0,
                output_tokens: 0,
                message_count: None,
                session_count: None,
                tool_call_count: None,
                model_breakdown: None,
            },
            DailyAggregate {
                date: "2026-02-02".to_string(),
                cost_usd: 0.0,
                runs: 1,
                input_tokens: 0,
                output_tokens: 0,
                message_count: None,
                session_count: None,
                tool_call_count: None,
                model_breakdown: None,
            },
        ];
        let result = prepare_heatmap_daily(daily);
        assert_eq!(result[0].date, "2026-02-01");
        assert_eq!(result[1].date, "2026-02-02");
        assert_eq!(result[2].date, "2026-02-03");
    }

    #[test]
    fn test_prepare_heatmap_strips_breakdown() {
        let daily = vec![DailyAggregate {
            date: "2026-02-01".to_string(),
            cost_usd: 0.0,
            runs: 1,
            input_tokens: 0,
            output_tokens: 0,
            message_count: None,
            session_count: None,
            tool_call_count: None,
            model_breakdown: Some(std::collections::HashMap::from([(
                "test".to_string(),
                crate::models::ModelTokens::default(),
            )])),
        }];
        let result = prepare_heatmap_daily(daily);
        assert!(result[0].model_breakdown.is_none());
    }

    #[test]
    fn test_heatmap_daily_invalid_scope() {
        let result = get_heatmap_daily("foo".to_string());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("invalid scope"));
    }
}
