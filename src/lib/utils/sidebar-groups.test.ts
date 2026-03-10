import { describe, it, expect } from "vitest";
import type { TaskRun } from "$lib/types";
import {
  buildProjectFolders,
  autoExpandForRun,
  expandForProjectChange,
  normalizeCwd,
} from "./sidebar-groups";

// ── Test helpers ──

function makeRun(overrides: Partial<TaskRun> = {}): TaskRun {
  return {
    id: "r1",
    prompt: "hello",
    cwd: "/project",
    agent: "claude",
    auth_mode: "cli",
    status: "completed",
    started_at: "2024-01-01T00:00:00Z",
    ...overrides,
  };
}

const NO_FAVS = new Set<string>();
const NO_PINS: string[] = [];

// ── normalizeCwd ──

describe("normalizeCwd", () => {
  it("returns empty for undefined/null/empty", () => {
    expect(normalizeCwd(undefined)).toBe("");
    expect(normalizeCwd("")).toBe("");
    expect(normalizeCwd("  ")).toBe("");
  });

  it("returns empty for root slash", () => {
    expect(normalizeCwd("/")).toBe("");
    expect(normalizeCwd("\\")).toBe("");
  });

  it("strips trailing slashes", () => {
    expect(normalizeCwd("/path/to/proj/")).toBe("/path/to/proj");
    expect(normalizeCwd("/path/to/proj///")).toBe("/path/to/proj");
  });

  it("preserves_drive_root", () => {
    expect(normalizeCwd("C:\\")).toBe("C:/");
    expect(normalizeCwd("C:/")).toBe("C:/");
  });

  it("unifies_case_and_separators", () => {
    expect(normalizeCwd("c:\\Repo")).toBe("C:/Repo");
    expect(normalizeCwd("C:/Repo")).toBe("C:/Repo");
    expect(normalizeCwd("c:\\Repo")).toBe(normalizeCwd("C:/Repo"));
  });

  it("preserves_unc", () => {
    expect(normalizeCwd("\\\\server\\share")).toBe("//server/share");
    expect(normalizeCwd("//server/share")).toBe("//server/share");
  });

  it("bare_drive_letter", () => {
    expect(normalizeCwd("C:")).toBe("C:/");
    expect(normalizeCwd("d:")).toBe("D:/");
  });

  it("cwd_trailing_backslash_normalized", () => {
    const a = normalizeCwd("C:\\Users\\proj\\");
    const b = normalizeCwd("C:\\Users\\proj");
    expect(a).toBe(b);
    expect(a).toBe("C:/Users/proj");
  });
});

// ── buildProjectFolders ──

describe("buildProjectFolders", () => {
  it("groups_runs_by_session_id", () => {
    const runs = [
      makeRun({ id: "r1", session_id: "s1", started_at: "2024-01-01T00:00:00Z" }),
      makeRun({ id: "r2", session_id: "s1", started_at: "2024-01-02T00:00:00Z" }),
      makeRun({ id: "r3", session_id: "s1", started_at: "2024-01-03T00:00:00Z" }),
    ];
    const folders = buildProjectFolders(runs, NO_FAVS, NO_PINS);
    expect(folders).toHaveLength(1);
    expect(folders[0].conversations).toHaveLength(1);
    expect(folders[0].conversations[0].runs).toHaveLength(3);
  });

  it("no_session_id_stays_individual", () => {
    const runs = [makeRun({ id: "r1" }), makeRun({ id: "r2" })];
    const folders = buildProjectFolders(runs, NO_FAVS, NO_PINS);
    expect(folders).toHaveLength(1);
    expect(folders[0].conversations).toHaveLength(2);
  });

  it("mixed_sessions_and_standalone", () => {
    const runs = [
      makeRun({ id: "r1", session_id: "s1" }),
      makeRun({ id: "r2", session_id: "s1" }),
      makeRun({ id: "r3" }), // standalone
    ];
    const folders = buildProjectFolders(runs, NO_FAVS, NO_PINS);
    expect(folders[0].conversations).toHaveLength(2); // 1 session group + 1 standalone
  });

  it("cross_cwd_same_session_id_separate", () => {
    const runs = [
      makeRun({ id: "r1", session_id: "s1", cwd: "/projA" }),
      makeRun({ id: "r2", session_id: "s1", cwd: "/projB" }),
    ];
    const folders = buildProjectFolders(runs, NO_FAVS, NO_PINS);
    expect(folders).toHaveLength(2);
    // Each folder should have its own conversation group
    expect(folders[0].conversations).toHaveLength(1);
    expect(folders[1].conversations).toHaveLength(1);
  });

  it("empty_cwd_goes_to_uncategorized", () => {
    const runs = [
      makeRun({ id: "r1", cwd: "" }),
      makeRun({ id: "r2", cwd: "/" }),
      makeRun({ id: "r3", cwd: "/proj" }),
    ];
    const folders = buildProjectFolders(runs, NO_FAVS, NO_PINS);
    const uncat = folders.find((f) => f.isUncategorized);
    expect(uncat).toBeDefined();
    expect(uncat!.conversations).toHaveLength(2); // r1 and r2 both → uncategorized
    const proj = folders.find((f) => !f.isUncategorized);
    expect(proj).toBeDefined();
    expect(proj!.conversations).toHaveLength(1);
  });

  it("uncategorized_folder_at_end", () => {
    const runs = [
      makeRun({ id: "r1", cwd: "", started_at: "2024-12-01T00:00:00Z" }), // newer
      makeRun({ id: "r2", cwd: "/proj", started_at: "2024-01-01T00:00:00Z" }), // older
    ];
    const folders = buildProjectFolders(runs, NO_FAVS, NO_PINS);
    expect(folders).toHaveLength(2);
    expect(folders[folders.length - 1].isUncategorized).toBe(true);
  });

  it("pinned_cwds_empty_folders", () => {
    const folders = buildProjectFolders([], NO_FAVS, ["/pinned/proj"]);
    expect(folders).toHaveLength(1);
    expect(folders[0].cwd).toBe("/pinned/proj");
    expect(folders[0].conversations).toHaveLength(0);
  });

  it("favorites_propagate_to_conversation", () => {
    const runs = [makeRun({ id: "r1", session_id: "s1" }), makeRun({ id: "r2", session_id: "s1" })];
    const favs = new Set(["r2"]);
    const folders = buildProjectFolders(runs, favs, NO_PINS);
    expect(folders[0].conversations[0].isFavorite).toBe(true);
  });

  it("sort_order_newest_first", () => {
    const runs = [
      makeRun({
        id: "r1",
        session_id: "s1",
        started_at: "2024-01-01T00:00:00Z",
        last_activity_at: "2024-01-01T00:00:00Z",
      }),
      makeRun({
        id: "r2",
        session_id: "s2",
        started_at: "2024-06-01T00:00:00Z",
        last_activity_at: "2024-06-01T00:00:00Z",
      }),
    ];
    const folders = buildProjectFolders(runs, NO_FAVS, NO_PINS);
    expect(folders[0].conversations[0].groupKey).toBe("s:s2");
    expect(folders[0].conversations[1].groupKey).toBe("s:s1");
  });

  it("title_prefers_latest_name", () => {
    const runs = [
      makeRun({
        id: "r1",
        session_id: "s1",
        started_at: "2024-01-01T00:00:00Z",
        prompt: "early prompt",
      }),
      makeRun({
        id: "r2",
        session_id: "s1",
        started_at: "2024-02-01T00:00:00Z",
        name: "My Custom Name",
      }),
    ];
    const folders = buildProjectFolders(runs, NO_FAVS, NO_PINS);
    expect(folders[0].conversations[0].title).toBe("My Custom Name");
  });

  it("title_fallback_to_earliest_prompt", () => {
    const runs = [
      makeRun({
        id: "r1",
        session_id: "s1",
        started_at: "2024-01-01T00:00:00Z",
        prompt: "first prompt",
      }),
      makeRun({
        id: "r2",
        session_id: "s1",
        started_at: "2024-02-01T00:00:00Z",
        prompt: "second prompt",
      }),
    ];
    const folders = buildProjectFolders(runs, NO_FAVS, NO_PINS);
    // No name on latestRun → fallback to earliest prompt
    expect(folders[0].conversations[0].title).toBe("first prompt");
  });

  it("title_empty_name_not_used", () => {
    const runs = [
      makeRun({
        id: "r1",
        session_id: "s1",
        started_at: "2024-01-01T00:00:00Z",
        prompt: "real prompt",
      }),
      makeRun({
        id: "r2",
        session_id: "s1",
        started_at: "2024-02-01T00:00:00Z",
        name: "  ",
        prompt: "second",
      }),
    ];
    const folders = buildProjectFolders(runs, NO_FAVS, NO_PINS);
    // "  " should be trimmed to "" and skipped → fallback to earliest prompt
    expect(folders[0].conversations[0].title).toBe("real prompt");
  });

  it("no_label_field_on_folder", () => {
    const runs = [makeRun({ id: "r1" })];
    const folders = buildProjectFolders(runs, NO_FAVS, NO_PINS);

    expect((folders[0] as any).label).toBeUndefined();
    expect(folders[0].cwd).toBeDefined();
    expect(folders[0].isUncategorized).toBeDefined();
  });

  it("group_key_has_prefix", () => {
    const runs = [
      makeRun({ id: "r1", session_id: "s1" }),
      makeRun({ id: "r2" }), // standalone
    ];
    const folders = buildProjectFolders(runs, NO_FAVS, NO_PINS);
    const keys = folders[0].conversations.map((c) => c.groupKey);
    expect(keys).toContain("s:s1");
    expect(keys).toContain("r:r2");
  });

  it("pinned_cwds_filters_empty_and_root", () => {
    const folders = buildProjectFolders([], NO_FAVS, ["", "/", "/real/proj"]);
    // Only /real/proj should produce a folder
    expect(folders).toHaveLength(1);
    expect(folders[0].cwd).toBe("/real/proj");
  });

  it("cwd_trailing_slash_normalized", () => {
    const runs = [
      makeRun({ id: "r1", cwd: "/path/to/proj/" }),
      makeRun({ id: "r2", cwd: "/path/to/proj" }),
    ];
    const folders = buildProjectFolders(runs, NO_FAVS, NO_PINS);
    expect(folders).toHaveLength(1);
    expect(folders[0].conversations).toHaveLength(2);
  });

  it("cwd_trailing_backslash_normalized", () => {
    const runs = [
      makeRun({ id: "r1", cwd: "C:\\Users\\proj\\" }),
      makeRun({ id: "r2", cwd: "C:\\Users\\proj" }),
    ];
    const folders = buildProjectFolders(runs, NO_FAVS, NO_PINS);
    expect(folders).toHaveLength(1);
    expect(folders[0].cwd).toBe("C:/Users/proj");
  });

  it("removed_cwd_not_in_folders", () => {
    const runs = [makeRun({ id: "r1", cwd: "/projA" }), makeRun({ id: "r2", cwd: "/projB" })];
    const folders = buildProjectFolders(runs, NO_FAVS, NO_PINS, ["/projA"]);
    expect(folders).toHaveLength(1);
    expect(folders[0].cwd).toBe("/projB");
  });

  it("uncategorized_never_removed", () => {
    const runs = [makeRun({ id: "r1", cwd: "" }), makeRun({ id: "r2", cwd: "/proj" })];
    // Attempt to remove "" (Uncategorized) — should have no effect
    const folders = buildProjectFolders(runs, NO_FAVS, NO_PINS, [""]);
    const uncat = folders.find((f) => f.isUncategorized);
    expect(uncat).toBeDefined();
    expect(uncat!.conversations).toHaveLength(1);
  });

  it("removed_cwd_in_pinnedCwds_no_empty_folder", () => {
    // If a cwd is both pinned and removed, it should not generate a folder
    const folders = buildProjectFolders([], NO_FAVS, ["/pinned/proj"], ["/pinned/proj"]);
    expect(folders).toHaveLength(0);
  });

  it("removed_cwd_normalization_consistent", () => {
    const runs = [makeRun({ id: "r1", cwd: "C:\\Users\\proj" })];
    // Remove with different formatting — should still match after normalization
    const folders = buildProjectFolders(runs, NO_FAVS, NO_PINS, ["c:\\Users\\proj\\"]);
    expect(folders).toHaveLength(0);
  });
});

// ── autoExpandForRun ──

describe("autoExpandForRun", () => {
  it("auto_expand_adds_folder_for_selected_run", () => {
    const runs = [makeRun({ id: "r1", cwd: "/proj" })];
    const folders = buildProjectFolders(runs, NO_FAVS, NO_PINS);
    const result = autoExpandForRun("r1", folders, new Set());
    expect(result).not.toBeNull();
    expect(result!.has("cwd:/proj")).toBe(true);
  });

  it("auto_expand_returns_null_if_already_expanded", () => {
    const runs = [makeRun({ id: "r1", cwd: "/proj" })];
    const folders = buildProjectFolders(runs, NO_FAVS, NO_PINS);
    const result = autoExpandForRun("r1", folders, new Set(["cwd:/proj"]));
    expect(result).toBeNull();
  });

  it("auto_expand_returns_null_if_no_selected_run", () => {
    const runs = [makeRun({ id: "r1" })];
    const folders = buildProjectFolders(runs, NO_FAVS, NO_PINS);
    expect(autoExpandForRun(undefined, folders, new Set())).toBeNull();
    expect(autoExpandForRun("", folders, new Set())).toBeNull();
  });
});

// ── expandForProjectChange ──

describe("expandForProjectChange", () => {
  it("expand_for_project_change_adds_cwd", () => {
    const result = expandForProjectChange("cwd:/proj", new Set());
    expect(result).not.toBeNull();
    expect(result!.has("cwd:/proj")).toBe(true);
  });

  it("expand_for_project_change_skips_empty_cwd", () => {
    const result = expandForProjectChange("", new Set());
    expect(result).toBeNull();
  });

  it("expand_for_project_change_skips_already_expanded", () => {
    const result = expandForProjectChange("cwd:/proj", new Set(["cwd:/proj"]));
    expect(result).toBeNull();
  });

  it("folderKey_uncategorized_does_not_conflict_with_all_projects", () => {
    // "All Projects" sends empty string → expandForProjectChange should skip
    const result = expandForProjectChange("", new Set());
    expect(result).toBeNull();
    // Uncategorized folder has folderKey "uncategorized", not ""
    const runs = [makeRun({ id: "r1", cwd: "" })];
    const folders = buildProjectFolders(runs, NO_FAVS, NO_PINS);
    const uncatFolder = folders.find((f) => f.isUncategorized);
    expect(uncatFolder?.folderKey).toBe("uncategorized");
  });
});
