/**
 * Session store reducer tests.
 *
 * Tests the core reducer logic (applyEvent / applyEventBatch) using
 * event fixtures derived from real ~/.opencovibe/runs/ data.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import type { BusEvent, TimelineEntry } from "$lib/types";
import { assertTransition, canResumeRun, getResumeWarning, classifyError } from "./types";

// Mock Tauri API — the store imports api.ts which calls invoke()
vi.mock("$lib/api", () => ({
  getRun: vi.fn(),
  getBusEvents: vi.fn(),
  getRunEvents: vi.fn(),
  startRun: vi.fn(),
  startSession: vi.fn(),
  sendSessionMessage: vi.fn(),
  sendChatMessage: vi.fn(),
  stopSession: vi.fn(),
  stopRun: vi.fn(),
  sendApiMessage: vi.fn(),
  stopApiAgent: vi.fn(),
  sendSessionControl: vi.fn(),
  syncCliSession: vi.fn().mockResolvedValue({ newEvents: 0 }),
}));

// Mock debug utils — they access localStorage which doesn't exist in node
vi.mock("$lib/utils/debug", () => ({
  dbg: vi.fn(),
  dbgWarn: vi.fn(),
}));

// Mock snapshot-cache — IndexedDB not available in Vitest (node)
vi.mock("$lib/utils/snapshot-cache", () => ({
  readSnapshot: vi.fn().mockResolvedValue(null),
  writeSnapshot: vi.fn().mockResolvedValue(undefined),
  deleteSnapshot: vi.fn().mockResolvedValue(undefined),
}));

// Mock cli-info — getCliCommands used by isKnownSlashCommand
const cliInfoMocks = vi.hoisted(() => ({
  getCliCommands: vi.fn().mockReturnValue([]),
  updateInstalledVersion: vi.fn(),
}));
vi.mock("./cli-info.svelte", () => cliInfoMocks);

// Fixtures
import simpleChatEvents from "./__fixtures__/simple-chat.json";
import chatWithToolsEvents from "./__fixtures__/chat-with-tools.json";
import multiTurnEvents from "./__fixtures__/multi-turn.json";
import sessionFailedEvents from "./__fixtures__/session-failed.json";
import askUserQuestionEvents from "./__fixtures__/ask-user-question.json";
import resultErrorMaxTurnsEvents from "./__fixtures__/result-error-max-turns.json";
import compactBoundaryEvents from "./__fixtures__/compact-boundary.json";
import subagentTaskEvents from "./__fixtures__/subagent-task.json";
import protocolEvents from "./__fixtures__/protocol-events.json";
import teamSessionEvents from "./__fixtures__/team-session.json";
import malformedEvents from "./__fixtures__/malformed-events.json";

// Import store and mocked modules after mocks
import { SessionStore } from "./session-store.svelte";
import * as snapshotCache from "$lib/utils/snapshot-cache";
import * as api from "$lib/api";

function makeRun(id: string, overrides: Record<string, unknown> = {}) {
  return {
    id,
    prompt: "test",
    cwd: "/",
    agent: "claude",
    auth_mode: "cli",
    status: "running" as const,
    started_at: new Date().toISOString(),
    ...overrides,
  };
}

describe("SessionStore reducer", () => {
  let store: SessionStore;
  let warnSpy: ReturnType<typeof vi.spyOn>;

  beforeEach(() => {
    store = new SessionStore();
    // Catch unexpected console.warn (transition guard warnings = test bug)
    warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});
  });

  afterEach(() => {
    // Unless the test explicitly expects warnings, zero = healthy.
    // Tests that intentionally trigger warnings must call warnSpy.mockClear()
    // before returning so this check passes.
    if (warnSpy.mock.calls.length > 0) {
      const msgs = warnSpy.mock.calls.map((c) => c.join(" ")).join("\n");
      warnSpy.mockRestore();
      throw new Error(`Unexpected console.warn during test:\n${msgs}`);
    }
    warnSpy.mockRestore();
  });

  // ── Simple chat replay ──

  describe("simple chat replay", () => {
    beforeEach(() => {
      store.run = makeRun("run-1");
      store.phase = "running";
      store.applyEventBatch(simpleChatEvents as BusEvent[]);
    });

    it("builds correct timeline", () => {
      expect(store.timeline).toHaveLength(2); // user_message + message_complete
      expect(store.timeline[0].kind).toBe("user");
      expect(store.timeline[0].content).toBe("Hello");
      expect(store.timeline[1].kind).toBe("assistant");
      expect(store.timeline[1].content).toBe("Hi there! How can I help?");
    });

    it("sets model from session_init", () => {
      expect(store.model).toBe("claude-opus-4-6");
    });

    it("updates usage from usage_update", () => {
      expect(store.usage.inputTokens).toBe(100);
      expect(store.usage.outputTokens).toBe(20);
      expect(store.usage.cost).toBe(0.005);
    });

    it("appends per-turn usage snapshot", () => {
      expect(store.turnUsages).toHaveLength(1);
      expect(store.turnUsages[0].inputTokens).toBe(100);
      expect(store.turnUsages[0].outputTokens).toBe(20);
      expect(store.turnUsages[0].cost).toBe(0.005);
      // turnIndex should match the number of user_messages seen (1 in simple-chat)
      expect(store.turnUsages[0].turnIndex).toBe(1);
    });

    it("ends at idle phase", () => {
      expect(store.phase).toBe("idle");
    });

    it("clears streaming text after message_complete", () => {
      expect(store.streamingText).toBe("");
    });
  });

  // ── Chat with tools ──

  describe("chat with tools", () => {
    beforeEach(() => {
      store.run = makeRun("run-2");
      store.phase = "running";
      store.applyEventBatch(chatWithToolsEvents as BusEvent[]);
    });

    it("tracks tool start and end in timeline", () => {
      const toolEntries = store.timeline.filter((e) => e.kind === "tool");
      expect(toolEntries).toHaveLength(2);
    });

    it("resolves first tool as success", () => {
      const t1 = store.timeline.find((e) => e.kind === "tool" && e.id === "tool-1") as Extract<
        TimelineEntry,
        { kind: "tool" }
      >;
      expect(t1).toBeDefined();
      expect(t1.tool.status).toBe("success");
      expect(t1.tool.duration_ms).toBe(50);
    });

    it("resolves second tool as error", () => {
      const t2 = store.timeline.find((e) => e.kind === "tool" && e.id === "tool-2") as Extract<
        TimelineEntry,
        { kind: "tool" }
      >;
      expect(t2).toBeDefined();
      expect(t2.tool.status).toBe("error");
    });

    it("stream mode: tools mirror (HookEvent[]) is empty, data in timeline only", () => {
      // In stream mode (claude + cli), reducer no longer writes to store.tools
      expect(store.tools).toHaveLength(0);
      // Tools are tracked in timeline as BusToolItem
      const toolEntries = store.timeline.filter((e) => e.kind === "tool");
      expect(toolEntries).toHaveLength(2);
      expect(toolEntries[0].tool.tool_name).toBe("Write");
      expect(toolEntries[0].tool.status).toBe("success");
      expect(toolEntries[1].tool.tool_name).toBe("Bash");
      expect(toolEntries[1].tool.status).toBe("error");
    });

    it("has assistant message after tools", () => {
      const msgs = store.timeline.filter((e) => e.kind === "assistant");
      expect(msgs).toHaveLength(1);
      expect(msgs[0].content).toContain("denied");
    });
  });

  // ── Multi-turn session ──

  describe("multi-turn session", () => {
    beforeEach(() => {
      store.run = makeRun("run-3");
      store.phase = "running";
      store.applyEventBatch(multiTurnEvents as BusEvent[]);
    });

    it("builds correct timeline with 2 turns", () => {
      const users = store.timeline.filter((e) => e.kind === "user");
      const assistants = store.timeline.filter((e) => e.kind === "assistant");
      const tools = store.timeline.filter((e) => e.kind === "tool");
      expect(users).toHaveLength(2);
      expect(assistants).toHaveLength(2);
      expect(tools).toHaveLength(1);
    });

    it("preserves timeline order", () => {
      expect(store.timeline[0].kind).toBe("user"); // "Hi"
      expect(store.timeline[1].kind).toBe("assistant"); // "Hello! How..."
      expect(store.timeline[2].kind).toBe("user"); // "Tell me more"
      expect(store.timeline[3].kind).toBe("tool"); // Bash
      expect(store.timeline[4].kind).toBe("assistant"); // "Here is more"
    });

    it("updates usage to latest values (overwrite, not accumulate)", () => {
      // usage_update overwrites — final values are from the second update
      expect(store.usage.inputTokens).toBe(150);
      expect(store.usage.outputTokens).toBe(30);
      expect(store.usage.cost).toBe(0.008);
    });

    it("ends at idle phase", () => {
      expect(store.phase).toBe("idle");
    });

    it("tracks per-turn usage with correct turnIndex", () => {
      expect(store.turnUsages).toHaveLength(2);
      // Turn 1: after first user_message("Hi")
      expect(store.turnUsages[0].turnIndex).toBe(1);
      expect(store.turnUsages[0].inputTokens).toBe(50);
      expect(store.turnUsages[0].outputTokens).toBe(10);
      // Turn 2: after second user_message("Tell me more")
      expect(store.turnUsages[1].turnIndex).toBe(2);
      expect(store.turnUsages[1].inputTokens).toBe(150);
      expect(store.turnUsages[1].outputTokens).toBe(30);
    });
  });

  // ── Session failure ──

  describe("session failure", () => {
    beforeEach(() => {
      store.run = makeRun("run-4");
      store.phase = "running";
      store.applyEventBatch(sessionFailedEvents as BusEvent[]);
    });

    it("ends at failed phase", () => {
      expect(store.phase).toBe("failed");
    });

    it("captures error message", () => {
      expect(store.error).toBe("Process exited with code None");
    });

    it("still has timeline from before failure", () => {
      expect(store.timeline).toHaveLength(2); // user + assistant
    });
  });

  // ── Resume replay (replayOnly=true) ──

  describe("resume replay (replayOnly)", () => {
    it("does NOT change phase during replay", () => {
      store.run = makeRun("run-1");
      store.phase = "spawning"; // phase set before replay (resume flow)
      store.applyEventBatch(simpleChatEvents as BusEvent[], {
        replayOnly: true,
      });

      // Phase should remain "spawning" — replay must not overwrite it
      expect(store.phase).toBe("spawning");
    });

    it("does NOT set error during replay", () => {
      store.run = makeRun("run-4");
      store.phase = "spawning";
      store.applyEventBatch(sessionFailedEvents as BusEvent[], {
        replayOnly: true,
      });

      // Error should not be set in replayOnly mode
      expect(store.error).toBe("");
      // Phase should remain spawning
      expect(store.phase).toBe("spawning");
    });

    it("still builds timeline and usage during replay", () => {
      store.run = makeRun("run-3");
      store.phase = "spawning";
      store.applyEventBatch(multiTurnEvents as BusEvent[], {
        replayOnly: true,
      });

      // Timeline and usage should still be populated
      expect(store.timeline).toHaveLength(5);
      expect(store.usage.inputTokens).toBe(150);
      expect(store.model).toBe("claude-opus-4-6");
      // But phase stays unchanged
      expect(store.phase).toBe("spawning");
    });
  });

  // ── AskUserQuestion flow ──

  describe("AskUserQuestion tool", () => {
    beforeEach(() => {
      store.run = makeRun("run-5");
      store.phase = "running";
      store.applyEventBatch(askUserQuestionEvents as BusEvent[]);
    });

    it("initially sets AskUserQuestion tool to ask_pending", () => {
      // After tool_end with AskUserQuestion, the tool should be ask_pending
      // But then user_message resolves it to success
      const toolEntry = store.timeline.find(
        (e) => e.kind === "tool" && e.id === "ask-1",
      ) as Extract<TimelineEntry, { kind: "tool" }>;
      expect(toolEntry).toBeDefined();
      // After replay with user_message following, it should be resolved
      expect(toolEntry.tool.status).toBe("success");
      expect(toolEntry.tool.output).toEqual({ answer: "app.js" });
    });

    it("has user answer in timeline", () => {
      const users = store.timeline.filter((e) => e.kind === "user");
      expect(users).toHaveLength(2); // "Create a file" + "app.js"
      expect(users[1].content).toBe("app.js");
    });
  });

  // ── Deduplication ──

  describe("deduplication", () => {
    it("does not duplicate message_complete on double replay", () => {
      store.run = makeRun("run-1");
      store.phase = "running";
      store.applyEventBatch(simpleChatEvents as BusEvent[]);
      // Replay same events again
      store.applyEventBatch(simpleChatEvents as BusEvent[]);

      const assistants = store.timeline.filter((e) => e.kind === "assistant");
      expect(assistants).toHaveLength(1); // Not 2
    });

    it("does not duplicate tool_start on double replay", () => {
      store.run = makeRun("run-2");
      store.phase = "running";
      store.applyEventBatch(chatWithToolsEvents as BusEvent[]);
      store.applyEventBatch(chatWithToolsEvents as BusEvent[]);

      const tools = store.timeline.filter((e) => e.kind === "tool");
      expect(tools).toHaveLength(2); // Not 4
    });
  });

  // ── applyEvent (single live event) ──

  describe("applyEvent (single live event)", () => {
    it("drops events for wrong run_id", () => {
      store.run = makeRun("run-1");
      store.phase = "running";
      const ev: BusEvent = {
        type: "message_complete",
        run_id: "wrong-run",
        message_id: "m1",
        text: "nope",
      };
      store.applyEvent(ev);
      expect(store.timeline).toHaveLength(0);
    });

    it("applies event for matching run_id", () => {
      store.run = makeRun("run-1");
      store.phase = "running";
      const ev: BusEvent = {
        type: "message_complete",
        run_id: "run-1",
        message_id: "m1",
        text: "hello",
      };
      store.applyEvent(ev);
      expect(store.timeline).toHaveLength(1);
      expect(store.timeline[0].content).toBe("hello");
    });

    it("accumulates message_delta streaming text", () => {
      store.run = makeRun("run-1");
      store.phase = "running";
      store.applyEvent({
        type: "message_delta",
        run_id: "run-1",
        text: "hel",
      });
      store.applyEvent({
        type: "message_delta",
        run_id: "run-1",
        text: "lo",
      });
      expect(store.streamingText).toBe("hello");
    });

    it("clears streaming text on message_complete", () => {
      store.run = makeRun("run-1");
      store.phase = "running";
      store.applyEvent({
        type: "message_delta",
        run_id: "run-1",
        text: "streaming...",
      });
      store.applyEvent({
        type: "message_complete",
        run_id: "run-1",
        message_id: "m1",
        text: "final text",
      });
      expect(store.streamingText).toBe("");
    });
  });

  // ── Raw event ──

  describe("raw event handling", () => {
    it("adds raw claude_stdout_text to timeline", () => {
      store.run = makeRun("run-1");
      store.phase = "running";
      store.applyEvent({
        type: "raw",
        run_id: "run-1",
        source: "claude_stdout_text",
        data: { text: "raw output" } as unknown as Record<string, unknown>,
      });
      expect(store.timeline).toHaveLength(1);
      expect(store.timeline[0].kind).toBe("assistant");
      expect(store.timeline[0].content).toContain("claude_stdout_text");
    });

    it("ignores raw events from non-claude sources", () => {
      store.run = makeRun("run-1");
      store.phase = "running";
      store.applyEvent({
        type: "raw",
        run_id: "run-1",
        source: "internal",
        data: { something: true } as unknown as Record<string, unknown>,
      });
      expect(store.timeline).toHaveLength(0);
    });
  });

  // ── Reset ──

  describe("reset", () => {
    it("clears all state", () => {
      store.run = makeRun("run-1");
      store.phase = "running";
      store.applyEventBatch(simpleChatEvents as BusEvent[]);
      expect(store.timeline.length).toBeGreaterThan(0);

      store.reset();

      expect(store.phase).toBe("empty");
      expect(store.run).toBeNull();
      expect(store.timeline).toHaveLength(0);
      expect(store.streamingText).toBe("");
      expect(store.tools).toHaveLength(0);
      expect(store.turnUsages).toHaveLength(0);
      expect(store.usage.inputTokens).toBe(0);
      expect(store.model).toBe("");
      expect(store.error).toBe("");
    });
  });

  // ── Derived getters ──

  describe("derived getters", () => {
    it("isRunning is true for active phases", () => {
      store.phase = "running";
      expect(store.isRunning).toBe(true);
      store.phase = "spawning";
      expect(store.isRunning).toBe(true);
      store.phase = "idle";
      expect(store.isRunning).toBe(false);
    });

    it("sessionAlive includes idle", () => {
      store.phase = "idle";
      expect(store.sessionAlive).toBe(true);
      store.phase = "running";
      expect(store.sessionAlive).toBe(true);
      store.phase = "completed";
      expect(store.sessionAlive).toBe(false);
    });

    it("canSend includes empty, ready, idle", () => {
      store.phase = "empty";
      expect(store.canSend).toBe(true);
      store.phase = "ready";
      expect(store.canSend).toBe(true);
      store.phase = "idle";
      expect(store.canSend).toBe(true);
      store.phase = "running";
      expect(store.canSend).toBe(false);
    });

    it("totalTokens sums input + output + cache", () => {
      store.usage = {
        inputTokens: 10,
        outputTokens: 50,
        cacheReadTokens: 800,
        cacheWriteTokens: 40,
        cost: 0.01,
      };
      expect(store.totalTokens).toBe(900);
    });
  });

  // ── Terminal run ask_pending resolution ──

  describe("terminal run ask_pending cleanup", () => {
    it("resolves ask_pending tools when run is terminal (replayOnly)", () => {
      // Simulate loadRun for a stopped run: phase is set to "stopped" from run.status,
      // then events are replayed with replayOnly=true (terminal runs don't let historical
      // run_state events overwrite the phase).
      const events: BusEvent[] = [
        { type: "user_message", run_id: "run-6", text: "Do it" },
        {
          type: "run_state",
          run_id: "run-6",
          state: "running",
          error: null,
          exit_code: null,
        } as BusEvent,
        {
          type: "tool_start",
          run_id: "run-6",
          tool_use_id: "ask-2",
          tool_name: "AskUserQuestion",
          input: { question: "Which?" },
        },
        {
          type: "tool_end",
          run_id: "run-6",
          tool_use_id: "ask-2",
          tool_name: "AskUserQuestion",
          output: { error: "auto-failed" },
          status: "error",
        },
        {
          type: "run_state",
          run_id: "run-6",
          state: "idle",
          error: null,
          exit_code: null,
        } as BusEvent,
        // Session ends without user answering
      ];

      store.run = makeRun("run-6", { status: "stopped" });
      store.phase = "stopped";
      store.applyEventBatch(events as BusEvent[], { replayOnly: true });

      // Phase stays "stopped" — replayOnly prevents historical run_state from changing it
      expect(store.phase).toBe("stopped");

      const toolEntry = store.timeline.find(
        (e) => e.kind === "tool" && e.id === "ask-2",
      ) as Extract<TimelineEntry, { kind: "tool" }>;
      expect(toolEntry).toBeDefined();
      expect(toolEntry.tool.status).toBe("error");
      expect(toolEntry.tool.output).toEqual({ error: "Session ended" });
    });

    it("resolves running tools nested in subTimeline (subagent case)", () => {
      const events: BusEvent[] = [
        { type: "user_message", run_id: "run-7", text: "Do it" },
        {
          type: "run_state",
          run_id: "run-7",
          state: "running",
          error: null,
          exit_code: null,
        } as BusEvent,
        // Parent Task tool starts
        {
          type: "tool_start",
          run_id: "run-7",
          tool_use_id: "task-1",
          tool_name: "Task",
          input: { prompt: "do stuff" },
        },
        // Child Bash tool starts inside the Task (subagent)
        {
          type: "tool_start",
          run_id: "run-7",
          tool_use_id: "bash-1",
          tool_name: "Bash",
          input: { command: "ls" },
          parent_tool_use_id: "task-1",
        },
        // Another child tool starts
        {
          type: "tool_start",
          run_id: "run-7",
          tool_use_id: "bash-2",
          tool_name: "Bash",
          input: { command: "echo hi" },
          parent_tool_use_id: "task-1",
        },
        // Session ends without tools completing
      ];

      store.run = makeRun("run-7", { status: "stopped" });
      store.phase = "stopped";
      store.applyEventBatch(events as BusEvent[], { replayOnly: true });

      // Parent Task tool should be finalized
      const taskEntry = store.timeline.find(
        (e) => e.kind === "tool" && e.id === "task-1",
      ) as Extract<TimelineEntry, { kind: "tool" }>;
      expect(taskEntry).toBeDefined();
      expect(taskEntry.tool.status).toBe("error");

      // Children in subTimeline should also be finalized
      expect(taskEntry.subTimeline).toBeDefined();
      expect(taskEntry.subTimeline!.length).toBe(2);
      for (const child of taskEntry.subTimeline!) {
        if (child.kind === "tool") {
          expect(child.tool.status).toBe("error");
          expect(child.tool.output).toEqual({ error: "Session ended" });
        }
      }
    });

    it("permission_prompt with missing parent_tool_use_id updates subTimeline instead of creating duplicate", () => {
      const events: BusEvent[] = [
        { type: "user_message", run_id: "run-8", text: "Do it" },
        {
          type: "run_state",
          run_id: "run-8",
          state: "running",
          error: null,
          exit_code: null,
        } as BusEvent,
        // Parent Task tool starts
        {
          type: "tool_start",
          run_id: "run-8",
          tool_use_id: "task-parent",
          tool_name: "Task",
          input: { prompt: "do stuff" },
        },
        // Child Bash tool starts inside the Task (subagent)
        {
          type: "tool_start",
          run_id: "run-8",
          tool_use_id: "bash-child",
          tool_name: "Bash",
          input: { command: "rm -rf /" },
          parent_tool_use_id: "task-parent",
        },
        // permission_prompt for bash-child BUT with parent_tool_use_id missing (CLI bug)
        {
          type: "permission_prompt",
          run_id: "run-8",
          tool_use_id: "bash-child",
          tool_name: "Bash",
          tool_input: { command: "rm -rf /" },
          request_id: "req-1",
          // NO parent_tool_use_id — this is the CLI bug
        },
      ];

      store.run = makeRun("run-8", { status: "running" });
      store.phase = "running";
      store.applyEventBatch(events as BusEvent[], { replayOnly: false });

      // The tool should NOT appear in the main timeline (only the Task parent should be there)
      const mainToolEntries = store.timeline.filter(
        (e) => e.kind === "tool" && e.id === "bash-child",
      );
      expect(mainToolEntries).toHaveLength(0); // No duplicate in main timeline

      // The tool should be updated in the subTimeline
      const taskEntry = store.timeline.find(
        (e) => e.kind === "tool" && e.id === "task-parent",
      ) as Extract<TimelineEntry, { kind: "tool" }>;
      expect(taskEntry).toBeDefined();
      expect(taskEntry.subTimeline).toBeDefined();
      const bashInSub = taskEntry.subTimeline!.find(
        (e) => e.kind === "tool" && e.id === "bash-child",
      ) as Extract<TimelineEntry, { kind: "tool" }>;
      expect(bashInSub).toBeDefined();
      expect(bashInSub.tool.status).toBe("permission_prompt");
      expect(bashInSub.tool.permission_request_id).toBe("req-1");
    });

    it("permission_denied with missing parent_tool_use_id updates subTimeline", () => {
      const events: BusEvent[] = [
        { type: "user_message", run_id: "run-9", text: "Do it" },
        {
          type: "run_state",
          run_id: "run-9",
          state: "running",
          error: null,
          exit_code: null,
        } as BusEvent,
        // Parent Task tool starts
        {
          type: "tool_start",
          run_id: "run-9",
          tool_use_id: "task-p2",
          tool_name: "Task",
          input: { prompt: "do stuff" },
        },
        // Child tool starts inside the Task
        {
          type: "tool_start",
          run_id: "run-9",
          tool_use_id: "bash-c2",
          tool_name: "Bash",
          input: { command: "rm -rf /" },
          parent_tool_use_id: "task-p2",
        },
        // permission_denied for bash-c2 BUT with parent_tool_use_id missing
        {
          type: "permission_denied",
          run_id: "run-9",
          tool_use_id: "bash-c2",
          // NO parent_tool_use_id
        },
      ];

      store.run = makeRun("run-9", { status: "running" });
      store.phase = "running";
      store.applyEventBatch(events as BusEvent[], { replayOnly: false });

      // Should be updated in subTimeline, not in main timeline
      const taskEntry = store.timeline.find(
        (e) => e.kind === "tool" && e.id === "task-p2",
      ) as Extract<TimelineEntry, { kind: "tool" }>;
      expect(taskEntry).toBeDefined();
      const bashInSub = taskEntry.subTimeline!.find(
        (e) => e.kind === "tool" && e.id === "bash-c2",
      ) as Extract<TimelineEntry, { kind: "tool" }>;
      expect(bashInSub).toBeDefined();
      expect(bashInSub.tool.status).toBe("permission_denied");
    });
  });

  // ── Transition guard ──

  describe("assertTransition", () => {
    it("allows valid transitions silently", () => {
      assertTransition("empty", "loading");
      assertTransition("loading", "running");
      assertTransition("running", "idle");
      assertTransition("idle", "running");
      assertTransition("running", "completed");
      assertTransition("completed", "empty");
      expect(warnSpy).not.toHaveBeenCalled();
    });

    it("warns on invalid transitions", () => {
      assertTransition("completed", "idle"); // invalid
      expect(warnSpy).toHaveBeenCalledTimes(1);
      expect(warnSpy.mock.calls[0][0]).toContain("completed → idle");
      warnSpy.mockClear(); // clear so afterEach doesn't fail
    });

    it("silently allows identity transitions", () => {
      assertTransition("running", "running");
      expect(warnSpy).not.toHaveBeenCalled();
    });
  });

  // ── Unknown event warning ──

  describe("unknown event type warning", () => {
    it("calls dbgWarn for unknown event types", async () => {
      const { dbgWarn: mockDbgWarn } = (await import("$lib/utils/debug")) as {
        dbgWarn: ReturnType<typeof vi.fn>;
      };
      mockDbgWarn.mockClear();

      store.run = makeRun("run-1");
      store.phase = "running";
      store.applyEvent({
        type: "some_future_event" as BusEvent["type"],
        run_id: "run-1",
      } as BusEvent);

      expect(mockDbgWarn).toHaveBeenCalledWith(
        "store",
        "unknown bus event type:",
        "some_future_event",
      );
    });
  });

  // ── Bad/out-of-order/missing event scenarios ──

  describe("malformed event sequences", () => {
    it("tool_end before tool_start: no crash, tool not in timeline", () => {
      store.run = makeRun("run-1");
      store.phase = "running";
      // tool_end arrives without a preceding tool_start
      store.applyEvent({
        type: "tool_end",
        run_id: "run-1",
        tool_use_id: "orphan-1",
        tool_name: "Bash",
        output: { result: "ok" },
        status: "success",
      });
      // No tool entry in timeline (tool_start never created it)
      const tools = store.timeline.filter((e) => e.kind === "tool");
      expect(tools).toHaveLength(0);
      // But no crash either
    });

    it("missing session_init: timeline still builds, model stays empty", () => {
      store.run = makeRun("run-1");
      store.phase = "running";
      const events: BusEvent[] = [
        // No session_init — jump straight to running + message
        {
          type: "run_state",
          run_id: "run-1",
          state: "running",
          error: null,
          exit_code: null,
        } as BusEvent,
        { type: "message_complete", run_id: "run-1", message_id: "m1", text: "Response" },
        {
          type: "run_state",
          run_id: "run-1",
          state: "idle",
          error: null,
          exit_code: null,
        } as BusEvent,
      ];
      store.applyEventBatch(events as BusEvent[]);
      expect(store.timeline).toHaveLength(1);
      expect(store.model).toBe(""); // no session_init → model stays empty
      expect(store.phase).toBe("idle");
    });

    it("duplicate run_state events: no phase corruption", () => {
      store.run = makeRun("run-1");
      store.phase = "running";
      const events: BusEvent[] = [
        {
          type: "run_state",
          run_id: "run-1",
          state: "running",
          error: null,
          exit_code: null,
        } as BusEvent,
        {
          type: "run_state",
          run_id: "run-1",
          state: "running",
          error: null,
          exit_code: null,
        } as BusEvent,
        {
          type: "run_state",
          run_id: "run-1",
          state: "idle",
          error: null,
          exit_code: null,
        } as BusEvent,
        {
          type: "run_state",
          run_id: "run-1",
          state: "idle",
          error: null,
          exit_code: null,
        } as BusEvent,
      ];
      store.applyEventBatch(events as BusEvent[]);
      expect(store.phase).toBe("idle"); // settles at idle, no corruption
    });

    it("message_delta without message_complete: streaming text accumulates", () => {
      store.run = makeRun("run-1");
      store.phase = "running";
      const events: BusEvent[] = [
        { type: "message_delta", run_id: "run-1", text: "partial " },
        { type: "message_delta", run_id: "run-1", text: "response" },
        // No message_complete — interrupted mid-stream
        {
          type: "run_state",
          run_id: "run-1",
          state: "failed",
          error: "interrupted",
          exit_code: null,
        } as BusEvent,
      ];
      store.applyEventBatch(events as BusEvent[]);
      // Streaming text stays (no message_complete to clear it)
      expect(store.streamingText).toBe("partial response");
      // No assistant entry in timeline (message_complete never came)
      expect(store.timeline.filter((e) => e.kind === "assistant")).toHaveLength(0);
      expect(store.phase).toBe("failed");
    });

    it("tool_start with duplicate tool_use_id: second is ignored", () => {
      store.run = makeRun("run-1");
      store.phase = "running";
      const events: BusEvent[] = [
        {
          type: "tool_start",
          run_id: "run-1",
          tool_use_id: "dup-1",
          tool_name: "Read",
          input: { path: "/a" },
        },
        {
          type: "tool_start",
          run_id: "run-1",
          tool_use_id: "dup-1",
          tool_name: "Read",
          input: { path: "/b" },
        },
        {
          type: "tool_end",
          run_id: "run-1",
          tool_use_id: "dup-1",
          tool_name: "Read",
          output: { ok: true },
          status: "success",
        },
      ];
      store.applyEventBatch(events as BusEvent[]);
      const tools = store.timeline.filter((e) => e.kind === "tool");
      expect(tools).toHaveLength(1); // dedup — only first tool_start counted
      expect((tools[0] as Extract<TimelineEntry, { kind: "tool" }>).tool.input).toEqual({
        path: "/a",
      });
    });

    it("empty event batch: no state change", () => {
      store.run = makeRun("run-1");
      store.phase = "running";
      store.applyEventBatch([]);
      expect(store.phase).toBe("running");
      expect(store.timeline).toHaveLength(0);
    });

    it("tool_start with empty tool_use_id: no crash, tool appears in timeline", () => {
      store.run = makeRun("run-1");
      store.phase = "running";
      expect(() =>
        store.applyEvent({
          type: "tool_start",
          run_id: "run-1",
          tool_use_id: "",
          tool_name: "Bash",
          input: { cmd: "ls" },
        }),
      ).not.toThrow();
      // Empty tool_use_id still creates a timeline entry (frontend tolerates it)
      const tools = store.timeline.filter((e) => e.kind === "tool");
      expect(tools).toHaveLength(1);
    });

    it("raw claude_stderr: appears in timeline", () => {
      store.run = makeRun("run-1");
      store.phase = "running";
      store.applyEvent({
        type: "raw",
        run_id: "run-1",
        source: "claude_stderr",
        data: { text: "error msg" } as unknown as Record<string, unknown>,
      });
      expect(store.timeline).toHaveLength(1);
      expect(store.timeline[0].kind).toBe("assistant");
      expect(store.timeline[0].content).toContain("claude_stderr");
    });

    it("raw unknown source: silently ignored, no crash", () => {
      store.run = makeRun("run-1");
      store.phase = "running";
      store.applyEvent({
        type: "raw",
        run_id: "run-1",
        source: "claude_some_new_feature",
        data: {} as unknown as Record<string, unknown>,
      });
      // Unknown raw source should not add to timeline and should not crash
      expect(store.timeline).toHaveLength(0);
    });
  });

  // ── Terminal run replay with replayOnly ──

  describe("terminal run replay (loadRun pattern)", () => {
    it("replays stopped run without phase corruption", () => {
      // Simulates loadRun: phase set from run.status, then replayOnly
      store.run = makeRun("run-1", { status: "stopped" });
      store.phase = "stopped";
      store.applyEventBatch(simpleChatEvents as BusEvent[], { replayOnly: true });

      // Phase stays stopped (not overwritten by historical run_state: idle)
      expect(store.phase).toBe("stopped");
      // But timeline is populated
      expect(store.timeline).toHaveLength(2);
      expect(store.model).toBe("claude-opus-4-6");
    });

    it("replays completed run without phase corruption", () => {
      store.run = makeRun("run-1", { status: "completed" });
      store.phase = "completed";
      store.applyEventBatch(simpleChatEvents as BusEvent[], { replayOnly: true });

      expect(store.phase).toBe("completed");
      expect(store.timeline).toHaveLength(2);
    });

    it("replays failed run: error not set, timeline populated", () => {
      store.run = makeRun("run-4", { status: "failed" });
      store.phase = "failed";
      store.applyEventBatch(sessionFailedEvents as BusEvent[], { replayOnly: true });

      expect(store.phase).toBe("failed");
      expect(store.error).toBe(""); // replayOnly skips error
      expect(store.timeline).toHaveLength(2);
    });
  });

  // ── result_subtype error flows ──

  describe("result_subtype", () => {
    it("error flows through to phase and error state", () => {
      store.run = makeRun("run-err-1");
      store.phase = "running";
      store.applyEventBatch(resultErrorMaxTurnsEvents as BusEvent[]);

      expect(store.phase).toBe("failed");
      expect(store.error).toBe("Max turns reached");
      expect(store.timeline).toHaveLength(2); // user + assistant
    });

    it("captures usage before failure", () => {
      store.run = makeRun("run-err-1");
      store.phase = "running";
      store.applyEventBatch(resultErrorMaxTurnsEvents as BusEvent[]);

      expect(store.usage.inputTokens).toBe(50000);
      expect(store.usage.outputTokens).toBe(8000);
      expect(store.usage.cost).toBe(0.25);
    });

    it("sets model from session_init even on error runs", () => {
      store.run = makeRun("run-err-1");
      store.phase = "running";
      store.applyEventBatch(resultErrorMaxTurnsEvents as BusEvent[]);

      expect(store.model).toBe("claude-sonnet-4-5-20250929");
    });
  });

  // ── compact_boundary handling ──

  describe("compact_boundary", () => {
    it("adds compaction notice to timeline", () => {
      store.run = makeRun("run-cb-1");
      store.phase = "running";
      store.applyEventBatch(compactBoundaryEvents as BusEvent[]);

      const compactEntries = store.timeline.filter(
        (e) => e.kind === "separator" && e.content.includes("Context compacted"),
      );
      expect(compactEntries).toHaveLength(1);
      expect(compactEntries[0].content).toContain("180k tokens");
    });

    it("preserves surrounding messages", () => {
      store.run = makeRun("run-cb-1");
      store.phase = "running";
      store.applyEventBatch(compactBoundaryEvents as BusEvent[]);

      // user + assistant("Hello!...") + separator(compact_boundary) + assistant("Continuing...")
      expect(store.timeline).toHaveLength(4);
      expect(store.timeline[0].kind).toBe("user");
      expect(store.timeline[1].kind).toBe("assistant");
      expect(store.timeline[1].content).toContain("Hello!");
      expect(store.timeline[2].kind).toBe("separator");
      expect(store.timeline[2].content).toContain("Context compacted");
      expect(store.timeline[3].kind).toBe("assistant");
      expect(store.timeline[3].content).toContain("Continuing");
    });

    it("ends at idle phase", () => {
      store.run = makeRun("run-cb-1");
      store.phase = "running";
      store.applyEventBatch(compactBoundaryEvents as BusEvent[]);

      expect(store.phase).toBe("idle");
    });
  });

  // ── getResumeWarning ──

  describe("getResumeWarning", () => {
    it("returns warning for error_input_too_long subtype", () => {
      const run = { error_message: "Some error", result_subtype: "error_input_too_long" };
      const warning = getResumeWarning(run);
      expect(warning).not.toBeNull();
      expect(warning).toContain("Fork");
    });

    it("returns warning for error_max_turns subtype", () => {
      const run = { error_message: "Max turns", result_subtype: "error_max_turns" };
      const warning = getResumeWarning(run);
      expect(warning).not.toBeNull();
    });

    it("returns warning for context-full error message patterns", () => {
      const patterns = [
        "Input is too long for this model",
        "The prompt is too long",
        "Too many tokens in context",
        "Exceeded context window limit",
      ];
      for (const msg of patterns) {
        const run = { error_message: msg };
        const warning = getResumeWarning(run);
        expect(warning).not.toBeNull();
      }
    });

    it("returns null for normal error messages", () => {
      const run = { error_message: "Process exited with code 1" };
      expect(getResumeWarning(run)).toBeNull();
    });

    it("returns null for null run", () => {
      expect(getResumeWarning(null)).toBeNull();
    });

    it("returns null when no error", () => {
      const run = { error_message: undefined, result_subtype: undefined };
      expect(getResumeWarning(run)).toBeNull();
    });
  });

  // ── classifyError ──

  describe("classifyError", () => {
    it("classifies context_limit by subtype prefix", () => {
      expect(classifyError("error_input_too_long").category).toBe("context_limit");
      expect(classifyError("error_max_turns").category).toBe("context_limit");
      expect(classifyError("error_input_too_long").canFork).toBe(true);
      expect(classifyError("error_input_too_long").canRetry).toBe(false);
    });

    it("classifies budget_limit", () => {
      const c = classifyError("error_max_budget");
      expect(c.category).toBe("budget_limit");
      expect(c.canRetry).toBe(false);
      expect(c.canFork).toBe(false);
      expect(c.settingsLink).toBe("/settings");
    });

    it("classifies auth_issue", () => {
      expect(classifyError("error_api_key_invalid").category).toBe("auth_issue");
      expect(classifyError("error_auth_failed").category).toBe("auth_issue");
      expect(classifyError("error_api_key_invalid").settingsLink).toBe("/settings");
    });

    it("classifies server_issue", () => {
      expect(classifyError("error_rate_limit").category).toBe("server_issue");
      expect(classifyError("error_overloaded").category).toBe("server_issue");
      expect(classifyError("error_model_unavailable").category).toBe("server_issue");
      expect(classifyError("error_timeout").category).toBe("server_issue");
      expect(classifyError("error_network_error").category).toBe("server_issue");
      expect(classifyError("error_rate_limit").canRetry).toBe(true);
    });

    it("classifies tool_issue", () => {
      expect(classifyError("error_permission_denied").category).toBe("tool_issue");
      expect(classifyError("error_tool_execution").category).toBe("tool_issue");
      expect(classifyError("error_structured_output_retries").category).toBe("tool_issue");
    });

    it("classifies unknown error subtypes as unknown", () => {
      const c = classifyError("error_some_future_type");
      expect(c.category).toBe("unknown");
      expect(c.canRetry).toBe(true);
    });

    it("falls back to text matching when no subtype", () => {
      expect(classifyError(undefined, "Input is too long for this model").category).toBe(
        "context_limit",
      );
      expect(classifyError(undefined, "API key is invalid").category).toBe("auth_issue");
      expect(classifyError(undefined, "rate limit exceeded").category).toBe("server_issue");
      expect(classifyError(undefined, "max_budget reached").category).toBe("budget_limit");
    });

    it("returns unknown for unrecognized error messages", () => {
      expect(classifyError(undefined, "Process exited with code 1").category).toBe("unknown");
    });

    it("returns unknown for empty inputs", () => {
      expect(classifyError().category).toBe("unknown");
      expect(classifyError("", "").category).toBe("unknown");
    });
  });

  // ── taskNotifications Map ──

  describe("taskNotifications Map", () => {
    beforeEach(() => {
      store.run = makeRun("run-tasks");
      store.phase = "running";
    });

    it("upserts task notifications by task_id", () => {
      store.applyEventBatch([
        {
          type: "task_notification",
          run_id: "run-tasks",
          task_id: "indexing",
          status: "started",
          data: { message: "Indexing files..." },
        } as unknown as BusEvent,
        {
          type: "task_notification",
          run_id: "run-tasks",
          task_id: "indexing",
          status: "completed",
          data: { message: "Indexing complete" },
        } as unknown as BusEvent,
      ]);
      // Should have 1 entry (upserted), not 2
      expect(store.taskNotifications.size).toBe(1);
      const item = store.taskNotifications.get("indexing");
      expect(item!.status).toBe("completed");
    });

    it("activeBackgroundTasks filters completed/failed", () => {
      store.applyEventBatch([
        {
          type: "task_notification",
          run_id: "run-tasks",
          task_id: "t1",
          status: "started",
          data: { message: "Task 1" },
        } as unknown as BusEvent,
        {
          type: "task_notification",
          run_id: "run-tasks",
          task_id: "t2",
          status: "completed",
          data: { message: "Task 2" },
        } as unknown as BusEvent,
        {
          type: "task_notification",
          run_id: "run-tasks",
          task_id: "t3",
          status: "failed",
          data: { message: "Task 3" },
        } as unknown as BusEvent,
      ]);
      expect(store.activeBackgroundTasks).toHaveLength(1);
      expect(store.activeBackgroundTasks[0].task_id).toBe("t1");
    });

    it("_clearContentState resets taskNotifications", () => {
      store.applyEventBatch([
        {
          type: "task_notification",
          run_id: "run-tasks",
          task_id: "t1",
          status: "started",
          data: { message: "Task 1" },
        } as unknown as BusEvent,
      ]);
      expect(store.taskNotifications.size).toBe(1);
      store.reset();
      expect(store.taskNotifications.size).toBe(0);
    });

    it("hasBackgroundTasks reflects Map size", () => {
      expect(store.hasBackgroundTasks).toBe(false);
      store.applyEventBatch([
        {
          type: "task_notification",
          run_id: "run-tasks",
          task_id: "t1",
          status: "started",
          data: { message: "Task 1" },
        } as unknown as BusEvent,
      ]);
      expect(store.hasBackgroundTasks).toBe(true);
    });

    it("extracts output_file from snake_case data", () => {
      store.applyEventBatch([
        {
          type: "task_notification",
          run_id: "run-tasks",
          task_id: "t-out",
          status: "started",
          data: { message: "bg", output_file: "/tmp/x.output", task_type: "shell" },
        } as unknown as BusEvent,
      ]);
      const item = store.taskNotifications.get("t-out")!;
      expect(item.output_file).toBe("/tmp/x.output");
      expect(item.task_type).toBe("shell");
    });

    it("extracts output_file from camelCase data", () => {
      store.applyEventBatch([
        {
          type: "task_notification",
          run_id: "run-tasks",
          task_id: "t-camel",
          status: "started",
          data: { message: "bg", outputFile: "/tmp/y.output", taskType: "agent" },
        } as unknown as BusEvent,
      ]);
      const item = store.taskNotifications.get("t-camel")!;
      expect(item.output_file).toBe("/tmp/y.output");
      expect(item.task_type).toBe("agent");
    });

    it("preserves output_file across status updates", () => {
      store.applyEventBatch([
        {
          type: "task_notification",
          run_id: "run-tasks",
          task_id: "t-persist",
          status: "started",
          data: { message: "running", output_file: "/tmp/z.output", summary: "Building" },
        } as unknown as BusEvent,
      ]);
      // Second update without output_file — should preserve from previous
      store.applyEventBatch([
        {
          type: "task_notification",
          run_id: "run-tasks",
          task_id: "t-persist",
          status: "completed",
          data: { message: "done" },
        } as unknown as BusEvent,
      ]);
      const item = store.taskNotifications.get("t-persist")!;
      expect(item.status).toBe("completed");
      expect(item.output_file).toBe("/tmp/z.output");
      expect(item.summary).toBe("Building");
    });

    it("extracts summary and tool_use_id", () => {
      store.applyEventBatch([
        {
          type: "task_notification",
          run_id: "run-tasks",
          task_id: "t-full",
          status: "started",
          data: {
            message: "task",
            summary: "Running tests",
            tool_use_id: "tu-123",
          },
        } as unknown as BusEvent,
      ]);
      const item = store.taskNotifications.get("t-full")!;
      expect(item.summary).toBe("Running tests");
      expect(item.tool_use_id).toBe("tu-123");
    });
  });

  // ── Subagent (parent_tool_use_id) tracking ──

  describe("subagent tracking", () => {
    beforeEach(() => {
      store.run = makeRun("run-sub");
      store.phase = "running";
      store.applyEventBatch(subagentTaskEvents as BusEvent[]);
    });

    it("routes subagent events to parent tool subTimeline", () => {
      // Main timeline should NOT contain subagent tool or message
      const mainTools = store.timeline.filter((e) => e.kind === "tool");
      expect(mainTools).toHaveLength(1); // Only the parent Task tool
      expect((mainTools[0] as Extract<TimelineEntry, { kind: "tool" }>).tool.tool_name).toBe(
        "Task",
      );

      const mainAssistants = store.timeline.filter((e) => e.kind === "assistant");
      expect(mainAssistants).toHaveLength(1); // Only main agent's message_complete
      expect(mainAssistants[0].content).toBe("The subagent ran successfully and printed hello.");

      // Parent Task tool should have subTimeline with child entries
      const taskEntry = mainTools[0] as Extract<TimelineEntry, { kind: "tool" }>;
      expect(taskEntry.subTimeline).toBeDefined();
      expect(taskEntry.subTimeline!).toHaveLength(2); // Bash tool + assistant message

      // Verify subTimeline contents
      const subBash = taskEntry.subTimeline![0];
      expect(subBash.kind).toBe("tool");
      expect((subBash as Extract<TimelineEntry, { kind: "tool" }>).tool.tool_name).toBe("Bash");
      expect((subBash as Extract<TimelineEntry, { kind: "tool" }>).tool.status).toBe("success");
      expect((subBash as Extract<TimelineEntry, { kind: "tool" }>).tool.duration_ms).toBe(100);

      const subMsg = taskEntry.subTimeline![1];
      expect(subMsg.kind).toBe("assistant");
      expect(subMsg.content).toBe("The command ran successfully.");
    });

    it("discards subagent message_delta (does not affect streamingText)", () => {
      // Subagent message_delta with parent_tool_use_id should be discarded
      expect(store.streamingText).toBe("");
    });

    it("discards subagent thinking_delta (does not affect thinkingText)", () => {
      // Subagent thinking_delta with parent_tool_use_id should be discarded
      expect(store.thinkingText).toBe("");
    });

    it("deduplicates subagent tool_use_id via seenToolIds", () => {
      // Apply the same batch again — should not duplicate
      store.applyEventBatch(subagentTaskEvents as BusEvent[]);

      const mainTools = store.timeline.filter((e) => e.kind === "tool");
      expect(mainTools).toHaveLength(1); // Still only one Task tool

      const taskEntry = mainTools[0] as Extract<TimelineEntry, { kind: "tool" }>;
      expect(taskEntry.subTimeline!).toHaveLength(2); // No duplicates
    });

    it("does not add subTimeline to tools without subagent events", () => {
      // Load a fixture without subagent events
      const plainStore = new SessionStore();
      plainStore.run = makeRun("run-2");
      plainStore.phase = "running";
      plainStore.applyEventBatch(chatWithToolsEvents as BusEvent[]);

      const tools = plainStore.timeline.filter((e) => e.kind === "tool");
      for (const t of tools) {
        const toolEntry = t as Extract<TimelineEntry, { kind: "tool" }>;
        expect(toolEntry.subTimeline).toBeUndefined();
      }
    });

    it("parent Task tool resolves to success", () => {
      const taskEntry = store.timeline.find(
        (e) => e.kind === "tool" && e.id === "toolu_task_1",
      ) as Extract<TimelineEntry, { kind: "tool" }>;
      expect(taskEntry).toBeDefined();
      expect(taskEntry.tool.status).toBe("success");
      expect(taskEntry.tool.duration_ms).toBe(5000);
    });
  });

  // ── Subagent streaming deltas ──

  describe("subagent streaming deltas", () => {
    beforeEach(() => {
      store.run = makeRun("run-sub");
      store.phase = "running";
    });

    it("creates synthetic __sub_stream_ assistant entry on first message_delta", () => {
      const events: BusEvent[] = [
        {
          type: "tool_start",
          run_id: "run-sub",
          tool_use_id: "parent-1",
          tool_name: "Task",
          input: {},
        },
        { type: "message_delta", run_id: "run-sub", text: "Hello", parent_tool_use_id: "parent-1" },
      ];
      store.applyEventBatch(events);

      const parent = store.timeline.find(
        (e) => e.kind === "tool" && e.id === "parent-1",
      ) as Extract<TimelineEntry, { kind: "tool" }>;
      expect(parent.subTimeline).toHaveLength(1);
      expect(parent.subTimeline![0].kind).toBe("assistant");
      expect(parent.subTimeline![0].id).toBe("__sub_stream_parent-1");
      expect(parent.subTimeline![0].content).toBe("Hello");
    });

    it("accumulates message_delta text into synthetic entry content", () => {
      const events: BusEvent[] = [
        {
          type: "tool_start",
          run_id: "run-sub",
          tool_use_id: "parent-1",
          tool_name: "Task",
          input: {},
        },
        {
          type: "message_delta",
          run_id: "run-sub",
          text: "Hello ",
          parent_tool_use_id: "parent-1",
        },
        { type: "message_delta", run_id: "run-sub", text: "world", parent_tool_use_id: "parent-1" },
      ];
      store.applyEventBatch(events);

      const parent = store.timeline.find(
        (e) => e.kind === "tool" && e.id === "parent-1",
      ) as Extract<TimelineEntry, { kind: "tool" }>;
      expect(parent.subTimeline![0].content).toBe("Hello world");
    });

    it("accumulates thinking_delta text into synthetic entry thinkingText", () => {
      const events: BusEvent[] = [
        {
          type: "tool_start",
          run_id: "run-sub",
          tool_use_id: "parent-1",
          tool_name: "Task",
          input: {},
        },
        {
          type: "thinking_delta",
          run_id: "run-sub",
          text: "Let me ",
          parent_tool_use_id: "parent-1",
        },
        {
          type: "thinking_delta",
          run_id: "run-sub",
          text: "think...",
          parent_tool_use_id: "parent-1",
        },
      ];
      store.applyEventBatch(events);

      const parent = store.timeline.find(
        (e) => e.kind === "tool" && e.id === "parent-1",
      ) as Extract<TimelineEntry, { kind: "tool" }>;
      const synthetic = parent.subTimeline![0] as Extract<TimelineEntry, { kind: "assistant" }>;
      expect(synthetic.thinkingText).toBe("Let me think...");
      expect(synthetic.content).toBe(""); // content is empty when only thinking
    });

    it("removes synthetic entry and appends final assistant entry on message_complete", () => {
      const events: BusEvent[] = [
        {
          type: "tool_start",
          run_id: "run-sub",
          tool_use_id: "parent-1",
          tool_name: "Task",
          input: {},
        },
        {
          type: "message_delta",
          run_id: "run-sub",
          text: "streaming...",
          parent_tool_use_id: "parent-1",
        },
        {
          type: "message_complete",
          run_id: "run-sub",
          message_id: "msg-final",
          text: "Final answer",
          parent_tool_use_id: "parent-1",
        },
      ];
      store.applyEventBatch(events);

      const parent = store.timeline.find(
        (e) => e.kind === "tool" && e.id === "parent-1",
      ) as Extract<TimelineEntry, { kind: "tool" }>;
      expect(parent.subTimeline).toHaveLength(1);
      // Synthetic entry replaced by final message
      expect(parent.subTimeline![0].id).toBe("msg-final");
      expect(parent.subTimeline![0].content).toBe("Final answer");
    });

    it("handles concurrent children: each parent_tool_use_id gets its own synthetic entry", () => {
      const events: BusEvent[] = [
        {
          type: "tool_start",
          run_id: "run-sub",
          tool_use_id: "parent-a",
          tool_name: "Task",
          input: {},
        },
        {
          type: "tool_start",
          run_id: "run-sub",
          tool_use_id: "parent-b",
          tool_name: "Task",
          input: {},
        },
        {
          type: "message_delta",
          run_id: "run-sub",
          text: "from A",
          parent_tool_use_id: "parent-a",
        },
        {
          type: "message_delta",
          run_id: "run-sub",
          text: "from B",
          parent_tool_use_id: "parent-b",
        },
      ];
      store.applyEventBatch(events);

      const parentA = store.timeline.find(
        (e) => e.kind === "tool" && e.id === "parent-a",
      ) as Extract<TimelineEntry, { kind: "tool" }>;
      const parentB = store.timeline.find(
        (e) => e.kind === "tool" && e.id === "parent-b",
      ) as Extract<TimelineEntry, { kind: "tool" }>;
      expect(parentA.subTimeline).toHaveLength(1);
      expect(parentA.subTimeline![0].content).toBe("from A");
      expect(parentA.subTimeline![0].id).toBe("__sub_stream_parent-a");
      expect(parentB.subTimeline).toHaveLength(1);
      expect(parentB.subTimeline![0].content).toBe("from B");
      expect(parentB.subTimeline![0].id).toBe("__sub_stream_parent-b");
    });

    it("routes tool_input_delta with parent_tool_use_id to child tool _inputJsonAccum", () => {
      const events: BusEvent[] = [
        {
          type: "tool_start",
          run_id: "run-sub",
          tool_use_id: "parent-1",
          tool_name: "Task",
          input: {},
        },
        {
          type: "tool_start",
          run_id: "run-sub",
          tool_use_id: "child-1",
          tool_name: "Bash",
          input: {},
          parent_tool_use_id: "parent-1",
        },
        {
          type: "tool_input_delta",
          run_id: "run-sub",
          tool_use_id: "child-1",
          partial_json: '{"command":',
          parent_tool_use_id: "parent-1",
        },
        {
          type: "tool_input_delta",
          run_id: "run-sub",
          tool_use_id: "child-1",
          partial_json: '"ls"}',
          parent_tool_use_id: "parent-1",
        },
      ];
      store.applyEventBatch(events);

      const parent = store.timeline.find(
        (e) => e.kind === "tool" && e.id === "parent-1",
      ) as Extract<TimelineEntry, { kind: "tool" }>;
      const child = parent.subTimeline![0] as Extract<TimelineEntry, { kind: "tool" }>;
      expect(child.tool.input).toEqual({ command: "ls" });
      expect((child.tool as Record<string, unknown>)._inputJsonAccum).toBe('{"command":"ls"}');
    });
  });

  // ── Protocol extension events ──

  describe("protocol extension events", () => {
    beforeEach(() => {
      store.run = makeRun("run-1");
      store.phase = "running";
      store.applyEventBatch(protocolEvents as BusEvent[]);
    });

    it("system_status updates store field", () => {
      expect(store.systemStatus).toEqual({ status: "compacting" });
    });

    it("auth_status updates store field", () => {
      expect(store.authStatus).toEqual({ is_authenticating: true, output: ["Authenticating..."] });
    });

    it("hook_started/progress/response append to hookEvents", () => {
      // hook_started + hook_progress + hook_response + hook_callback = 4
      expect(store.hookEvents).toHaveLength(4);
      expect(store.hookEvents[0].type).toBe("hook_started");
      expect(store.hookEvents[0].hook_id).toBe("hook-1");
      expect(store.hookEvents[1].type).toBe("hook_progress");
      expect(store.hookEvents[2].type).toBe("hook_response");
      expect(store.hookEvents[3].type).toBe("hook_callback");
    });

    it("task_notification upserts into taskNotifications Map", () => {
      expect(store.taskNotifications.size).toBe(1);
      const item = store.taskNotifications.get("task-1");
      expect(item).toBeDefined();
      expect(item!.task_id).toBe("task-1");
      expect(item!.status).toBe("started");
    });

    it("files_persisted appends to persistedFiles", () => {
      expect(store.persistedFiles).toHaveLength(1);
      expect((store.persistedFiles[0] as Record<string, unknown>).filename).toBe("test.ts");
    });

    it("tool_progress updates timeline tool elapsed_time_seconds", () => {
      const toolEntry = store.timeline.find(
        (e) => e.kind === "tool" && e.id === "tool-1",
      ) as Extract<TimelineEntry, { kind: "tool" }>;
      expect(toolEntry).toBeDefined();
      // After tool_end, duration_ms is set; elapsed_time_seconds from tool_progress is also preserved
      expect(toolEntry.tool.elapsed_time_seconds).toBe(2.5);
    });

    it("tool_use_summary updates timeline tool summary", () => {
      const toolEntry = store.timeline.find(
        (e) => e.kind === "tool" && e.id === "tool-1",
      ) as Extract<TimelineEntry, { kind: "tool" }>;
      expect(toolEntry).toBeDefined();
      expect(toolEntry.tool.summary).toBe("Listed files in current directory");
    });

    it("builds correct timeline with new events mixed in", () => {
      // user_message + tool (start+end merged) + message_complete = 3 entries
      expect(store.timeline).toHaveLength(3);
      expect(store.timeline[0].kind).toBe("user");
      expect(store.timeline[1].kind).toBe("tool");
      expect(store.timeline[2].kind).toBe("assistant");
    });

    it("sets usage from usage_update", () => {
      expect(store.usage.inputTokens).toBe(200);
      expect(store.usage.outputTokens).toBe(50);
      expect(store.usage.cost).toBe(0.01);
    });

    it("ends at idle phase", () => {
      expect(store.phase).toBe("idle");
    });
  });

  describe("tool_progress with parent_tool_use_id", () => {
    it("updates subTimeline tool elapsed_time_seconds", () => {
      store.run = makeRun("run-1");
      store.phase = "running";
      // Set up a parent tool with a subTimeline tool
      const events: BusEvent[] = [
        {
          type: "tool_start",
          run_id: "run-1",
          tool_use_id: "parent-1",
          tool_name: "Task",
          input: {},
        },
        {
          type: "tool_start",
          run_id: "run-1",
          tool_use_id: "child-1",
          tool_name: "Bash",
          input: { command: "ls" },
          parent_tool_use_id: "parent-1",
        },
        {
          type: "tool_progress",
          run_id: "run-1",
          tool_use_id: "child-1",
          elapsed_time_seconds: 1.5,
          data: {},
          parent_tool_use_id: "parent-1",
        },
      ];
      store.applyEventBatch(events);

      const parentEntry = store.timeline.find(
        (e) => e.kind === "tool" && e.id === "parent-1",
      ) as Extract<TimelineEntry, { kind: "tool" }>;
      expect(parentEntry).toBeDefined();
      expect(parentEntry.subTimeline).toHaveLength(1);
      const child = parentEntry.subTimeline![0] as Extract<TimelineEntry, { kind: "tool" }>;
      expect(child.tool.elapsed_time_seconds).toBe(1.5);
    });
  });

  describe("tool_use_summary with parent_tool_use_id", () => {
    it("updates subTimeline tool summary", () => {
      store.run = makeRun("run-1");
      store.phase = "running";
      const events: BusEvent[] = [
        {
          type: "tool_start",
          run_id: "run-1",
          tool_use_id: "parent-1",
          tool_name: "Task",
          input: {},
        },
        {
          type: "tool_start",
          run_id: "run-1",
          tool_use_id: "child-1",
          tool_name: "Bash",
          input: { command: "ls" },
          parent_tool_use_id: "parent-1",
        },
        {
          type: "tool_end",
          run_id: "run-1",
          tool_use_id: "child-1",
          tool_name: "Bash",
          output: {},
          status: "success",
          parent_tool_use_id: "parent-1",
        },
        {
          type: "tool_use_summary",
          run_id: "run-1",
          tool_use_id: "child-1",
          summary: "Listed files",
          preceding_tool_use_ids: [],
          data: {},
          parent_tool_use_id: "parent-1",
        },
      ];
      store.applyEventBatch(events);

      const parentEntry = store.timeline.find(
        (e) => e.kind === "tool" && e.id === "parent-1",
      ) as Extract<TimelineEntry, { kind: "tool" }>;
      expect(parentEntry).toBeDefined();
      const child = parentEntry.subTimeline![0] as Extract<TimelineEntry, { kind: "tool" }>;
      expect(child.tool.summary).toBe("Listed files");
    });
  });

  describe("control_cancelled", () => {
    it("resolves permission_prompt tool card to error", () => {
      store.run = makeRun("run-1");
      store.phase = "running";
      const events: BusEvent[] = [
        {
          type: "tool_start",
          run_id: "run-1",
          tool_use_id: "tool-perm-1",
          tool_name: "Bash",
          input: { command: "rm -rf" },
        },
        {
          type: "permission_prompt",
          run_id: "run-1",
          request_id: "req-1",
          tool_name: "Bash",
          tool_use_id: "tool-perm-1",
          tool_input: { command: "rm -rf" },
          decision_reason: "dangerous",
        },
        { type: "control_cancelled", run_id: "run-1", request_id: "req-1" },
      ];
      store.applyEventBatch(events);

      const toolEntry = store.timeline.find(
        (e) => e.kind === "tool" && e.id === "tool-perm-1",
      ) as Extract<TimelineEntry, { kind: "tool" }>;
      expect(toolEntry).toBeDefined();
      expect(toolEntry.tool.status).toBe("error");
    });
  });

  describe("permission_prompt suggestions data chain", () => {
    it("permission_prompt merges suggestions into tool entry", () => {
      store.run = makeRun("run-1");
      store.phase = "running";
      const events: BusEvent[] = [
        {
          type: "tool_start",
          run_id: "run-1",
          tool_use_id: "tool-1",
          tool_name: "Bash",
          input: { command: "npm test" },
        },
        {
          type: "permission_prompt",
          run_id: "run-1",
          request_id: "req-1",
          tool_name: "Bash",
          tool_use_id: "tool-1",
          tool_input: { command: "npm test" },
          decision_reason: "needs approval",
          suggestions: [{ type: "addRules", rules: ["Bash(npm test)"], behavior: "allow" }],
        },
      ];
      store.applyEventBatch(events);
      const entry = store.timeline.find((e) => e.kind === "tool" && e.id === "tool-1") as Extract<
        TimelineEntry,
        { kind: "tool" }
      >;
      expect(entry.tool.status).toBe("permission_prompt");
      expect(entry.tool.suggestions).toEqual([
        { type: "addRules", rules: ["Bash(npm test)"], behavior: "allow" },
      ]);
    });

    it("permission_prompt merges suggestions in subTimeline (fallback path)", () => {
      const events: BusEvent[] = [
        { type: "user_message", run_id: "run-8", text: "Do it" },
        {
          type: "run_state",
          run_id: "run-8",
          state: "running",
          error: null,
          exit_code: null,
        } as BusEvent,
        {
          type: "tool_start",
          run_id: "run-8",
          tool_use_id: "task-parent",
          tool_name: "Task",
          input: { prompt: "do stuff" },
        },
        {
          type: "tool_start",
          run_id: "run-8",
          tool_use_id: "bash-child",
          tool_name: "Bash",
          input: { command: "rm -rf /" },
          parent_tool_use_id: "task-parent",
        },
        // permission_prompt with suggestions but WITHOUT parent_tool_use_id (fallback path)
        {
          type: "permission_prompt",
          run_id: "run-8",
          tool_use_id: "bash-child",
          tool_name: "Bash",
          tool_input: { command: "rm -rf /" },
          request_id: "req-1",
          decision_reason: "dangerous",
          suggestions: [{ type: "addRules", rules: ["Bash(rm -rf /)"], behavior: "allow" }],
        },
      ];
      store.run = makeRun("run-8", { status: "running" });
      store.phase = "running";
      store.applyEventBatch(events as BusEvent[], { replayOnly: false });

      const taskEntry = store.timeline.find(
        (e) => e.kind === "tool" && e.id === "task-parent",
      ) as Extract<TimelineEntry, { kind: "tool" }>;
      expect(taskEntry).toBeDefined();
      const bashInSub = taskEntry.subTimeline!.find(
        (e) => e.kind === "tool" && e.id === "bash-child",
      ) as Extract<TimelineEntry, { kind: "tool" }>;
      expect(bashInSub).toBeDefined();
      expect(bashInSub.tool.status).toBe("permission_prompt");
      expect(bashInSub.tool.suggestions).toEqual([
        { type: "addRules", rules: ["Bash(rm -rf /)"], behavior: "allow" },
      ]);
    });

    it("run_state idle resolves stale permission_prompt to error", () => {
      store.run = makeRun("run-1");
      store.phase = "running";
      const events: BusEvent[] = [
        {
          type: "tool_start",
          run_id: "run-1",
          tool_use_id: "tool-1",
          tool_name: "Bash",
          input: {},
        },
        {
          type: "permission_prompt",
          run_id: "run-1",
          request_id: "req-1",
          tool_name: "Bash",
          tool_use_id: "tool-1",
          tool_input: {},
          decision_reason: "",
        },
        {
          type: "run_state",
          run_id: "run-1",
          state: "idle",
        } as BusEvent,
      ];
      store.applyEventBatch(events);
      const entry = store.timeline.find((e) => e.kind === "tool" && e.id === "tool-1") as Extract<
        TimelineEntry,
        { kind: "tool" }
      >;
      expect(entry.tool.status).toBe("error");
    });
  });

  describe("unknown event type", () => {
    it("triggers dbgWarn", async () => {
      store.run = makeRun("run-1");
      store.phase = "running";
      store.applyEvent({ type: "totally_unknown_type", run_id: "run-1" } as unknown as BusEvent);
      // dbgWarn is mocked via vi.mock
      const { dbgWarn } = await import("$lib/utils/debug");
      expect(dbgWarn).toHaveBeenCalled();
      warnSpy.mockClear(); // Clear to avoid afterEach failure
    });
  });

  // ── Per-model usage (#4) ──

  describe("per-model usage (model_usage + duration_api_ms)", () => {
    it("stores modelUsage and durationApiMs from usage_update", () => {
      store.run = makeRun("run-1");
      store.phase = "running";
      const events: BusEvent[] = [
        {
          type: "usage_update",
          run_id: "run-1",
          input_tokens: 500,
          output_tokens: 100,
          cache_read_tokens: 50,
          cache_write_tokens: 10,
          total_cost_usd: 0.05,
          model_usage: {
            "claude-sonnet-4-5-20250929": {
              input_tokens: 400,
              output_tokens: 80,
              cache_read_tokens: 40,
              cache_write_tokens: 8,
              web_search_requests: 0,
              cost_usd: 0.03,
              context_window: 200000,
            },
            "claude-haiku-4-5-20251001": {
              input_tokens: 100,
              output_tokens: 20,
              cache_read_tokens: 10,
              cache_write_tokens: 2,
              web_search_requests: 0,
              cost_usd: 0.02,
            },
          },
          duration_api_ms: 12345,
        },
      ];
      store.applyEventBatch(events as BusEvent[]);

      expect(store.usage.modelUsage).toBeDefined();
      expect(Object.keys(store.usage.modelUsage!)).toHaveLength(2);
      expect(store.usage.modelUsage!["claude-sonnet-4-5-20250929"].cost_usd).toBe(0.03);
      expect(store.usage.modelUsage!["claude-haiku-4-5-20251001"].cost_usd).toBe(0.02);
      expect(store.usage.durationApiMs).toBe(12345);
    });

    it("zero-token guard preserves modelUsage and durationApiMs", () => {
      store.run = makeRun("run-1");
      store.phase = "running";
      const events: BusEvent[] = [
        // First: real usage with tokens
        {
          type: "usage_update",
          run_id: "run-1",
          input_tokens: 500,
          output_tokens: 100,
          total_cost_usd: 0.05,
        },
        // Second: zero-token error result with modelUsage
        {
          type: "usage_update",
          run_id: "run-1",
          input_tokens: 0,
          output_tokens: 0,
          total_cost_usd: 0.06,
          model_usage: {
            "claude-sonnet-4-5-20250929": {
              input_tokens: 500,
              output_tokens: 100,
              cache_read_tokens: 0,
              cache_write_tokens: 0,
              web_search_requests: 0,
              cost_usd: 0.06,
            },
          },
          duration_api_ms: 9999,
        },
      ];
      store.applyEventBatch(events as BusEvent[]);

      // Token counts preserved from first update (zero-token guard)
      expect(store.usage.inputTokens).toBe(500);
      expect(store.usage.outputTokens).toBe(100);
      // Cost takes max
      expect(store.usage.cost).toBe(0.06);
      // modelUsage and durationApiMs preserved from zero-token update
      expect(store.usage.modelUsage).toBeDefined();
      expect(store.usage.modelUsage!["claude-sonnet-4-5-20250929"].cost_usd).toBe(0.06);
      expect(store.usage.durationApiMs).toBe(9999);
    });

    it("modelUsage is undefined when not present in events (backward compat)", () => {
      store.run = makeRun("run-1");
      store.phase = "running";
      store.applyEventBatch(simpleChatEvents as BusEvent[]);
      expect(store.usage.modelUsage).toBeUndefined();
      expect(store.usage.durationApiMs).toBeUndefined();
    });
  });

  // ── applyHookUsage preserves new fields ──

  describe("applyHookUsage preserves modelUsage/durationApiMs", () => {
    it("preserves modelUsage and durationApiMs after hook usage", () => {
      store.run = makeRun("run-1");
      store.phase = "running";
      // Set up usage with modelUsage
      store.usage = {
        inputTokens: 500,
        outputTokens: 100,
        cacheReadTokens: 0,
        cacheWriteTokens: 0,
        cost: 0.05,
        modelUsage: {
          "claude-sonnet-4-5-20250929": {
            input_tokens: 500,
            output_tokens: 100,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            web_search_requests: 0,
            cost_usd: 0.05,
          },
        },
        durationApiMs: 5000,
      };
      // Apply hook usage (cumulative)
      store.applyHookUsage({
        run_id: "run-1",
        input_tokens: 100,
        output_tokens: 20,
        cost: 0.01,
      });

      expect(store.usage.inputTokens).toBe(600);
      expect(store.usage.outputTokens).toBe(120);
      expect(store.usage.cost).toBeCloseTo(0.06);
      // New fields should be preserved
      expect(store.usage.modelUsage).toBeDefined();
      expect(store.usage.modelUsage!["claude-sonnet-4-5-20250929"]).toBeDefined();
      expect(store.usage.durationApiMs).toBe(5000);
    });
  });

  // ── Session commands from session_init (#9) ──

  describe("session_init slash_commands", () => {
    it("stores sessionCommands from session_init", () => {
      store.run = makeRun("run-1");
      store.phase = "running";
      const events: BusEvent[] = [
        {
          type: "session_init",
          run_id: "run-1",
          session_id: "sess-1",
          model: "claude-opus-4-6",
          tools: ["Bash"],
          cwd: "/",
          slash_commands: [
            { name: "compact", description: "Compact context", aliases: [] },
            { name: "model", description: "Switch model", aliases: ["m"] },
          ],
        },
      ];
      store.applyEventBatch(events as BusEvent[]);

      expect(store.sessionCommands).toHaveLength(2);
      expect(store.sessionCommands[0].name).toBe("compact");
      expect(store.sessionCommands[1].name).toBe("model");
      expect(store.sessionCommands[1].aliases).toEqual(["m"]);
    });

    it("leaves sessionCommands empty when slash_commands is missing", () => {
      store.run = makeRun("run-1");
      store.phase = "running";
      const events: BusEvent[] = [
        {
          type: "session_init",
          run_id: "run-1",
          session_id: "sess-1",
          model: "claude-opus-4-6",
          tools: ["Bash"],
          cwd: "/",
        },
      ];
      store.applyEventBatch(events as BusEvent[]);

      expect(store.sessionCommands).toHaveLength(0);
    });

    it("leaves sessionCommands empty when slash_commands is empty array", () => {
      store.run = makeRun("run-1");
      store.phase = "running";
      const events: BusEvent[] = [
        {
          type: "session_init",
          run_id: "run-1",
          session_id: "sess-1",
          model: "claude-opus-4-6",
          tools: ["Bash"],
          cwd: "/",
          slash_commands: [],
        },
      ];
      store.applyEventBatch(events as BusEvent[]);

      expect(store.sessionCommands).toHaveLength(0);
    });

    it("clears sessionCommands on reset()", () => {
      store.run = makeRun("run-1");
      store.phase = "running";
      store.sessionCommands = [{ name: "test", description: "test", aliases: [] }];
      store.reset();
      expect(store.sessionCommands).toHaveLength(0);
    });
  });

  // ── isKnownSlashCommand ──

  describe("isKnownSlashCommand", () => {
    afterEach(() => {
      cliInfoMocks.getCliCommands.mockClear();
      cliInfoMocks.getCliCommands.mockReturnValue([]);
    });

    it("returns true for known session command", () => {
      store.sessionCommands = [{ name: "insights", description: "Show insights", aliases: ["i"] }];
      expect(store.isKnownSlashCommand("/insights")).toBe(true);
      expect(store.isKnownSlashCommand("/insights some args")).toBe(true);
    });

    it("returns true for command alias", () => {
      store.sessionCommands = [{ name: "insights", description: "Show insights", aliases: ["i"] }];
      expect(store.isKnownSlashCommand("/i")).toBe(true);
    });

    it("returns true for available skill", () => {
      store.availableSkills = ["find-bugs"];
      expect(store.isKnownSlashCommand("/find-bugs")).toBe(true);
    });

    it("falls back to getCliCommands when sessionCommands empty", () => {
      cliInfoMocks.getCliCommands.mockReturnValue([
        { name: "compact", description: "Compact", aliases: [] },
      ]);
      store.sessionCommands = [];
      expect(store.isKnownSlashCommand("/compact")).toBe(true);
      expect(cliInfoMocks.getCliCommands).toHaveBeenCalled();
    });

    it("returns true on cold start (all sources empty) for valid pattern", () => {
      // Cold start: no commands loaded yet — trusts regex boundary
      store.sessionCommands = [];
      store.availableSkills = [];
      cliInfoMocks.getCliCommands.mockReturnValue([]);
      expect(store.isKnownSlashCommand("/anything")).toBe(true);
    });

    it("returns false for unknown command when list available", () => {
      store.sessionCommands = [{ name: "insights", description: "Show insights", aliases: [] }];
      expect(store.isKnownSlashCommand("/nonexistent")).toBe(false);
    });

    it("returns false for path-like text", () => {
      expect(store.isKnownSlashCommand("/home/user/path")).toBe(false);
    });

    it("returns false for slash-path without boundary", () => {
      store.sessionCommands = [{ name: "status", description: "Status", aliases: [] }];
      expect(store.isKnownSlashCommand("/status/log")).toBe(false);
    });

    it("returns false for non-slash text", () => {
      expect(store.isKnownSlashCommand("hello world")).toBe(false);
    });

    it("is case-insensitive", () => {
      store.sessionCommands = [{ name: "Insights", description: "Show insights", aliases: [] }];
      expect(store.isKnownSlashCommand("/insights")).toBe(true);
      expect(store.isKnownSlashCommand("/INSIGHTS")).toBe(true);
    });
  });

  // ── MCP servers from session_init (#2) ──

  describe("session_init mcp_servers", () => {
    it("stores mcpServers from session_init", () => {
      store.run = makeRun("run-1");
      store.phase = "running";
      const events: BusEvent[] = [
        {
          type: "session_init",
          run_id: "run-1",
          session_id: "sess-1",
          model: "claude-opus-4-6",
          tools: ["Bash"],
          cwd: "/",
          mcp_servers: [
            { name: "postgres", status: "connected" },
            { name: "github", status: "failed", error: "auth error" },
          ],
        },
      ];
      store.applyEventBatch(events as BusEvent[]);

      expect(store.mcpServers).toHaveLength(2);
      expect(store.mcpServers[0].name).toBe("postgres");
      expect(store.mcpServers[0].status).toBe("connected");
      expect(store.mcpServers[1].name).toBe("github");
      expect(store.mcpServers[1].status).toBe("failed");
      expect(store.mcpServers[1].error).toBe("auth error");
    });

    it("leaves mcpServers empty when mcp_servers is missing", () => {
      store.run = makeRun("run-1");
      store.phase = "running";
      const events: BusEvent[] = [
        {
          type: "session_init",
          run_id: "run-1",
          session_id: "sess-1",
          model: "claude-opus-4-6",
          tools: ["Bash"],
          cwd: "/",
        },
      ];
      store.applyEventBatch(events as BusEvent[]);

      expect(store.mcpServers).toHaveLength(0);
    });

    it("clears mcpServers on reset()", () => {
      store.run = makeRun("run-1");
      store.phase = "running";
      store.mcpServers = [{ name: "test", status: "connected" }];
      store.reset();
      expect(store.mcpServers).toHaveLength(0);
    });

    it("updateMcpServers replaces server list", () => {
      store.run = makeRun("run-1");
      store.phase = "running";
      store.mcpServers = [{ name: "old", status: "connected" }];
      store.updateMcpServers([
        { name: "new1", status: "connected" },
        { name: "new2", status: "pending" },
      ]);
      expect(store.mcpServers).toHaveLength(2);
      expect(store.mcpServers[0].name).toBe("new1");
      expect(store.mcpServers[1].name).toBe("new2");
    });
  });

  // ── canResumeRun ──

  describe("canResumeRun", () => {
    it("returns true for terminal run with session_id", () => {
      expect(canResumeRun({ session_id: "sess-1", status: "completed" }, "completed", false)).toBe(
        true,
      );
      expect(canResumeRun({ session_id: "sess-1", status: "failed" }, "failed", false)).toBe(true);
      expect(canResumeRun({ session_id: "sess-1", status: "stopped" }, "stopped", false)).toBe(
        true,
      );
    });

    it("returns false without session_id", () => {
      expect(canResumeRun({ status: "completed" }, "completed", false)).toBe(false);
    });

    it("returns false for active phases", () => {
      expect(canResumeRun({ session_id: "sess-1", status: "running" }, "running", false)).toBe(
        false,
      );
      expect(canResumeRun({ session_id: "sess-1", status: "running" }, "spawning", false)).toBe(
        false,
      );
    });

    it("returns false when noSessionPersistence is true", () => {
      expect(canResumeRun({ session_id: "sess-1", status: "completed" }, "completed", true)).toBe(
        false,
      );
    });

    it("returns false for null run", () => {
      expect(canResumeRun(null, "completed", false)).toBe(false);
    });

    it("returns false for non-terminal phases (idle, ready, empty)", () => {
      expect(canResumeRun({ session_id: "sess-1", status: "running" }, "idle", false)).toBe(false);
      expect(canResumeRun({ session_id: "sess-1" }, "ready", false)).toBe(false);
      expect(canResumeRun({ session_id: "sess-1" }, "empty", false)).toBe(false);
    });
  });

  // ── Team session tool events ──

  describe("team session events", () => {
    beforeEach(() => {
      store.run = makeRun("run-1");
      store.phase = "running";
      store.applyEventBatch(teamSessionEvents as BusEvent[]);
    });

    it("creates timeline entries for all team tools", () => {
      const toolEntries = store.timeline.filter((e) => e.kind === "tool");
      expect(toolEntries).toHaveLength(6); // TeamCreate, 2x TaskCreate, TaskUpdate, TaskList, SendMessage
    });

    it("TeamCreate tool has correct input and status", () => {
      const tc = store.timeline.find(
        (e) => e.kind === "tool" && e.tool.tool_name === "TeamCreate",
      ) as Extract<TimelineEntry, { kind: "tool" }>;
      expect(tc).toBeDefined();
      expect(tc.tool.input.team_name).toBe("sdk-p0p1");
      expect(tc.tool.input.description).toBe("SDK priority features");
      expect(tc.tool.status).toBe("success");
    });

    it("TaskCreate tools have correct input", () => {
      const tasks = store.timeline.filter(
        (e) => e.kind === "tool" && e.tool.tool_name === "TaskCreate",
      ) as Extract<TimelineEntry, { kind: "tool" }>[];
      expect(tasks).toHaveLength(2);
      expect(tasks[0].tool.input.subject).toBe("Implement auth module");
      expect(tasks[1].tool.input.subject).toBe("Write unit tests");
    });

    it("TaskUpdate tool has correct input with taskId", () => {
      const tu = store.timeline.find(
        (e) => e.kind === "tool" && e.tool.tool_name === "TaskUpdate",
      ) as Extract<TimelineEntry, { kind: "tool" }>;
      expect(tu).toBeDefined();
      expect(tu.tool.input.taskId).toBe("1");
      expect(tu.tool.input.status).toBe("in_progress");
      expect(tu.tool.input.owner).toBe("researcher");
    });

    it("TaskList tool is present with success status", () => {
      const tl = store.timeline.find(
        (e) => e.kind === "tool" && e.tool.tool_name === "TaskList",
      ) as Extract<TimelineEntry, { kind: "tool" }>;
      expect(tl).toBeDefined();
      expect(tl.tool.status).toBe("success");
      expect(tl.tool.output).toBeDefined();
    });

    it("SendMessage tool has correct input fields", () => {
      const sm = store.timeline.find(
        (e) => e.kind === "tool" && e.tool.tool_name === "SendMessage",
      ) as Extract<TimelineEntry, { kind: "tool" }>;
      expect(sm).toBeDefined();
      expect(sm.tool.input.type).toBe("message");
      expect(sm.tool.input.recipient).toBe("researcher");
      expect(sm.tool.input.content).toBe("Please start on auth");
    });

    it("task_notification is captured in taskNotifications Map", () => {
      expect(store.taskNotifications.size).toBe(1);
      const item = store.taskNotifications.get("1");
      expect(item).toBeDefined();
      expect(item!.task_id).toBe("1");
      expect(item!.status).toBe("in_progress");
    });

    it("builds correct full timeline (user + 6 tools + assistant)", () => {
      // user_message + 6 tool entries + message_complete = 8
      expect(store.timeline).toHaveLength(8);
      expect(store.timeline[0].kind).toBe("user");
      expect(store.timeline[7].kind).toBe("assistant");
    });

    it("ends at idle phase", () => {
      expect(store.phase).toBe("idle");
    });
  });

  // ── Verbose CLI fields ──

  describe("verbose CLI fields", () => {
    it("stores session_init verbose fields", () => {
      store.run = makeRun("run-1");
      store.phase = "running";
      const events: BusEvent[] = [
        {
          type: "session_init",
          run_id: "run-1",
          session_id: "sess-1",
          model: "claude-opus-4-6",
          tools: ["Bash"],
          cwd: "/",
          permissionMode: "default",
          apiKeySource: "anthropic",
          claude_code_version: "2.1.41",
          output_style: "default",
          agents: ["Bash", "general-purpose", "Explore", "Plan"],
          skills: ["find-bugs", "review"],
          plugins: [],
          fast_mode_state: "off",
        },
      ];
      store.applyEventBatch(events as BusEvent[]);

      expect(store.cliVersion).toBe("2.1.41");
      expect(store.permissionMode).toBe("default");
      expect(store.fastModeState).toBe("off");
      expect(store.apiKeySource).toBe("anthropic");
      expect(store.availableAgents).toEqual(["Bash", "general-purpose", "Explore", "Plan"]);
      expect(store.availableSkills).toEqual(["find-bugs", "review"]);
      expect(store.availablePlugins).toEqual([]);
    });

    it("stores usage_update verbose fields", () => {
      store.run = makeRun("run-1");
      store.phase = "running";
      const events: BusEvent[] = [
        {
          type: "session_init",
          run_id: "run-1",
          session_id: "sess-1",
          model: "claude-opus-4-6",
          tools: ["Bash"],
          cwd: "/",
        },
        {
          type: "usage_update",
          run_id: "run-1",
          input_tokens: 500,
          output_tokens: 100,
          total_cost_usd: 0.05,
          duration_ms: 4277,
          num_turns: 3,
        },
      ];
      store.applyEventBatch(events as BusEvent[]);

      expect(store.durationMs).toBe(4277);
      expect(store.numTurns).toBe(3);
    });

    it("handles events without verbose fields (backward compat)", () => {
      store.run = makeRun("run-1");
      store.phase = "running";
      const events: BusEvent[] = [
        {
          type: "session_init",
          run_id: "run-1",
          session_id: "sess-1",
          model: "claude-opus-4-6",
          tools: ["Bash"],
          cwd: "/",
        },
        {
          type: "usage_update",
          run_id: "run-1",
          input_tokens: 100,
          output_tokens: 20,
          total_cost_usd: 0.01,
        },
      ];
      store.applyEventBatch(events as BusEvent[]);

      // All verbose fields should remain at their defaults
      expect(store.cliVersion).toBe("");
      expect(store.permissionMode).toBe("");
      expect(store.fastModeState).toBe("");
      expect(store.apiKeySource).toBe("");
      expect(store.availableAgents).toEqual([]);
      expect(store.availableSkills).toEqual([]);
      expect(store.availablePlugins).toEqual([]);
      expect(store.numTurns).toBe(0);
      expect(store.durationMs).toBe(0);
    });

    it("protocol-events fixture includes verbose fields", () => {
      store.run = makeRun("run-1");
      store.phase = "running";
      store.applyEventBatch(protocolEvents as BusEvent[]);

      // session_init verbose fields
      expect(store.cliVersion).toBe("2.1.41");
      expect(store.permissionMode).toBe("default");
      expect(store.fastModeState).toBe("off");
      expect(store.apiKeySource).toBe("anthropic");
      expect(store.availableAgents).toEqual(["Bash", "general-purpose", "Explore", "Plan"]);
      expect(store.availableSkills).toEqual(["find-bugs", "review"]);
      expect(store.availablePlugins).toEqual([]);

      // usage_update verbose fields
      expect(store.durationMs).toBe(4277);
      expect(store.numTurns).toBe(1);

      // hook verbose fields
      const hookStarted = store.hookEvents.find((h) => h.type === "hook_started");
      expect(hookStarted).toBeDefined();
      expect(hookStarted!.hook_name).toBe("PreToolUse:check");

      const hookResponse = store.hookEvents.find((h) => h.type === "hook_response");
      expect(hookResponse).toBeDefined();
      expect(hookResponse!.hook_name).toBe("PreToolUse:check");
      expect(hookResponse!.stdout).toBe("");
      expect(hookResponse!.stderr).toBe("");
      expect(hookResponse!.exit_code).toBe(0);
    });

    it("clears verbose fields on reset()", () => {
      store.run = makeRun("run-1");
      store.phase = "running";
      store.cliVersion = "2.1.41";
      store.permissionMode = "default";
      store.fastModeState = "off";
      store.apiKeySource = "anthropic";
      store.availableAgents = ["Bash"];
      store.availableSkills = ["review"];
      store.availablePlugins = [{ name: "test" }];
      store.numTurns = 5;
      store.durationMs = 10000;

      store.reset();

      expect(store.cliVersion).toBe("");
      expect(store.permissionMode).toBe("");
      expect(store.fastModeState).toBe("");
      expect(store.apiKeySource).toBe("");
      expect(store.availableAgents).toEqual([]);
      expect(store.availableSkills).toEqual([]);
      expect(store.availablePlugins).toEqual([]);
      expect(store.numTurns).toBe(0);
      expect(store.durationMs).toBe(0);
    });
  });

  // ── Session state sync (cwd, tools, outputStyle, plan mode) ──

  describe("session state sync", () => {
    it("session_init populates sessionCwd, sessionTools, outputStyle", () => {
      store.run = makeRun("run-1");
      store.phase = "running";
      const events: BusEvent[] = [
        {
          type: "session_init",
          run_id: "run-1",
          session_id: "sess-1",
          model: "claude-opus-4-6",
          tools: ["Bash", "Read", "Write"],
          cwd: "/home/user/project",
          output_style: "concise",
        },
      ];
      store.applyEventBatch(events as BusEvent[]);

      expect(store.sessionCwd).toBe("/home/user/project");
      expect(store.sessionTools).toEqual(["Bash", "Read", "Write"]);
      expect(store.outputStyle).toBe("concise");
      expect(store.sessionInitReceived).toBe(true);
    });

    it("session_init with empty values clears stale state", () => {
      store.run = makeRun("run-1");
      store.phase = "running";
      // Pre-populate with old values
      store.sessionCwd = "/old/path";
      store.sessionTools = ["OldTool"];
      store.outputStyle = "verbose";

      const events: BusEvent[] = [
        {
          type: "session_init",
          run_id: "run-1",
          session_id: "sess-1",
          model: "claude-opus-4-6",
          // cwd, tools, output_style not present → should clear via ?? ""
        },
      ];
      store.applyEventBatch(events as BusEvent[]);

      expect(store.sessionCwd).toBe("");
      expect(store.sessionTools).toEqual([]);
      expect(store.outputStyle).toBe("");
    });

    it("effectiveCwd prefers sessionCwd, falls back to run.cwd", () => {
      store.run = makeRun("run-1", { cwd: "/run/cwd" });
      store.phase = "running";
      expect(store.effectiveCwd).toBe("/run/cwd");

      store.sessionCwd = "/session/cwd";
      expect(store.effectiveCwd).toBe("/session/cwd");

      store.sessionCwd = "";
      expect(store.effectiveCwd).toBe("/run/cwd");
    });

    it("EnterPlanMode tool_end(success) sets permissionMode to plan", () => {
      store.run = makeRun("run-1");
      store.phase = "running";
      store.permissionMode = "default";

      const events: BusEvent[] = [
        {
          type: "tool_start",
          run_id: "run-1",
          tool_use_id: "epm-1",
          tool_name: "EnterPlanMode",
          input: {},
        },
        {
          type: "tool_end",
          run_id: "run-1",
          tool_use_id: "epm-1",
          tool_name: "EnterPlanMode",
          output: {},
          status: "success",
        },
      ];
      store.applyEventBatch(events as BusEvent[]);

      expect(store.permissionMode).toBe("plan");
      expect(store.previousPermissionMode).toBe("default");
    });

    it("ExitPlanMode tool_end(success) restores previousPermissionMode", () => {
      store.run = makeRun("run-1");
      store.phase = "running";
      store.permissionMode = "plan";
      store.previousPermissionMode = "acceptEdits";

      const events: BusEvent[] = [
        {
          type: "tool_start",
          run_id: "run-1",
          tool_use_id: "xpm-1",
          tool_name: "ExitPlanMode",
          input: {},
        },
        {
          type: "tool_end",
          run_id: "run-1",
          tool_use_id: "xpm-1",
          tool_name: "ExitPlanMode",
          output: {},
          status: "success",
        },
      ];
      store.applyEventBatch(events as BusEvent[]);

      expect(store.permissionMode).toBe("acceptEdits");
      expect(store.previousPermissionMode).toBe("");
    });

    it("EnterPlanMode tool_end(error) does not change permissionMode", () => {
      store.run = makeRun("run-1");
      store.phase = "running";
      store.permissionMode = "default";

      const events: BusEvent[] = [
        {
          type: "tool_start",
          run_id: "run-1",
          tool_use_id: "epm-err",
          tool_name: "EnterPlanMode",
          input: {},
        },
        {
          type: "tool_end",
          run_id: "run-1",
          tool_use_id: "epm-err",
          tool_name: "EnterPlanMode",
          output: { error: "denied" },
          status: "error",
        },
      ];
      store.applyEventBatch(events as BusEvent[]);

      expect(store.permissionMode).toBe("default");
      expect(store.previousPermissionMode).toBe("");
    });

    it("ExitPlanMode with empty previousPermissionMode is no-op", () => {
      store.run = makeRun("run-1");
      store.phase = "running";
      store.permissionMode = "bypassPermissions";
      store.previousPermissionMode = "";

      const events: BusEvent[] = [
        {
          type: "tool_start",
          run_id: "run-1",
          tool_use_id: "xpm-noop",
          tool_name: "ExitPlanMode",
          input: {},
        },
        {
          type: "tool_end",
          run_id: "run-1",
          tool_use_id: "xpm-noop",
          tool_name: "ExitPlanMode",
          output: {},
          status: "success",
        },
      ];
      store.applyEventBatch(events as BusEvent[]);

      // Should NOT change permissionMode — previousPermissionMode was empty
      expect(store.permissionMode).toBe("bypassPermissions");
      expect(store.previousPermissionMode).toBe("");
    });

    it("subagent EnterPlanMode (with parent_tool_use_id) does not affect main permissionMode", () => {
      store.run = makeRun("run-1");
      store.phase = "running";
      store.permissionMode = "default";

      const events: BusEvent[] = [
        // Parent tool
        {
          type: "tool_start",
          run_id: "run-1",
          tool_use_id: "parent-task",
          tool_name: "Task",
          input: {},
        },
        // Subagent EnterPlanMode (has parent_tool_use_id)
        {
          type: "tool_start",
          run_id: "run-1",
          tool_use_id: "sub-epm",
          tool_name: "EnterPlanMode",
          input: {},
          parent_tool_use_id: "parent-task",
        },
        {
          type: "tool_end",
          run_id: "run-1",
          tool_use_id: "sub-epm",
          tool_name: "EnterPlanMode",
          output: {},
          status: "success",
          parent_tool_use_id: "parent-task",
        },
      ];
      store.applyEventBatch(events as BusEvent[]);

      // Main permissionMode should NOT change
      expect(store.permissionMode).toBe("default");
      expect(store.previousPermissionMode).toBe("");
    });

    it("reset() clears new session state fields", () => {
      store.run = makeRun("run-1");
      store.phase = "running";
      store.sessionCwd = "/some/path";
      store.sessionTools = ["Bash"];
      store.outputStyle = "concise";
      store.previousPermissionMode = "default";
      store.sessionInitReceived = true;

      store.reset();

      expect(store.sessionCwd).toBe("");
      expect(store.sessionTools).toEqual([]);
      expect(store.outputStyle).toBe("");
      expect(store.previousPermissionMode).toBe("");
      expect(store.sessionInitReceived).toBe(false);
    });
  });

  // ── Strict fixture replay (Phase 3 contract tests) ──

  describe("strict fixture replay (contract tests)", () => {
    // Strict fixtures: all events are known types, 0 unknown + 0 raw fallback expected.
    const strictFixtures: Array<{ name: string; runId: string; events: unknown[] }> = [
      { name: "simple-chat", runId: "run-1", events: simpleChatEvents },
      { name: "chat-with-tools", runId: "run-2", events: chatWithToolsEvents },
      { name: "multi-turn", runId: "run-3", events: multiTurnEvents },
      { name: "compact-boundary", runId: "run-cb-1", events: compactBoundaryEvents },
      { name: "ask-user-question", runId: "run-5", events: askUserQuestionEvents },
      { name: "subagent-task", runId: "run-sub", events: subagentTaskEvents },
      { name: "team-session", runId: "run-1", events: teamSessionEvents },
    ];

    for (const { name, runId, events } of strictFixtures) {
      describe(`strict: ${name}`, () => {
        it("replays with strictMode without throwing", () => {
          const strictStore = new SessionStore();
          strictStore.strictMode = true;
          strictStore.run = makeRun(runId);
          strictStore.phase = "running";
          // Should not throw — all events in strict fixtures are known types
          expect(() => strictStore.applyEventBatch(events as BusEvent[])).not.toThrow();
        });

        it("post-condition: 0 unknownEventCount + 0 rawFallbackCount", () => {
          const strictStore = new SessionStore();
          strictStore.strictMode = true;
          strictStore.run = makeRun(runId);
          strictStore.phase = "running";
          strictStore.applyEventBatch(events as BusEvent[]);
          expect(strictStore.unknownEventCount).toBe(0);
          expect(strictStore.rawFallbackCount).toBe(0);
        });
      });
    }

    // Non-strict fixtures: degradation scenarios that may contain unknown/raw events.
    describe("non-strict: malformed-events", () => {
      it("replays without strictMode, no crash", () => {
        const s = new SessionStore();
        s.run = makeRun("run-m1");
        s.phase = "running";
        expect(() => s.applyEventBatch(malformedEvents as BusEvent[])).not.toThrow();
      });

      it("counts unknown events and raw fallbacks", () => {
        const s = new SessionStore();
        s.run = makeRun("run-m1");
        s.phase = "running";
        s.applyEventBatch(malformedEvents as BusEvent[]);
        // malformed-events.json contains: 1 unknown type (brand_new_event_type) + 1 raw fallback (claude_future_feature)
        expect(s.unknownEventCount).toBe(1);
        expect(s.rawFallbackCount).toBe(1);
      });

      it("strict mode would throw on unknown event", () => {
        const s = new SessionStore();
        s.strictMode = true;
        s.run = makeRun("run-m1");
        s.phase = "running";
        expect(() => s.applyEventBatch(malformedEvents as BusEvent[])).toThrow("[STRICT]");
      });

      it("still builds valid timeline for known events", () => {
        const s = new SessionStore();
        s.run = makeRun("run-m1");
        s.phase = "running";
        s.applyEventBatch(malformedEvents as BusEvent[]);
        // user_message + message_complete = 2 timeline entries (tool_start with empty id still adds an entry)
        expect(s.timeline.filter((e) => e.kind === "user")).toHaveLength(1);
        expect(s.timeline.filter((e) => e.kind === "assistant")).toHaveLength(1);
        expect(s.phase).toBe("idle");
      });
    });
  });

  // ── Snapshot cache (IDB) ──

  describe("snapshot cache", () => {
    const mockReadSnapshot = snapshotCache.readSnapshot as ReturnType<typeof vi.fn>;
    const mockWriteSnapshot = snapshotCache.writeSnapshot as ReturnType<typeof vi.fn>;
    const mockDeleteSnapshot = snapshotCache.deleteSnapshot as ReturnType<typeof vi.fn>;
    const mockGetRun = api.getRun as ReturnType<typeof vi.fn>;
    const mockGetBusEvents = api.getBusEvents as ReturnType<typeof vi.fn>;

    beforeEach(() => {
      mockReadSnapshot.mockReset().mockResolvedValue(null);
      mockWriteSnapshot.mockReset().mockResolvedValue(undefined);
      mockDeleteSnapshot.mockReset().mockResolvedValue(undefined);
      mockGetRun.mockReset();
      mockGetBusEvents.mockReset().mockResolvedValue([]);
    });

    describe("snapshot hit vs miss deep comparison", () => {
      it("produces identical state whether from snapshot or reducer replay", async () => {
        // Step 1: Build store via reducer replay (the "miss" path)
        const missStore = new SessionStore();
        missStore.run = makeRun("run-deep", { status: "completed", agent: "claude" });
        missStore.phase = "completed";
        missStore.applyEventBatch(multiTurnEvents as BusEvent[], { replayOnly: true });

        // Capture the snapshot body that would have been written
        const snapshotBody = (
          missStore as unknown as { _buildSnapshot(): string }
        )._buildSnapshot();

        // Step 2: Build store from snapshot (the "hit" path)
        const hitStore = new SessionStore();
        hitStore.run = makeRun("run-deep", { status: "completed", agent: "claude" });
        hitStore.phase = "completed";
        // _clearContentState is called by loadRun before _tryApplySnapshot
        (hitStore as unknown as { _clearContentState(): void })._clearContentState();
        const ok = (
          hitStore as unknown as { _tryApplySnapshot(b: string): boolean }
        )._tryApplySnapshot(snapshotBody);
        expect(ok).toBe(true);

        // Deep-compare key fields
        expect(hitStore.timeline).toEqual(missStore.timeline);
        expect(hitStore.usage).toEqual(missStore.usage);
        expect(hitStore.model).toBe(missStore.model);
        expect(hitStore.turnUsages).toEqual(missStore.turnUsages);
        expect(hitStore.streamingText).toBe(missStore.streamingText);
        expect(hitStore.thinkingText).toBe(missStore.thinkingText);
        expect(hitStore.tools).toEqual(missStore.tools);
        expect(hitStore.hookEvents).toEqual(missStore.hookEvents);
        expect(hitStore.numTurns).toBe(missStore.numTurns);
        expect(hitStore.durationMs).toBe(missStore.durationMs);
        expect(hitStore.compactCount).toBe(missStore.compactCount);
        expect(hitStore.sessionInitReceived).toBe(missStore.sessionInitReceived);
        expect(hitStore.cliVersion).toBe(missStore.cliVersion);
        expect(hitStore.permissionMode).toBe(missStore.permissionMode);
      });

      it("includes sentinel values through snapshot round-trip", () => {
        const store1 = new SessionStore();
        store1.run = makeRun("run-s1", { status: "completed", agent: "claude" });
        store1.phase = "completed";
        store1.applyEventBatch(simpleChatEvents as BusEvent[], { replayOnly: true });
        // Set sentinel values to verify they survive round-trip
        store1.cliVersion = "1.2.3-sentinel";
        store1.sessionCwd = "/sentinel/path";

        const body = (store1 as unknown as { _buildSnapshot(): string })._buildSnapshot();

        const store2 = new SessionStore();
        store2.run = makeRun("run-s1", { status: "completed", agent: "claude" });
        store2.phase = "completed";
        (store2 as unknown as { _clearContentState(): void })._clearContentState();
        const ok = (
          store2 as unknown as { _tryApplySnapshot(b: string): boolean }
        )._tryApplySnapshot(body);
        expect(ok).toBe(true);
        expect(store2.cliVersion).toBe("1.2.3-sentinel");
        expect(store2.sessionCwd).toBe("/sentinel/path");
      });
    });

    describe("_tryApplySnapshot shape validation", () => {
      it("rejects body without timeline array", () => {
        const store1 = new SessionStore();
        const ok = (
          store1 as unknown as { _tryApplySnapshot(b: string): boolean }
        )._tryApplySnapshot(JSON.stringify({ usage: { inputTokens: 0 } }));
        expect(ok).toBe(false);
        // Suppress console.warn from dbgWarn mock
        warnSpy.mockClear();
      });

      it("rejects body without usage object", () => {
        const store1 = new SessionStore();
        const ok = (
          store1 as unknown as { _tryApplySnapshot(b: string): boolean }
        )._tryApplySnapshot(JSON.stringify({ timeline: [], usage: null }));
        expect(ok).toBe(false);
        warnSpy.mockClear();
      });

      it("rejects invalid JSON", () => {
        const store1 = new SessionStore();
        const ok = (
          store1 as unknown as { _tryApplySnapshot(b: string): boolean }
        )._tryApplySnapshot("not json {{");
        expect(ok).toBe(false);
        warnSpy.mockClear();
      });
    });

    describe("loadRun snapshot paths", () => {
      it("uses snapshot on hit for terminal stream session", async () => {
        const termRun = makeRun("run-snap-1", { status: "completed", agent: "claude" });
        mockGetRun.mockResolvedValue(termRun);

        // Build snapshot body from a real replay
        const refStore = new SessionStore();
        refStore.run = termRun;
        refStore.phase = "completed";
        refStore.applyEventBatch(simpleChatEvents as BusEvent[], { replayOnly: true });
        const snapBody = (refStore as unknown as { _buildSnapshot(): string })._buildSnapshot();

        mockReadSnapshot.mockResolvedValue(snapBody);

        const testStore = new SessionStore();
        await testStore.loadRun("run-snap-1");

        // Should have used snapshot (readSnapshot called, getBusEvents NOT called)
        expect(mockReadSnapshot).toHaveBeenCalledWith("run-snap-1", "completed");
        expect(mockGetBusEvents).not.toHaveBeenCalled();
        // Timeline should match
        expect(testStore.timeline).toEqual(refStore.timeline);
        warnSpy.mockClear();
      });

      it("falls back to getBusEvents on snapshot miss", async () => {
        vi.useFakeTimers();
        const termRun = makeRun("run-snap-2", { status: "stopped", agent: "claude" });
        mockGetRun.mockResolvedValue(termRun);
        mockReadSnapshot.mockResolvedValue(null); // miss
        mockGetBusEvents.mockResolvedValue(simpleChatEvents);

        const testStore = new SessionStore();
        await testStore.loadRun("run-snap-2");

        expect(mockReadSnapshot).toHaveBeenCalledWith("run-snap-2", "stopped");
        expect(mockGetBusEvents).toHaveBeenCalledWith("run-snap-2");
        // Flush deferred _saveSnapshotToIdb (setTimeout(0))
        vi.advanceTimersByTime(1);
        // Should have written snapshot after reducer
        expect(mockWriteSnapshot).toHaveBeenCalled();
        expect(testStore.timeline.length).toBeGreaterThan(0);
        vi.useRealTimers();
        warnSpy.mockClear();
      });

      it("falls back to getBusEvents on corrupted snapshot", async () => {
        const termRun = makeRun("run-snap-3", { status: "completed", agent: "claude" });
        mockGetRun.mockResolvedValue(termRun);
        mockReadSnapshot.mockResolvedValue("{ invalid json }}}"); // corrupt
        mockGetBusEvents.mockResolvedValue(simpleChatEvents);

        const testStore = new SessionStore();
        await testStore.loadRun("run-snap-3");

        // Snapshot read was attempted but failed → fell back to getBusEvents
        expect(mockReadSnapshot).toHaveBeenCalled();
        expect(mockGetBusEvents).toHaveBeenCalledWith("run-snap-3");
        expect(testStore.timeline.length).toBeGreaterThan(0);
        warnSpy.mockClear();
      });
    });

    describe("loadRun write guard", () => {
      it("does NOT write snapshot when busEvents produce empty timeline (reducer anomaly)", async () => {
        const termRun = makeRun("run-wg-1", { status: "completed", agent: "claude" });
        mockGetRun.mockResolvedValue(termRun);
        mockReadSnapshot.mockResolvedValue(null);
        // Non-empty busEvents that produce an empty timeline is a reducer anomaly
        // For this test, we use a single unknown event type that doesn't create timeline entries
        mockGetBusEvents.mockResolvedValue([
          { type: "run_state", run_id: "run-wg-1", state: "running", error: null, exit_code: null },
        ]);

        const testStore = new SessionStore();
        await testStore.loadRun("run-wg-1");

        // Timeline is empty, busEvents was non-empty → should NOT write snapshot
        expect(testStore.timeline).toHaveLength(0);
        expect(mockWriteSnapshot).not.toHaveBeenCalled();
        warnSpy.mockClear();
      });

      it("writes snapshot for legit empty session (0 busEvents)", async () => {
        vi.useFakeTimers();
        const termRun = makeRun("run-wg-2", { status: "completed", agent: "claude" });
        mockGetRun.mockResolvedValue(termRun);
        mockReadSnapshot.mockResolvedValue(null);
        mockGetBusEvents.mockResolvedValue([]); // truly empty session

        const testStore = new SessionStore();
        await testStore.loadRun("run-wg-2");

        // Flush deferred _saveSnapshotToIdb (setTimeout(0))
        vi.advanceTimersByTime(1);
        // 0 busEvents + 0 timeline → legit empty session → write allowed
        expect(mockWriteSnapshot).toHaveBeenCalled();
        vi.useRealTimers();
        warnSpy.mockClear();
      });
    });

    describe("resumeSession snapshot", () => {
      it("uses snapshot on hit and deletes after", async () => {
        const run = makeRun("run-res-1", {
          status: "stopped",
          agent: "claude",
          session_id: "sess-1",
        });
        mockGetRun.mockResolvedValue(run);

        // Build snapshot from replay
        const refStore = new SessionStore();
        refStore.run = run;
        refStore.phase = "stopped";
        refStore.applyEventBatch(simpleChatEvents as BusEvent[], { replayOnly: true });
        const snapBody = (refStore as unknown as { _buildSnapshot(): string })._buildSnapshot();

        mockReadSnapshot.mockResolvedValue(snapBody);

        const testStore = new SessionStore();
        testStore.agent = "claude";
        // resumeSession needs a phase that allows transition to spawning
        testStore.run = run;
        testStore.phase = "stopped";

        // Mock startSession to avoid actual IPC
        const mockStartSession = api.startSession as ReturnType<typeof vi.fn>;
        mockStartSession.mockResolvedValue(undefined);

        await testStore.resumeSession("run-res-1", "resume");

        // Snapshot was read
        expect(mockReadSnapshot).toHaveBeenCalledWith("run-res-1", "stopped");
        // getBusEvents NOT called (snapshot hit)
        expect(mockGetBusEvents).not.toHaveBeenCalled();
        // Snapshot was deleted (session goes live)
        expect(mockDeleteSnapshot).toHaveBeenCalledWith("run-res-1");
        // Timeline populated from snapshot
        expect(testStore.timeline).toEqual(refStore.timeline);
        warnSpy.mockClear();
      });

      it("falls back to getBusEvents when snapshot corrupted and deletes", async () => {
        const run = makeRun("run-res-2", {
          status: "stopped",
          agent: "claude",
          session_id: "sess-2",
        });
        mockGetRun.mockResolvedValue(run);
        mockReadSnapshot.mockResolvedValue("bad json {{{"); // corrupted
        mockGetBusEvents.mockResolvedValue(simpleChatEvents);

        const testStore = new SessionStore();
        testStore.agent = "claude";
        testStore.run = run;
        testStore.phase = "stopped";

        const mockStartSession = api.startSession as ReturnType<typeof vi.fn>;
        mockStartSession.mockResolvedValue(undefined);

        await testStore.resumeSession("run-res-2", "resume");

        // Snapshot read attempted, then fell back to getBusEvents
        expect(mockReadSnapshot).toHaveBeenCalled();
        expect(mockGetBusEvents).toHaveBeenCalledWith("run-res-2");
        // Still deleted (going live)
        expect(mockDeleteSnapshot).toHaveBeenCalledWith("run-res-2");
        expect(testStore.timeline.length).toBeGreaterThan(0);
        warnSpy.mockClear();
      });

      it("always deletes snapshot even on miss", async () => {
        const run = makeRun("run-res-3", {
          status: "stopped",
          agent: "claude",
          session_id: "sess-3",
        });
        mockGetRun.mockResolvedValue(run);
        mockReadSnapshot.mockResolvedValue(null); // miss
        mockGetBusEvents.mockResolvedValue(simpleChatEvents);

        const testStore = new SessionStore();
        testStore.agent = "claude";
        testStore.run = run;
        testStore.phase = "stopped";

        const mockStartSession = api.startSession as ReturnType<typeof vi.fn>;
        mockStartSession.mockResolvedValue(undefined);

        await testStore.resumeSession("run-res-3", "resume");

        expect(mockDeleteSnapshot).toHaveBeenCalledWith("run-res-3");
        warnSpy.mockClear();
      });
    });

    describe("compact_boundary replayOnly", () => {
      it("does NOT set lastCompactedAt during replay", () => {
        const s = new SessionStore();
        s.run = makeRun("run-cb-1");
        s.phase = "running";
        s.applyEventBatch(compactBoundaryEvents as BusEvent[], { replayOnly: true });

        // replayOnly skips lastCompactedAt assignment
        expect(s.lastCompactedAt).toBe(0);
        // But compactCount is still incremented (it's a counter, not a timestamp)
        expect(s.compactCount).toBe(1);
        // Timeline separator should still be present
        expect(s.timeline.some((e) => e.kind === "separator")).toBe(true);
      });

      it("sets lastCompactedAt during live replay", () => {
        const s = new SessionStore();
        s.run = makeRun("run-cb-1");
        s.phase = "running";
        s.applyEventBatch(compactBoundaryEvents as BusEvent[]); // no replayOnly

        expect(s.lastCompactedAt).toBeGreaterThan(0);
        expect(s.compactCount).toBe(1);
      });
    });

    describe("applyEventBatch returns elapsed ms", () => {
      it("returns a number >= 0", () => {
        store.run = makeRun("run-1");
        store.phase = "running";
        const ms = store.applyEventBatch(simpleChatEvents as BusEvent[]);
        expect(typeof ms).toBe("number");
        expect(ms).toBeGreaterThanOrEqual(0);
      });
    });
  });
});
