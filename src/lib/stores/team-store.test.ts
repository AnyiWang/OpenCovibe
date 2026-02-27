/**
 * TeamStore unit tests.
 *
 * Tests computed getters, watcher event handlers, and cooldown logic.
 */
import { describe, it, expect, vi, beforeEach } from "vitest";

// Mock Tauri API
vi.mock("$lib/api", () => ({
  listTeams: vi.fn(),
  getTeamConfig: vi.fn(),
  listTeamTasks: vi.fn(),
  getTeamInbox: vi.fn(),
}));

vi.mock("$lib/utils/debug", () => ({
  dbg: vi.fn(),
  dbgWarn: vi.fn(),
}));

// Import after mocks
import { TeamStore } from "./team-store.svelte";
import * as api from "$lib/api";

describe("TeamStore", () => {
  let store: TeamStore;

  beforeEach(() => {
    store = new TeamStore();
    vi.clearAllMocks();
  });

  describe("initial state", () => {
    it("starts with empty teams and no selection", () => {
      expect(store.teams).toEqual([]);
      expect(store.selectedTeam).toBe("");
      expect(store.teamConfig).toBeNull();
      expect(store.tasks).toEqual([]);
      expect(store.inbox).toEqual([]);
      expect(store.inboxAgent).toBe("");
      expect(store.loading).toBe(false);
    });
  });

  describe("computed getters filter by status", () => {
    it("pendingTasks returns only pending tasks", () => {
      store.tasks = [
        {
          id: "1",
          subject: "A",
          description: "",
          activeForm: "",
          owner: "",
          status: "pending",
          blocks: [],
          blockedBy: [],
        },
        {
          id: "2",
          subject: "B",
          description: "",
          activeForm: "",
          owner: "",
          status: "in_progress",
          blocks: [],
          blockedBy: [],
        },
        {
          id: "3",
          subject: "C",
          description: "",
          activeForm: "",
          owner: "",
          status: "completed",
          blocks: [],
          blockedBy: [],
        },
      ];
      expect(store.pendingTasks).toHaveLength(1);
      expect(store.pendingTasks[0].id).toBe("1");
    });

    it("inProgressTasks returns only in_progress tasks", () => {
      store.tasks = [
        {
          id: "1",
          subject: "A",
          description: "",
          activeForm: "",
          owner: "",
          status: "pending",
          blocks: [],
          blockedBy: [],
        },
        {
          id: "2",
          subject: "B",
          description: "",
          activeForm: "",
          owner: "w",
          status: "in_progress",
          blocks: [],
          blockedBy: [],
        },
        {
          id: "3",
          subject: "C",
          description: "",
          activeForm: "",
          owner: "",
          status: "in_progress",
          blocks: [],
          blockedBy: [],
        },
      ];
      expect(store.inProgressTasks).toHaveLength(2);
    });

    it("completedTasks returns only completed tasks", () => {
      store.tasks = [
        {
          id: "1",
          subject: "A",
          description: "",
          activeForm: "",
          owner: "",
          status: "completed",
          blocks: [],
          blockedBy: [],
        },
        {
          id: "2",
          subject: "B",
          description: "",
          activeForm: "",
          owner: "",
          status: "pending",
          blocks: [],
          blockedBy: [],
        },
      ];
      expect(store.completedTasks).toHaveLength(1);
      expect(store.completedTasks[0].id).toBe("1");
    });
  });

  describe("handleTeamUpdate", () => {
    it("sets watcher refresh timestamp (loadTeams cooldown)", async () => {
      vi.mocked(api.listTeams).mockResolvedValue([]);
      store.handleTeamUpdate({ team_name: "test", change: "created" });

      // loadTeams should now skip due to cooldown
      await store.loadTeams();
      // listTeams called once by handleTeamUpdate, but NOT by loadTeams (cooldown skip)
      expect(api.listTeams).toHaveBeenCalledTimes(1);
    });
  });

  describe("handleTaskUpdate", () => {
    it("refreshes tasks for selected team", async () => {
      const mockTasks = [
        {
          id: "1",
          subject: "Updated",
          description: "",
          activeForm: "",
          owner: "w",
          status: "in_progress",
          blocks: [],
          blockedBy: [],
        },
      ];
      vi.mocked(api.listTeamTasks).mockResolvedValue(mockTasks);
      store.selectedTeam = "my-team";

      store.handleTaskUpdate({ team_name: "my-team", task_id: "1", change: "updated" });

      // Wait for async task refresh
      await vi.waitFor(() => {
        expect(api.listTeamTasks).toHaveBeenCalledWith("my-team");
      });
    });

    it("does not refresh tasks for unselected team", () => {
      store.selectedTeam = "other-team";
      store.handleTaskUpdate({ team_name: "my-team", task_id: "1", change: "updated" });
      expect(api.listTeamTasks).not.toHaveBeenCalled();
    });
  });

  describe("loadTeams cooldown", () => {
    it("skips when watcher recently refreshed", async () => {
      vi.mocked(api.listTeams).mockResolvedValue([]);

      // Simulate watcher refresh
      store.handleTeamUpdate({ team_name: "t", change: "created" });
      vi.mocked(api.listTeams).mockClear();

      // loadTeams should be skipped (within cooldown window)
      await store.loadTeams();
      expect(api.listTeams).not.toHaveBeenCalled();
    });
  });
});
