use crate::models::{
    FacetCount, RunSearchFacets, RunSearchFilters, RunSearchResponse, RunSearchResult,
};
use crate::storage::run_index::{self, RunIndexEntry};
use std::collections::HashMap;

#[tauri::command]
pub async fn search_runs(filters: RunSearchFilters) -> Result<RunSearchResponse, String> {
    log::debug!("[history] search_runs: {:?}", filters);

    let entries = tokio::task::spawn_blocking(run_index::build_or_update_index)
        .await
        .map_err(|e| format!("Join error: {}", e))??;

    // Compute facets from ALL entries (before filtering)
    let facets = compute_facets(&entries);

    // Apply filters
    let filtered = apply_filters(&entries, &filters);
    let total_matching = filtered.len();

    // Sort
    let mut sorted = filtered;
    sort_entries(&mut sorted, &filters);

    // Paginate
    let offset = filters.offset.unwrap_or(0);
    let limit = filters.limit.unwrap_or(50);
    let page: Vec<_> = sorted.into_iter().skip(offset).take(limit).collect();

    // Convert to RunSearchResult
    let results: Vec<RunSearchResult> = page.into_iter().map(entry_to_result).collect();

    log::debug!(
        "[history] search_runs: total={}, matching={}, returned={}",
        facets.total_runs,
        total_matching,
        results.len()
    );

    Ok(RunSearchResponse {
        results,
        facets,
        total_matching,
    })
}

#[tauri::command]
pub async fn get_run_files(run_id: String) -> Result<Vec<String>, String> {
    log::debug!("[history] get_run_files: {}", run_id);

    let entries = tokio::task::spawn_blocking(run_index::build_or_update_index)
        .await
        .map_err(|e| format!("Join error: {}", e))??;

    let entry = entries.iter().find(|e| e.run_id == run_id);
    match entry {
        Some(e) => Ok(e.files_touched.clone()),
        None => Err(format!("Run not found: {}", run_id)),
    }
}

// ── Helpers ──

/// Normalize a path for comparison: replace `\` with `/`, strip trailing `/`,
/// lowercase on Windows.
fn normalize_path(p: &str) -> String {
    let mut s = p.replace('\\', "/");
    // Strip trailing slash
    while s.ends_with('/') && s.len() > 1 {
        s.pop();
    }
    #[cfg(target_os = "windows")]
    {
        s = s.to_lowercase();
    }
    s
}

/// Check if `entry_path` belongs to `filter_project` using path boundary matching.
/// `/repo/a` matches `/repo/a` and `/repo/a/sub`, but NOT `/repo/ab`.
fn path_matches_project(entry_path: &str, filter_project: &str) -> bool {
    let norm_entry = normalize_path(entry_path);
    let norm_filter = normalize_path(filter_project);

    if norm_entry == norm_filter {
        return true;
    }
    // Must be a prefix followed by `/`
    norm_entry.starts_with(&format!("{}/", norm_filter))
}

fn apply_filters(entries: &[RunIndexEntry], filters: &RunSearchFilters) -> Vec<RunIndexEntry> {
    entries
        .iter()
        .filter(|e| {
            // Query: tokenized search — split by whitespace, ANY token matching
            // at least one field is enough (OR logic). This is more forgiving for
            // natural language queries like "哪些会话改过 auth.rs" where the
            // Chinese part won't match but "auth.rs" will.
            if let Some(ref q) = filters.query {
                let tokens: Vec<String> = q
                    .split_whitespace()
                    .map(|t| t.to_lowercase())
                    .filter(|t| !t.is_empty())
                    .collect();
                if !tokens.is_empty() {
                    let prompt_lower = e.prompt_preview.to_lowercase();
                    let name_lower = e
                        .name
                        .as_deref()
                        .map(|n| n.to_lowercase())
                        .unwrap_or_default();
                    let cwd_lower = e.cwd.to_lowercase();

                    let any_match = tokens.iter().any(|token| {
                        prompt_lower.contains(token.as_str())
                            || name_lower.contains(token.as_str())
                            || cwd_lower.contains(token.as_str())
                            || e.files_touched
                                .iter()
                                .any(|f| f.to_lowercase().contains(token.as_str()))
                    });
                    if !any_match {
                        return false;
                    }
                }
            }

            // Projects: path boundary match
            if let Some(ref projects) = filters.projects {
                if !projects.is_empty() {
                    let matches = projects.iter().any(|p| path_matches_project(&e.cwd, p));
                    if !matches {
                        return false;
                    }
                }
            }

            // Tools: intersection check
            if let Some(ref tools) = filters.tools {
                if !tools.is_empty() {
                    let matches = tools.iter().any(|t| e.tools_used.contains(t));
                    if !matches {
                        return false;
                    }
                }
            }

            // Date range: lexicographic comparison on started_at
            if let Some(ref from) = filters.date_from {
                if e.started_at < *from {
                    return false;
                }
            }
            if let Some(ref to) = filters.date_to {
                if e.started_at > *to {
                    return false;
                }
            }

            // Cost range
            if let Some(min) = filters.cost_min {
                if e.total_cost_usd < min {
                    return false;
                }
            }
            if let Some(max) = filters.cost_max {
                if e.total_cost_usd > max {
                    return false;
                }
            }

            // Statuses
            if let Some(ref statuses) = filters.statuses {
                if !statuses.is_empty() && !statuses.contains(&e.status) {
                    return false;
                }
            }

            // Has errors
            if let Some(has_errors) = filters.has_errors {
                if e.has_errors != has_errors {
                    return false;
                }
            }

            // Agents
            if let Some(ref agents) = filters.agents {
                if !agents.is_empty() && !agents.contains(&e.agent) {
                    return false;
                }
            }

            true
        })
        .cloned()
        .collect()
}

fn sort_entries(entries: &mut [RunIndexEntry], filters: &RunSearchFilters) {
    let sort_by = filters.sort_by.as_deref().unwrap_or("date");
    let ascending = filters.sort_asc.unwrap_or(false);

    entries.sort_by(|a, b| {
        let cmp = match sort_by {
            "cost" => a
                .total_cost_usd
                .partial_cmp(&b.total_cost_usd)
                .unwrap_or(std::cmp::Ordering::Equal),
            "tokens" => {
                let a_total = a.input_tokens + a.output_tokens;
                let b_total = b.input_tokens + b.output_tokens;
                a_total.cmp(&b_total)
            }
            "turns" => a.num_turns.cmp(&b.num_turns),
            // Default: date (started_at)
            _ => a.started_at.cmp(&b.started_at),
        };
        if ascending {
            cmp
        } else {
            cmp.reverse()
        }
    });
}

fn compute_facets(entries: &[RunIndexEntry]) -> RunSearchFacets {
    let mut project_counts: HashMap<String, usize> = HashMap::new();
    let mut tool_counts: HashMap<String, usize> = HashMap::new();
    let mut agent_counts: HashMap<String, usize> = HashMap::new();

    let mut min_cost = f64::MAX;
    let mut max_cost = f64::MIN;
    let mut earliest_date = String::new();
    let mut latest_date = String::new();

    let mut total_cost = 0.0_f64;

    for e in entries {
        // Projects: count by cwd
        *project_counts.entry(e.cwd.clone()).or_insert(0) += 1;

        // Tools: each tool in tools_used
        for tool in &e.tools_used {
            *tool_counts.entry(tool.clone()).or_insert(0) += 1;
        }

        // Agents
        *agent_counts.entry(e.agent.clone()).or_insert(0) += 1;

        // Cost range + total
        total_cost += e.total_cost_usd;
        if e.total_cost_usd < min_cost {
            min_cost = e.total_cost_usd;
        }
        if e.total_cost_usd > max_cost {
            max_cost = e.total_cost_usd;
        }

        // Date range
        if earliest_date.is_empty() || e.started_at < earliest_date {
            earliest_date = e.started_at.clone();
        }
        if latest_date.is_empty() || e.started_at > latest_date {
            latest_date = e.started_at.clone();
        }
    }

    // Handle empty entries
    if entries.is_empty() {
        min_cost = 0.0;
        max_cost = 0.0;
    }

    // Convert to sorted FacetCount vecs (sorted by count descending)
    let mut projects: Vec<FacetCount> = project_counts
        .into_iter()
        .map(|(value, count)| FacetCount { value, count })
        .collect();
    projects.sort_by_key(|x| std::cmp::Reverse(x.count));

    let mut tools: Vec<FacetCount> = tool_counts
        .into_iter()
        .map(|(value, count)| FacetCount { value, count })
        .collect();
    tools.sort_by_key(|x| std::cmp::Reverse(x.count));

    let mut agents: Vec<FacetCount> = agent_counts
        .into_iter()
        .map(|(value, count)| FacetCount { value, count })
        .collect();
    agents.sort_by_key(|x| std::cmp::Reverse(x.count));

    RunSearchFacets {
        projects,
        tools,
        agents,
        cost_range: [min_cost, max_cost],
        date_range: [earliest_date, latest_date],
        total_runs: entries.len(),
        total_cost,
    }
}

fn entry_to_result(entry: RunIndexEntry) -> RunSearchResult {
    RunSearchResult {
        run_id: entry.run_id,
        cwd: entry.cwd,
        agent: entry.agent,
        model: entry.model,
        status: entry.status,
        started_at: entry.started_at,
        ended_at: entry.ended_at,
        name: entry.name,
        prompt_preview: entry.prompt_preview,
        tools_used: entry.tools_used,
        tool_call_count: entry.tool_call_count,
        files_touched_count: entry.files_touched.len() as u32,
        total_cost_usd: entry.total_cost_usd,
        input_tokens: entry.input_tokens,
        output_tokens: entry.output_tokens,
        duration_ms: entry.duration_ms,
        num_turns: entry.num_turns,
        has_errors: entry.has_errors,
        error_summary: entry.error_summary,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::RunStatus;

    fn make_entry(run_id: &str, cwd: &str, agent: &str, prompt_preview: &str) -> RunIndexEntry {
        RunIndexEntry {
            run_id: run_id.to_string(),
            cwd: cwd.to_string(),
            agent: agent.to_string(),
            model: Some("claude-3-5-sonnet".to_string()),
            status: RunStatus::Completed,
            started_at: "2024-06-15T10:00:00.000Z".to_string(),
            ended_at: Some("2024-06-15T10:05:00.000Z".to_string()),
            name: None,
            prompt_preview: prompt_preview.to_string(),
            tools_used: vec!["Read".to_string(), "Write".to_string()],
            tool_call_count: 5,
            files_touched: vec!["/src/main.ts".to_string(), "/src/auth.ts".to_string()],
            total_cost_usd: 0.5,
            input_tokens: 1000,
            output_tokens: 500,
            duration_ms: 300000,
            num_turns: 3,
            error_summary: None,
            has_errors: false,
            permission_denied_count: 0,
        }
    }

    fn empty_filters() -> RunSearchFilters {
        RunSearchFilters {
            query: None,
            projects: None,
            tools: None,
            date_from: None,
            date_to: None,
            cost_min: None,
            cost_max: None,
            statuses: None,
            has_errors: None,
            agents: None,
            sort_by: None,
            sort_asc: None,
            limit: None,
            offset: None,
        }
    }

    #[test]
    fn test_filter_by_query() {
        let entries = vec![
            make_entry("r1", "/repo", "claude", "fix the login bug"),
            make_entry("r2", "/repo", "claude", "add dark mode"),
            make_entry("r3", "/repo", "claude", "refactor auth"),
        ];

        let filters = RunSearchFilters {
            query: Some("login".to_string()),
            ..empty_filters()
        };
        let result = apply_filters(&entries, &filters);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].run_id, "r1");
    }

    #[test]
    fn test_filter_by_query_matches_filename() {
        let mut entry = make_entry("r1", "/repo", "claude", "some prompt");
        entry.files_touched = vec!["/src/auth.ts".to_string()];

        let entries = vec![entry];
        let filters = RunSearchFilters {
            query: Some("auth.ts".to_string()),
            ..empty_filters()
        };
        let result = apply_filters(&entries, &filters);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].run_id, "r1");
    }

    #[test]
    fn test_filter_by_query_tokenized() {
        let mut e1 = make_entry("r1", "/repo", "claude", "fix auth bug");
        e1.files_touched = vec!["/src/auth.ts".to_string()];
        let mut e2 = make_entry("r2", "/repo", "claude", "add dark mode");
        e2.files_touched = vec!["/src/theme.ts".to_string()];
        let mut e3 = make_entry("r3", "/repo", "claude", "unrelated prompt");
        e3.files_touched = vec!["/src/index.ts".to_string()];

        let entries = vec![e1, e2, e3];

        // Single token "auth.ts" should match r1 only
        let filters = RunSearchFilters {
            query: Some("auth.ts".to_string()),
            ..empty_filters()
        };
        let result = apply_filters(&entries, &filters);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].run_id, "r1");

        // OR logic: "auth bug" — "auth" matches r1 (prompt+files), "bug" matches r1 (prompt)
        // Only r1 matches (no other entry has "auth" or "bug")
        let filters2 = RunSearchFilters {
            query: Some("auth bug".to_string()),
            ..empty_filters()
        };
        let result2 = apply_filters(&entries, &filters2);
        assert_eq!(result2.len(), 1);
        assert_eq!(result2[0].run_id, "r1");

        // Chinese + keyword: "哪些会话改过 auth.ts" (OR logic)
        // "哪些会话改过" matches nothing, but "auth.ts" matches r1 → r1 passes
        let filters3 = RunSearchFilters {
            query: Some("哪些会话改过 auth.ts".to_string()),
            ..empty_filters()
        };
        let result3 = apply_filters(&entries, &filters3);
        assert_eq!(result3.len(), 1);
        assert_eq!(result3[0].run_id, "r1");

        // "dark mode" matches r2's prompt
        let filters4 = RunSearchFilters {
            query: Some("dark mode".to_string()),
            ..empty_filters()
        };
        let result4 = apply_filters(&entries, &filters4);
        assert_eq!(result4.len(), 1);
        assert_eq!(result4[0].run_id, "r2");

        // No token matches anything → 0 results
        let filters5 = RunSearchFilters {
            query: Some("zzzzz xxxxx".to_string()),
            ..empty_filters()
        };
        let result5 = apply_filters(&entries, &filters5);
        assert_eq!(result5.len(), 0);
    }

    #[test]
    fn test_filter_by_project_boundary() {
        let entries = vec![
            make_entry("r1", "/repo/a", "claude", "in repo a"),
            make_entry("r2", "/repo/ab", "claude", "in repo ab"),
            make_entry("r3", "/repo/a/sub", "claude", "in repo a/sub"),
        ];

        let filters = RunSearchFilters {
            projects: Some(vec!["/repo/a".to_string()]),
            ..empty_filters()
        };
        let result = apply_filters(&entries, &filters);
        assert_eq!(result.len(), 2); // r1 (/repo/a exact) + r3 (/repo/a/sub)
        let ids: Vec<&str> = result.iter().map(|e| e.run_id.as_str()).collect();
        assert!(ids.contains(&"r1"));
        assert!(ids.contains(&"r3"));
        assert!(!ids.contains(&"r2")); // /repo/ab should NOT match
    }

    #[test]
    fn test_filter_by_project_cross_platform() {
        let entries = vec![make_entry(
            "r1",
            r"C:\Users\dev\repo\a",
            "claude",
            "win path",
        )];

        // Normalization: backslashes -> forward slashes
        let norm = normalize_path(r"C:\Users\dev\repo\a");
        assert_eq!(norm, "C:/Users/dev/repo/a");

        // Filter with forward slashes should match
        let filters = RunSearchFilters {
            projects: Some(vec![r"C:\Users\dev\repo\a".to_string()]),
            ..empty_filters()
        };
        let result = apply_filters(&entries, &filters);
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn test_filter_by_tools() {
        let mut e1 = make_entry("r1", "/repo", "claude", "p1");
        e1.tools_used = vec!["Read".to_string(), "Write".to_string()];
        let mut e2 = make_entry("r2", "/repo", "claude", "p2");
        e2.tools_used = vec!["Bash".to_string()];

        let entries = vec![e1, e2];
        let filters = RunSearchFilters {
            tools: Some(vec!["Write".to_string()]),
            ..empty_filters()
        };
        let result = apply_filters(&entries, &filters);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].run_id, "r1");
    }

    #[test]
    fn test_filter_by_date_range() {
        let mut e1 = make_entry("r1", "/repo", "claude", "early");
        e1.started_at = "2024-01-01T00:00:00.000Z".to_string();
        let mut e2 = make_entry("r2", "/repo", "claude", "mid");
        e2.started_at = "2024-06-15T00:00:00.000Z".to_string();
        let mut e3 = make_entry("r3", "/repo", "claude", "late");
        e3.started_at = "2024-12-31T00:00:00.000Z".to_string();

        let entries = vec![e1, e2, e3];
        let filters = RunSearchFilters {
            date_from: Some("2024-03-01T00:00:00.000Z".to_string()),
            date_to: Some("2024-09-01T00:00:00.000Z".to_string()),
            ..empty_filters()
        };
        let result = apply_filters(&entries, &filters);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].run_id, "r2");
    }

    #[test]
    fn test_filter_by_cost_range() {
        let mut e1 = make_entry("r1", "/repo", "claude", "cheap");
        e1.total_cost_usd = 0.1;
        let mut e2 = make_entry("r2", "/repo", "claude", "medium");
        e2.total_cost_usd = 0.5;
        let mut e3 = make_entry("r3", "/repo", "claude", "expensive");
        e3.total_cost_usd = 2.0;

        let entries = vec![e1, e2, e3];
        let filters = RunSearchFilters {
            cost_min: Some(0.3),
            cost_max: Some(1.0),
            ..empty_filters()
        };
        let result = apply_filters(&entries, &filters);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].run_id, "r2");
    }

    #[test]
    fn test_filter_by_status() {
        let mut e1 = make_entry("r1", "/repo", "claude", "completed");
        e1.status = RunStatus::Completed;
        let mut e2 = make_entry("r2", "/repo", "claude", "failed");
        e2.status = RunStatus::Failed;
        let mut e3 = make_entry("r3", "/repo", "claude", "running");
        e3.status = RunStatus::Running;

        let entries = vec![e1, e2, e3];
        let filters = RunSearchFilters {
            statuses: Some(vec![RunStatus::Completed, RunStatus::Failed]),
            ..empty_filters()
        };
        let result = apply_filters(&entries, &filters);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_filter_combined() {
        let mut e1 = make_entry("r1", "/repo/a", "claude", "fix login");
        e1.total_cost_usd = 0.5;
        e1.status = RunStatus::Completed;
        let mut e2 = make_entry("r2", "/repo/b", "claude", "fix signup");
        e2.total_cost_usd = 0.3;
        e2.status = RunStatus::Completed;
        let mut e3 = make_entry("r3", "/repo/a", "claude", "fix auth");
        e3.total_cost_usd = 0.1;
        e3.status = RunStatus::Failed;

        let entries = vec![e1, e2, e3];
        let filters = RunSearchFilters {
            query: Some("fix".to_string()),
            projects: Some(vec!["/repo/a".to_string()]),
            statuses: Some(vec![RunStatus::Completed]),
            ..empty_filters()
        };
        let result = apply_filters(&entries, &filters);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].run_id, "r1");
    }

    #[test]
    fn test_sort_by_cost_desc() {
        let mut e1 = make_entry("r1", "/repo", "claude", "cheap");
        e1.total_cost_usd = 0.1;
        let mut e2 = make_entry("r2", "/repo", "claude", "expensive");
        e2.total_cost_usd = 2.0;
        let mut e3 = make_entry("r3", "/repo", "claude", "medium");
        e3.total_cost_usd = 0.5;

        let mut entries = vec![e1, e2, e3];
        let filters = RunSearchFilters {
            sort_by: Some("cost".to_string()),
            sort_asc: Some(false),
            ..empty_filters()
        };
        sort_entries(&mut entries, &filters);
        assert_eq!(entries[0].run_id, "r2"); // most expensive first
        assert_eq!(entries[1].run_id, "r3");
        assert_eq!(entries[2].run_id, "r1");
    }

    #[test]
    fn test_pagination() {
        let entries: Vec<RunIndexEntry> = (0..10)
            .map(|i| {
                let mut e = make_entry(&format!("r{}", i), "/repo", "claude", &format!("p{}", i));
                e.started_at = format!("2024-01-{:02}T00:00:00.000Z", i + 1);
                e
            })
            .collect();

        let filters = RunSearchFilters {
            sort_by: Some("date".to_string()),
            sort_asc: Some(true),
            ..empty_filters()
        };

        let mut sorted = apply_filters(&entries, &filters);
        sort_entries(&mut sorted, &filters);

        // Page 1: offset=0, limit=3
        let page1: Vec<_> = sorted.iter().skip(0).take(3).collect();
        assert_eq!(page1.len(), 3);
        assert_eq!(page1[0].run_id, "r0");
        assert_eq!(page1[2].run_id, "r2");

        // Page 2: offset=3, limit=3
        let page2: Vec<_> = sorted.iter().skip(3).take(3).collect();
        assert_eq!(page2.len(), 3);
        assert_eq!(page2[0].run_id, "r3");

        // Last page: offset=9, limit=3
        let last: Vec<_> = sorted.iter().skip(9).take(3).collect();
        assert_eq!(last.len(), 1);
        assert_eq!(last[0].run_id, "r9");
    }
}
