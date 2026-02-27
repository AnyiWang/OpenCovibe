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
    storage::claude_usage::read_global_usage(days)
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

        let cost = usage.as_ref().map(|u| u.total_cost_usd).unwrap_or(0.0);
        // total_tokens = input + output (billable tokens only, not cache)
        let tokens = usage
            .as_ref()
            .map(|u| u.input_tokens + u.output_tokens)
            .unwrap_or(0);

        total_cost += cost;
        total_tokens += tokens;

        // Build per-model aggregates
        if let Some(ref u) = usage {
            for (model, mu) in &u.model_usage {
                let agg = model_map.entry(model.clone()).or_default();
                agg.runs += 1;
                agg.input_tokens += mu.input_tokens;
                agg.output_tokens += mu.output_tokens;
                agg.cache_read_tokens += mu.cache_read_tokens;
                agg.cache_write_tokens += mu.cache_write_tokens;
                agg.cost_usd += mu.cost_usd;
            }
        }

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
            let overview = storage::claude_usage::read_global_usage(Some(365))?;
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
