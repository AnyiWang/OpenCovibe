/**
 * TeamStore: reactive state for the Teams panel.
 *
 * Watcher events (team-update, task-update) are the primary refresh path.
 * Layout poll (loadTeams every 60s) is a fallback for startup + missed events.
 */
import * as api from "$lib/api";
import type { TeamSummary, TeamConfig, TeamTask, TeamInboxMessage } from "$lib/types";
import { dbg, dbgWarn } from "$lib/utils/debug";

export class TeamStore {
  teams = $state<TeamSummary[]>([]);
  selectedTeam = $state("");
  teamConfig = $state<TeamConfig | null>(null);
  tasks = $state<TeamTask[]>([]);
  inbox = $state<TeamInboxMessage[]>([]);
  allInbox = $state<TeamInboxMessage[]>([]);
  inboxAgent = $state("");
  loading = $state(false);

  /** Currently expanded task id (for description view) */
  expandedTaskId = $state<string | null>(null);

  private _lastWatcherRefresh = 0;
  private static readonly _WATCHER_COOLDOWN_MS = 10_000;

  get pendingTasks(): TeamTask[] {
    return this.tasks.filter((t) => t.status === "pending");
  }

  get inProgressTasks(): TeamTask[] {
    return this.tasks.filter((t) => t.status === "in_progress");
  }

  get completedTasks(): TeamTask[] {
    return this.tasks.filter((t) => t.status === "completed");
  }

  /** Load team list. Skipped if watcher refreshed recently (poll is fallback only). */
  async loadTeams(): Promise<void> {
    if (Date.now() - this._lastWatcherRefresh < TeamStore._WATCHER_COOLDOWN_MS) {
      dbg("teams", "loadTeams skipped — watcher refreshed recently");
      return;
    }
    this.loading = true;
    try {
      this.teams = await api.listTeams();
      dbg("teams", "loadTeams", { count: this.teams.length });
    } catch (e) {
      dbgWarn("teams", "loadTeams error", e);
    } finally {
      this.loading = false;
    }
  }

  /** Force-refresh teams + selected team state. Bypasses cooldown.
   *  Used by layout listener recovery to fill gaps from missed events. */
  async forceRefresh(): Promise<void> {
    dbg("teams", "forceRefresh");
    try {
      this.teams = await api.listTeams();
      if (this.selectedTeam) await this.selectTeam(this.selectedTeam);
    } catch (e) {
      dbgWarn("teams", "forceRefresh error", e);
    }
  }

  /** Select a team and fetch its config + tasks + all inboxes. Always fetches fresh data. */
  async selectTeam(name: string): Promise<void> {
    this.selectedTeam = name;
    if (!name) {
      this.teamConfig = null;
      this.tasks = [];
      this.inbox = [];
      this.allInbox = [];
      this.inboxAgent = "";
      this.expandedTaskId = null;
      return;
    }
    dbg("teams", "selectTeam", name);
    try {
      const [config, tasks, allMsgs] = await Promise.all([
        api.getTeamConfig(name),
        api.listTeamTasks(name),
        api.getAllTeamInboxes(name),
      ]);
      if (this.selectedTeam !== name) return; // stale response, discard
      this.teamConfig = config;
      this.tasks = tasks;
      this.allInbox = allMsgs;
      dbg("teams", "selectTeam loaded", {
        members: config.members.length,
        tasks: tasks.length,
        inbox: allMsgs.length,
      });

      // Auto-load inbox for first member
      if (config.members.length > 0 && !this.inboxAgent) {
        this.loadInbox(name, config.members[0].name);
      }
    } catch (e) {
      dbgWarn("teams", "selectTeam error", e);
      if (this.selectedTeam !== name) return;
      this.teamConfig = null;
      this.tasks = [];
      this.allInbox = [];
    }
  }

  /** Load inbox messages for a specific agent in the current team. */
  async loadInbox(team: string, agent: string): Promise<void> {
    this.inboxAgent = agent;
    try {
      const msgs = await api.getTeamInbox(team, agent);
      if (this.selectedTeam !== team || this.inboxAgent !== agent) return; // stale
      this.inbox = msgs;
      dbg("teams", "loadInbox", { team, agent, count: msgs.length });
    } catch (e) {
      dbgWarn("teams", "loadInbox error", e);
      if (this.selectedTeam !== team || this.inboxAgent !== agent) return;
      this.inbox = [];
    }
  }

  /** Load all inboxes merged for the current team. */
  async loadAllInbox(team: string): Promise<void> {
    try {
      const msgs = await api.getAllTeamInboxes(team);
      if (this.selectedTeam !== team) return; // stale
      this.allInbox = msgs;
      dbg("teams", "loadAllInbox", { team, count: msgs.length });
    } catch (e) {
      dbgWarn("teams", "loadAllInbox error", e);
    }
  }

  /** Delete a team (removes ~/.claude/teams/{name} and ~/.claude/tasks/{name}). */
  async deleteTeam(name: string): Promise<void> {
    dbg("teams", "deleteTeam", name);
    await api.deleteTeam(name);
    // Clear selection if the deleted team was selected
    if (this.selectedTeam === name) {
      this.selectedTeam = "";
      this.teamConfig = null;
      this.tasks = [];
      this.inbox = [];
      this.allInbox = [];
      this.inboxAgent = "";
      this.expandedTaskId = null;
    }
    // Refresh team list
    this.teams = this.teams.filter((t) => t.name !== name);
  }

  /** Handle team-update watcher event (primary refresh path). */
  handleTeamUpdate(payload: { team_name: string; change: string }): void {
    this._lastWatcherRefresh = Date.now();
    dbg("teams", "handleTeamUpdate", payload);

    if (payload.team_name === this.selectedTeam) {
      if (payload.change === "inbox") {
        // Inbox-only change: just refresh inbox data (no config/tasks reload)
        this.loadAllInbox(this.selectedTeam);
        if (this.inboxAgent) {
          this.loadInbox(this.selectedTeam, this.inboxAgent);
        }
      } else {
        // Config/member/other change: full refresh (selectTeam includes allInbox)
        this.selectTeam(this.selectedTeam);
      }
    }

    api
      .listTeams()
      .then((t) => {
        this.teams = t;
      })
      .catch(() => {});
  }

  /** Handle task-update watcher event (primary refresh path). */
  handleTaskUpdate(payload: { team_name: string; task_id: string; change: string }): void {
    this._lastWatcherRefresh = Date.now();
    dbg("teams", "handleTaskUpdate", payload);
    if (payload.team_name === this.selectedTeam) {
      api
        .listTeamTasks(this.selectedTeam)
        .then((t) => {
          this.tasks = t;
        })
        .catch((e) => dbgWarn("teams", "task refresh error", e));
    }
  }
}
