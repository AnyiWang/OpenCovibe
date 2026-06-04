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
  sendSessionControl: vi.fn(),
  steerSession: vi.fn(),
  syncCliSession: vi.fn().mockResolvedValue({ newEvents: 0 }),
  renameRun: vi.fn().mockResolvedValue(undefined),
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
import ralphLoopEvents from "./__fixtures__/ralph-loop.json";
import scheduledTasksEvents from "./__fixtures__/scheduled-tasks.json";
import malformedEvents from "./__fixtures__/malformed-events.json";
import codexSimpleEvents from "./__fixtures__/codex-simple.json";
import subagentAgentEvents from "./__fixtures__/subagent-agent.json";
import codexCollabEvents from "./__fixtures__/codex-collab.json";

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
    execution_path: "session_actor" as const,
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
      const msgs = warnSpy.mock.calls.map((c: unknown[]) => c.join(" ")).join("\n");
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
      expect((store.timeline[0] as { content: string }).content).toBe("Hello");
      expect(store.timeline[1].kind).toBe("assistant");
      expect((store.timeline[1] as { content: string }).content).toBe("Hi there! How can I help?");
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

  // ── Live command output streaming (tool_output_delta) ──

  describe("tool_output_delta streaming", () => {
    beforeEach(() => {
      store.run = makeRun("run-1");
      store.phase = "running";
    });

    function bashTool() {
      return store.timeline.find((e) => e.kind === "tool" && e.id === "call_1") as Extract<
        (typeof store.timeline)[number],
        { kind: "tool" }
      >;
    }

    it("accumulates streamed chunks into the open Bash card, then tool_end overwrites", () => {
      store.applyEvent({
        type: "tool_start",
        run_id: "run-1",
        tool_use_id: "call_1",
        tool_name: "Bash",
        input: { command: "echo hi" },
      } as BusEvent);
      store.applyEvent({
        type: "tool_output_delta",
        run_id: "run-1",
        tool_use_id: "call_1",
        delta: "line 1\n",
      } as BusEvent);
      // Visible mid-run, before completion.
      expect(bashTool().tool.status).toBe("running");
      expect(bashTool().tool.output?.content).toBe("line 1\n");

      store.applyEvent({
        type: "tool_output_delta",
        run_id: "run-1",
        tool_use_id: "call_1",
        delta: "line 2\n",
      } as BusEvent);
      expect(bashTool().tool.output?.content).toBe("line 1\nline 2\n");

      // tool_end carries the authoritative aggregated output → overwrites (no dup).
      store.applyEvent({
        type: "tool_end",
        run_id: "run-1",
        tool_use_id: "call_1",
        tool_name: "Bash",
        output: { content: "line 1\nline 2\n" },
        status: "success",
        duration_ms: 5,
      } as BusEvent);
      expect(bashTool().tool.status).toBe("success");
      expect(bashTool().tool.output?.content).toBe("line 1\nline 2\n");
    });

    it("drops a delta with no matching open tool", () => {
      const before = store.timeline.length;
      store.applyEvent({
        type: "tool_output_delta",
        run_id: "run-1",
        tool_use_id: "missing",
        delta: "orphan",
      } as BusEvent);
      expect(store.timeline.length).toBe(before);
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

  // ── user_message UUID tracking ──

  describe("user_message UUID tracking", () => {
    it("stores cliUuid on timeline entry during replay", () => {
      store.run = makeRun("run-uuid");
      store.phase = "running";
      const ev: BusEvent = {
        type: "user_message",
        run_id: "run-uuid",
        text: "Hello",
        uuid: "cli-uuid-abc",
      };
      store.applyEventBatch([ev]);
      const userEntry = store.timeline.find((e) => e.kind === "user");
      expect(userEntry).toBeDefined();
      expect(userEntry!.kind).toBe("user");
      if (userEntry!.kind === "user") {
        expect(userEntry!.cliUuid).toBe("cli-uuid-abc");
      }
    });

    it("user_message without uuid has no cliUuid (backward compat)", () => {
      store.run = makeRun("run-uuid2");
      store.phase = "running";
      const ev: BusEvent = {
        type: "user_message",
        run_id: "run-uuid2",
        text: "Hello old",
      };
      store.applyEventBatch([ev]);
      const userEntry = store.timeline.find((e) => e.kind === "user");
      expect(userEntry).toBeDefined();
      if (userEntry!.kind === "user") {
        expect(userEntry!.cliUuid).toBeUndefined();
      }
    });

    it("merges cliUuid into existing optimistic entry (live dedup)", () => {
      store.run = makeRun("run-uuid3");
      store.phase = "running";
      // Simulate optimistic entry (no cliUuid)
      store.timeline = [
        {
          kind: "user",
          id: "opt-1",
          anchorId: "opt-1",
          content: "Hello",
          ts: new Date().toISOString(),
        },
      ];
      // Live event arrives with uuid
      const ev: BusEvent = {
        type: "user_message",
        run_id: "run-uuid3",
        text: "Hello",
        uuid: "cli-uuid-merge",
      };
      store.applyEvent(ev);
      expect(store.timeline).toHaveLength(1); // Still deduped
      const userEntry = store.timeline[0];
      if (userEntry.kind === "user") {
        expect(userEntry.cliUuid).toBe("cli-uuid-merge");
      }
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
      expect((store.timeline[0] as { content: string }).content).toBe("hello");
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
      expect((store.timeline[0] as { content: string }).content).toContain("claude_stdout_text");
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

    it("preserves permission_denied status in terminal runs (not finalized to error)", () => {
      // Denied AskUserQuestion: permission_denied is a terminal status, not stale.
      // It should NOT be overwritten to "error" by the finalizer.
      const events: BusEvent[] = [
        { type: "user_message", run_id: "run-pd", text: "Ask me" },
        {
          type: "run_state",
          run_id: "run-pd",
          state: "running",
        } as BusEvent,
        {
          type: "tool_start",
          run_id: "run-pd",
          tool_use_id: "ask-pd",
          tool_name: "AskUserQuestion",
          input: { question: "Pick one", options: ["A", "B"] },
        },
        {
          type: "permission_prompt",
          run_id: "run-pd",
          tool_use_id: "ask-pd",
          tool_name: "AskUserQuestion",
          request_id: "req-pd",
          tool_input: { question: "Pick one", options: ["A", "B"] },
          decision_reason: "",
        },
        {
          type: "tool_end",
          run_id: "run-pd",
          tool_use_id: "ask-pd",
          tool_name: "AskUserQuestion",
          output: { error: "User denied" },
          status: "error",
        },
        {
          type: "permission_denied",
          run_id: "run-pd",
          tool_use_id: "ask-pd",
          tool_name: "AskUserQuestion",
          tool_input: { question: "Pick one", options: ["A", "B"] },
        },
      ];

      store.run = makeRun("run-pd", { status: "stopped" });
      store.phase = "stopped";
      store.applyEventBatch(events as BusEvent[], { replayOnly: true });

      const toolEntry = store.timeline.find(
        (e) => e.kind === "tool" && e.id === "ask-pd",
      ) as Extract<TimelineEntry, { kind: "tool" }>;
      expect(toolEntry).toBeDefined();
      // permission_denied is terminal — should NOT be overwritten to "error"
      expect(toolEntry.tool.status).toBe("permission_denied");
    });

    it("permission_prompt with missing parent_tool_use_id updates subTimeline instead of creating duplicate", () => {
      const events: BusEvent[] = [
        { type: "user_message", run_id: "run-8", text: "Do it" },
        {
          type: "run_state",
          run_id: "run-8",
          state: "running",
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
          decision_reason: "",
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
          tool_name: "Bash",
          tool_input: { command: "rm -rf /" },
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
      const { dbgWarn: mockDbgWarn } = (await import("$lib/utils/debug")) as unknown as {
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
        } as BusEvent,
        { type: "message_complete", run_id: "run-1", message_id: "m1", text: "Response" },
        {
          type: "run_state",
          run_id: "run-1",
          state: "idle",
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
        } as BusEvent,
        {
          type: "run_state",
          run_id: "run-1",
          state: "running",
        } as BusEvent,
        {
          type: "run_state",
          run_id: "run-1",
          state: "idle",
        } as BusEvent,
        {
          type: "run_state",
          run_id: "run-1",
          state: "idle",
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
      expect((store.timeline[0] as { content: string }).content).toContain("claude_stderr");
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
    it("error flows through to error state but phase stays non-terminal (HC#1)", () => {
      store.run = makeRun("run-err-1");
      store.phase = "running";
      store.applyEventBatch(resultErrorMaxTurnsEvents as BusEvent[]);

      // HC#1 (#140): a result event = turn complete (→ idle), NOT session end.
      // Terminal failed/completed is decided in the backend on process EOF.
      expect(store.phase).toBe("idle");
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
        (e): e is Extract<TimelineEntry, { kind: "separator" }> =>
          e.kind === "separator" &&
          (e as { content: string }).content.includes("Context compacted"),
      );
      expect(compactEntries).toHaveLength(1);
      expect((compactEntries[0] as { content: string }).content).toContain("180k tokens");
    });

    it("preserves surrounding messages", () => {
      store.run = makeRun("run-cb-1");
      store.phase = "running";
      store.applyEventBatch(compactBoundaryEvents as BusEvent[]);

      // user + assistant("Hello!...") + separator(compact_boundary) + assistant("Continuing...")
      expect(store.timeline).toHaveLength(4);
      expect(store.timeline[0].kind).toBe("user");
      expect(store.timeline[1].kind).toBe("assistant");
      expect((store.timeline[1] as { content: string }).content).toContain("Hello!");
      expect(store.timeline[2].kind).toBe("separator");
      expect((store.timeline[2] as { content: string }).content).toContain("Context compacted");
      expect(store.timeline[3].kind).toBe("assistant");
      expect((store.timeline[3] as { content: string }).content).toContain("Continuing");
    });

    it("ends at idle phase", () => {
      store.run = makeRun("run-cb-1");
      store.phase = "running";
      store.applyEventBatch(compactBoundaryEvents as BusEvent[]);

      expect(store.phase).toBe("idle");
    });

    it("resets context usage tokens after full compaction", () => {
      store.run = makeRun("run-cb-1");
      store.phase = "running";
      // Apply events up to and including compact_boundary (index 5), stopping
      // before the post-compact usage_update so we can observe the reset.
      const upToCompact = (compactBoundaryEvents as BusEvent[]).slice(0, 6);
      store.applyEventBatch(upToCompact);

      expect(store.usage.inputTokens).toBe(0);
      expect(store.usage.cacheReadTokens).toBe(0);
      expect(store.usage.cacheWriteTokens).toBe(0);
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

    it("classifies frontend 60s timeout as server_issue, not auth_issue", () => {
      const c = classifyError(undefined, "No response after 60s — still waiting for API.");
      expect(c.category).toBe("server_issue");
      expect(c.canRetry).toBe(true);
      expect(c.settingsLink).toBe("");
    });

    it("still classifies real auth errors by subtype even if msg mentions API", () => {
      expect(classifyError("error_api_key_invalid", "Invalid API key provided").category).toBe(
        "auth_issue",
      );
      expect(classifyError(undefined, "Received 401 Unauthorized").category).toBe("auth_issue");
    });

    it("classifies session_timeout by text matching", () => {
      const c1 = classifyError(
        undefined,
        "Session timeout — waited 600s for can_use_tool response (Write). Process killed.",
      );
      expect(c1.category).toBe("session_timeout");
      expect(c1.canRetry).toBe(true);

      const c2 = classifyError(
        undefined,
        "Session timeout — no response from CLI for 10 minutes. Process killed.",
      );
      expect(c2.category).toBe("session_timeout");

      // Legacy message format
      expect(classifyError(undefined, "Session hard timeout — process killed").category).toBe(
        "session_timeout",
      );
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
      expect((subMsg as { content: string }).content).toBe("The command ran successfully.");
    });

    it("subagent message_delta does not affect main streamingText", () => {
      // Subagent message_delta routes to subTimeline, not main streamingText
      expect(store.streamingText).toBe("");
    });

    it("subagent thinking_delta does not affect main thinkingText", () => {
      // Subagent thinking_delta routes to subTimeline, not main thinkingText
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

  // ── Agent (Task→Agent rename) + Codex collab subagent shapes ──
  // The reducer routes subagent children by parent_tool_use_id (tool-name agnostic), so the
  // "Agent" rename and Codex collab runs must produce the same subTimeline + tool entries that
  // InlineToolCard / ToolDetailView consume. These lock the entry shapes (not the rendered DOM —
  // component-render tests aren't supported in this node-env vitest setup; assert on the data the
  // components branch on instead: isSubagentTool, tool_use_result.toolStats, input.codexCollab).
  describe("subagent rendering shapes", () => {
    it("Agent (renamed) routes children to subTimeline and surfaces AgentOutput toolStats", () => {
      store.run = makeRun("run-agent");
      store.phase = "running";
      store.applyEventBatch(subagentAgentEvents as BusEvent[]);

      const mainTools = store.timeline.filter((e) => e.kind === "tool");
      expect(mainTools).toHaveLength(1);
      const agentEntry = mainTools[0] as Extract<TimelineEntry, { kind: "tool" }>;
      expect(agentEntry.tool.tool_name).toBe("Agent");
      expect(agentEntry.tool.status).toBe("success");

      // Child Bash routed into the parent Agent subTimeline, same as the legacy Task path.
      expect(agentEntry.subTimeline).toBeDefined();
      expect(agentEntry.subTimeline!).toHaveLength(2); // Bash tool + subagent message
      const subBash = agentEntry.subTimeline![0] as Extract<TimelineEntry, { kind: "tool" }>;
      expect(subBash.tool.tool_name).toBe("Bash");

      // ToolDetailView's taskResult derives from tool_use_result.totalToolUseCount; toolStats
      // drives the per-category counts. Lock the shape the panel reads.
      const result = agentEntry.tool.tool_use_result as Record<string, unknown>;
      expect(result).toBeDefined();
      expect(result.totalToolUseCount).toBe(1);
      expect((result.toolStats as Record<string, unknown>).bashCount).toBe(1);
      expect(result.agentType).toBe("Bash");
    });

    it("Codex collab run carries codexCollab marker on input and tool_use_result", () => {
      store.run = makeRun("run-collab");
      store.phase = "running";
      store.applyEventBatch(codexCollabEvents as BusEvent[]);

      const tools = store.timeline.filter((e) => e.kind === "tool");
      expect(tools).toHaveLength(1);
      const collab = tools[0] as Extract<TimelineEntry, { kind: "tool" }>;
      expect(collab.tool.tool_name).toBe("Agent");
      expect(collab.tool.status).toBe("success");

      // ToolDetailView's codexCollab derived state prefers tool_use_result, falls back to input —
      // both carry the marker so the collab shape renders in-flight and after completion.
      const input = collab.tool.input as Record<string, unknown>;
      expect(input.codexCollab).toBe(true);
      expect(input.operation).toBe("spawnAgent");

      const result = collab.tool.tool_use_result as Record<string, unknown>;
      expect(result.codexCollab).toBe(true);
      expect(result.operation).toBe("spawnAgent");
      expect(result.status).toBe("completed");
      const agents = result.agents as Array<Record<string, unknown>>;
      expect(agents[0].thread_id).toBe("t2");
      expect(agents[0].status).toBe("completed");
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
      expect((parent.subTimeline![0] as { content: string }).content).toBe("Hello");
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
      expect((parent.subTimeline![0] as { content: string }).content).toBe("Hello world");
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
      expect((parent.subTimeline![0] as { content: string }).content).toBe("Final answer");
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
      expect((parentA.subTimeline![0] as { content: string }).content).toBe("from A");
      expect(parentA.subTimeline![0].id).toBe("__sub_stream_parent-a");
      expect(parentB.subTimeline).toHaveLength(1);
      expect((parentB.subTimeline![0] as { content: string }).content).toBe("from B");
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

  describe("ralph_loop events", () => {
    it("ralph_started initializes ralphLoop state", () => {
      store.run = makeRun("run-1");
      store.phase = "running";
      store.applyEvent({
        type: "ralph_started",
        run_id: "run-1",
        prompt: "Build an API",
        max_iterations: 10,
        completion_promise: "DONE",
        started_at: "2026-03-18T12:00:00Z",
      } as BusEvent);
      expect(store.ralphLoop).not.toBeNull();
      expect(store.ralphLoop!.active).toBe(true);
      expect(store.ralphLoop!.iteration).toBe(0);
      expect(store.ralphLoop!.maxIterations).toBe(10);
      expect(store.ralphLoop!.completionPromise).toBe("DONE");
    });

    it("ralph_iteration updates iteration count", () => {
      store.run = makeRun("run-1");
      store.phase = "running";
      store.applyEvent({
        type: "ralph_started",
        run_id: "run-1",
        prompt: "Build an API",
        max_iterations: 10,
        completion_promise: null,
        started_at: "2026-03-18T12:00:00Z",
      } as BusEvent);
      store.applyEvent({
        type: "ralph_iteration",
        run_id: "run-1",
        iteration: 3,
        max_iterations: 10,
      } as BusEvent);
      expect(store.ralphLoop!.iteration).toBe(3);
    });

    it("ralph_complete marks loop as inactive with reason", () => {
      store.run = makeRun("run-1");
      store.phase = "running";
      store.applyEvent({
        type: "ralph_started",
        run_id: "run-1",
        prompt: "Build an API",
        max_iterations: 5,
        completion_promise: null,
        started_at: "2026-03-18T12:00:00Z",
      } as BusEvent);
      store.applyEvent({
        type: "ralph_complete",
        run_id: "run-1",
        reason: "max_iterations",
        iteration: 5,
      } as BusEvent);
      expect(store.ralphLoop!.active).toBe(false);
      expect(store.ralphLoop!.reason).toBe("max_iterations");
      expect(store.ralphLoop!.iteration).toBe(5);
    });
  });

  describe("scheduled tasks (CronCreate/CronDelete reducer)", () => {
    function makeCronEvents(): BusEvent[] {
      return [
        {
          type: "tool_start",
          run_id: "run-cron",
          tool_use_id: "tool-cc-1",
          tool_name: "CronCreate",
          input: { cron: "*/5 * * * *", prompt: "echo test", recurring: true },
        } as BusEvent,
        {
          type: "tool_end",
          run_id: "run-cron",
          tool_use_id: "tool-cc-1",
          tool_name: "CronCreate",
          output: { success: true },
          status: "success",
          duration_ms: 12,
          tool_use_result: {
            id: "abc12345",
            humanSchedule: "every 5 minutes",
            recurring: true,
            durable: false,
          },
        } as BusEvent,
      ];
    }

    it("CronCreate success populates every ScheduledTask field via start↔end join", () => {
      store.run = makeRun("run-cron");
      store.phase = "running";
      store.applyEventBatch(makeCronEvents());
      expect(store.scheduledTasks).toHaveLength(1);
      const t = store.scheduledTasks[0];
      expect(t.id).toBe("abc12345");
      expect(t.humanSchedule).toBe("every 5 minutes");
      expect(t.recurring).toBe(true);
      expect(t.durable).toBe(false);
      expect(t.prompt).toBe("echo test");
      expect(t.cron).toBe("*/5 * * * *");
      expect(t.toolUseId).toBe("tool-cc-1");
    });

    it("re-emitted CronCreate with same id replaces existing task (latest input wins)", () => {
      store.run = makeRun("run-cron");
      store.phase = "running";
      // First create: prompt "echo test"
      store.applyEventBatch(makeCronEvents());
      expect(store.scheduledTasks).toHaveLength(1);
      expect(store.scheduledTasks[0].prompt).toBe("echo test");
      // Re-emit same id with different prompt (covers snapshot+live overlap)
      store.applyEventBatch([
        {
          type: "tool_start",
          run_id: "run-cron",
          tool_use_id: "tool-cc-2",
          tool_name: "CronCreate",
          input: { cron: "*/10 * * * *", prompt: "echo updated", recurring: true },
        } as BusEvent,
        {
          type: "tool_end",
          run_id: "run-cron",
          tool_use_id: "tool-cc-2",
          tool_name: "CronCreate",
          output: { success: true },
          status: "success",
          duration_ms: 12,
          tool_use_result: {
            id: "abc12345",
            humanSchedule: "every 10 minutes",
            recurring: true,
            durable: false,
          },
        } as BusEvent,
      ]);
      // Still one task — replaced, not appended
      expect(store.scheduledTasks).toHaveLength(1);
      expect(store.scheduledTasks[0].prompt).toBe("echo updated");
      expect(store.scheduledTasks[0].cron).toBe("*/10 * * * *");
      expect(store.scheduledTasks[0].humanSchedule).toBe("every 10 minutes");
    });

    it("live applyEvent path (ctx=null) joins start↔end via timeline lookup", () => {
      // Live WebSocket path delivers events one-by-one via applyEvent (ctx=null).
      // CronCreate at tool_end must find tool_start.input through the timeline.
      store.run = makeRun("run-cron");
      store.phase = "running";
      const [start, end] = makeCronEvents();
      store.applyEvent(start);
      store.applyEvent(end);
      expect(store.scheduledTasks).toHaveLength(1);
      const t = store.scheduledTasks[0];
      expect(t.prompt).toBe("echo test");
      expect(t.cron).toBe("*/5 * * * *");
      expect(t.id).toBe("abc12345");
    });

    it("lifecycle 0 → 1 → 0 across full fixture replay", () => {
      store.run = makeRun("run-cron");
      store.phase = "running";
      const events = scheduledTasksEvents as BusEvent[];
      // Replay through CronCreate tool_end (index 5)
      store.applyEventBatch(events.slice(0, 6));
      expect(store.scheduledTasks).toHaveLength(1);
      // Replay through end of fixture (includes CronDelete tool_end)
      store.applyEventBatch(events.slice(6));
      expect(store.scheduledTasks).toHaveLength(0);
    });

    it("CronDelete prefers tool_use_result.id over input fallback", () => {
      store.run = makeRun("run-cron");
      store.phase = "running";
      store.applyEventBatch(makeCronEvents());
      expect(store.scheduledTasks).toHaveLength(1);
      store.applyEventBatch([
        {
          type: "tool_start",
          run_id: "run-cron",
          tool_use_id: "tool-cd-1",
          tool_name: "CronDelete",
          // intentionally wrong/missing input id — must fall through to tool_use_result.id
          input: {},
        } as BusEvent,
        {
          type: "tool_end",
          run_id: "run-cron",
          tool_use_id: "tool-cd-1",
          tool_name: "CronDelete",
          output: { success: true },
          status: "success",
          duration_ms: 4,
          tool_use_result: { id: "abc12345" },
        } as BusEvent,
      ]);
      expect(store.scheduledTasks).toHaveLength(0);
    });

    it("CronDelete falls back to input.id when tool_use_result lacks id", () => {
      store.run = makeRun("run-cron");
      store.phase = "running";
      store.applyEventBatch(makeCronEvents());
      store.applyEventBatch([
        {
          type: "tool_start",
          run_id: "run-cron",
          tool_use_id: "tool-cd-2",
          tool_name: "CronDelete",
          input: { id: "abc12345" },
        } as BusEvent,
        {
          type: "tool_end",
          run_id: "run-cron",
          tool_use_id: "tool-cd-2",
          tool_name: "CronDelete",
          output: { success: true },
          status: "success",
          duration_ms: 4,
          tool_use_result: {},
        } as BusEvent,
      ]);
      expect(store.scheduledTasks).toHaveLength(0);
    });

    it("CronList does NOT mutate scheduledTasks (unverified shape)", () => {
      store.run = makeRun("run-cron");
      store.phase = "running";
      store.applyEventBatch(makeCronEvents());
      const before = [...store.scheduledTasks];
      store.applyEventBatch([
        {
          type: "tool_start",
          run_id: "run-cron",
          tool_use_id: "tool-cl-1",
          tool_name: "CronList",
          input: {},
        } as BusEvent,
        {
          type: "tool_end",
          run_id: "run-cron",
          tool_use_id: "tool-cl-1",
          tool_name: "CronList",
          output: { success: true },
          status: "success",
          duration_ms: 1,
          tool_use_result: { tasks: [] },
        } as BusEvent,
      ]);
      // Unchanged — we don't trust CronList shape yet
      expect(store.scheduledTasks).toEqual(before);
    });

    it("snapshot round-trip preserves scheduledTasks", () => {
      store.run = makeRun("run-cron");
      store.phase = "running";
      store.applyEventBatch(makeCronEvents());
      // Use private methods via cast to test snapshot machinery
      const snapBody = (store as unknown as { _buildSnapshot(): string })._buildSnapshot();
      const fresh = new SessionStore();
      fresh.run = makeRun("run-cron");
      const ok = (fresh as unknown as { _tryApplySnapshot(s: string): boolean })._tryApplySnapshot(
        snapBody,
      );
      expect(ok).toBe(true);
      expect(fresh.scheduledTasks).toHaveLength(1);
      expect(fresh.scheduledTasks[0]).toEqual(store.scheduledTasks[0]);
    });
  });

  describe("elicitation_prompt", () => {
    it("adds to pendingElicitations map keyed by request_id", () => {
      store.run = makeRun("run-1");
      store.phase = "running";
      store.applyEvent({
        type: "elicitation_prompt",
        run_id: "run-1",
        request_id: "req-elicit-1",
        mcp_server_name: "github-mcp",
        message: "Please authenticate",
        mode: "form",
        requested_schema: {
          type: "object",
          properties: {
            token: { type: "string", title: "Access Token" },
          },
          required: ["token"],
        },
      } as BusEvent);

      expect(store.pendingElicitations.size).toBe(1);
      expect(store.pendingElicitations.has("req-elicit-1")).toBe(true);
      expect(store.hasElicitation).toBe(true);
      expect(store.isThinking).toBe(false);
      expect(store.isActivelyRunning).toBe(false);

      const state = store.pendingElicitations.get("req-elicit-1")!;
      expect(state.mcpServerName).toBe("github-mcp");
      expect(state.message).toBe("Please authenticate");
      expect(state.mode).toBe("form");
    });

    it("control_cancelled removes from pendingElicitations", () => {
      store.run = makeRun("run-1");
      store.phase = "running";
      store.applyEventBatch([
        {
          type: "elicitation_prompt",
          run_id: "run-1",
          request_id: "req-elicit-1",
          mcp_server_name: "github-mcp",
          message: "Please authenticate",
        } as BusEvent,
        { type: "control_cancelled", run_id: "run-1", request_id: "req-elicit-1" },
      ]);

      expect(store.pendingElicitations.size).toBe(0);
      expect(store.hasElicitation).toBe(false);
    });

    it("removeElicitation clears specific entry", () => {
      store.run = makeRun("run-1");
      store.phase = "running";
      store.applyEventBatch([
        {
          type: "elicitation_prompt",
          run_id: "run-1",
          request_id: "req-elicit-1",
          mcp_server_name: "server-a",
          message: "Auth A",
        } as BusEvent,
        {
          type: "elicitation_prompt",
          run_id: "run-1",
          request_id: "req-elicit-2",
          mcp_server_name: "server-b",
          message: "Auth B",
        } as BusEvent,
      ]);

      expect(store.pendingElicitations.size).toBe(2);
      store.removeElicitation("req-elicit-1");
      expect(store.pendingElicitations.size).toBe(1);
      expect(store.pendingElicitations.has("req-elicit-2")).toBe(true);
    });

    it("_clearContentState clears all pending elicitations", () => {
      store.run = makeRun("run-1");
      store.phase = "running";
      store.applyEvent({
        type: "elicitation_prompt",
        run_id: "run-1",
        request_id: "req-elicit-1",
        mcp_server_name: "test",
        message: "test",
      } as BusEvent);

      expect(store.hasElicitation).toBe(true);

      // Trigger _clearContentState via reset
      store.reset();
      expect(store.pendingElicitations.size).toBe(0);
    });

    it("isThinking returns false when elicitation pending", () => {
      store.run = makeRun("run-1");
      store.phase = "running";
      // Without elicitation: isThinking should be true (running, no streaming text)
      expect(store.isThinking).toBe(true);

      store.applyEvent({
        type: "elicitation_prompt",
        run_id: "run-1",
        request_id: "req-1",
        mcp_server_name: "test",
        message: "test",
      } as BusEvent);

      expect(store.isThinking).toBe(false);
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

  describe("resolvePermissionAllow", () => {
    it("switches permission_prompt to running, preserves permission_request_id", () => {
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
          decision_reason: "",
        },
      ];
      store.applyEventBatch(events);
      const before = store.timeline.find((e) => e.kind === "tool" && e.id === "tool-1") as Extract<
        TimelineEntry,
        { kind: "tool" }
      >;
      expect(before.tool.status).toBe("permission_prompt");

      store.resolvePermissionAllow("req-1");

      const after = store.timeline.find((e) => e.kind === "tool" && e.id === "tool-1") as Extract<
        TimelineEntry,
        { kind: "tool" }
      >;
      expect(after.tool.status).toBe("running");
      expect(after.tool.permission_request_id).toBe("req-1");
    });

    it("skips AskUserQuestion tools", () => {
      store.run = makeRun("run-1");
      store.phase = "running";
      const events: BusEvent[] = [
        {
          type: "tool_start",
          run_id: "run-1",
          tool_use_id: "ask-1",
          tool_name: "AskUserQuestion",
          input: { question: "pick one" },
        },
        {
          type: "permission_prompt",
          run_id: "run-1",
          request_id: "req-ask",
          tool_name: "AskUserQuestion",
          tool_use_id: "ask-1",
          tool_input: { question: "pick one" },
          decision_reason: "",
        },
      ];
      store.applyEventBatch(events);

      store.resolvePermissionAllow("req-ask");

      const entry = store.timeline.find((e) => e.kind === "tool" && e.id === "ask-1") as Extract<
        TimelineEntry,
        { kind: "tool" }
      >;
      expect(entry.tool.status).toBe("permission_prompt"); // unchanged
    });

    it("updates subTimeline tool", () => {
      store.run = makeRun("run-1");
      store.phase = "running";
      const events: BusEvent[] = [
        {
          type: "tool_start",
          run_id: "run-1",
          tool_use_id: "task-parent",
          tool_name: "Task",
          input: { prompt: "do stuff" },
        },
        {
          type: "tool_start",
          run_id: "run-1",
          tool_use_id: "bash-child",
          tool_name: "Bash",
          input: { command: "ls" },
          parent_tool_use_id: "task-parent",
        },
        {
          type: "permission_prompt",
          run_id: "run-1",
          tool_use_id: "bash-child",
          tool_name: "Bash",
          tool_input: { command: "ls" },
          request_id: "req-sub",
          decision_reason: "",
        },
      ];
      store.applyEventBatch(events as BusEvent[]);

      store.resolvePermissionAllow("req-sub");

      const parent = store.timeline.find(
        (e) => e.kind === "tool" && e.id === "task-parent",
      ) as Extract<TimelineEntry, { kind: "tool" }>;
      const child = parent.subTimeline!.find(
        (e) => e.kind === "tool" && e.id === "bash-child",
      ) as Extract<TimelineEntry, { kind: "tool" }>;
      expect(child.tool.status).toBe("running");
      expect(child.tool.permission_request_id).toBe("req-sub");
    });

    it("skips AskUserQuestion in subTimeline", () => {
      store.run = makeRun("run-1");
      store.phase = "running";
      const events: BusEvent[] = [
        {
          type: "tool_start",
          run_id: "run-1",
          tool_use_id: "task-parent",
          tool_name: "Task",
          input: { prompt: "do stuff" },
        },
        {
          type: "tool_start",
          run_id: "run-1",
          tool_use_id: "ask-child",
          tool_name: "AskUserQuestion",
          input: { question: "which?" },
          parent_tool_use_id: "task-parent",
        },
        {
          type: "permission_prompt",
          run_id: "run-1",
          tool_use_id: "ask-child",
          tool_name: "AskUserQuestion",
          tool_input: { question: "which?" },
          request_id: "req-ask-sub",
          decision_reason: "",
        },
      ];
      store.applyEventBatch(events as BusEvent[]);

      store.resolvePermissionAllow("req-ask-sub");

      const parent = store.timeline.find(
        (e) => e.kind === "tool" && e.id === "task-parent",
      ) as Extract<TimelineEntry, { kind: "tool" }>;
      const child = parent.subTimeline!.find(
        (e) => e.kind === "tool" && e.id === "ask-child",
      ) as Extract<TimelineEntry, { kind: "tool" }>;
      expect(child.tool.status).toBe("permission_prompt"); // unchanged
    });

    it("no-ops for unmatched requestId", () => {
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
      ];
      store.applyEventBatch(events);
      const before = [...store.timeline];

      store.resolvePermissionAllow("req-nonexistent");

      expect(store.timeline).toEqual(before);
    });
  });

  describe("_resolveStaleTools (via idle/spawning/control_cancelled)", () => {
    it("idle resolves optimistic running (with permission_request_id) to error", () => {
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
      ];
      store.applyEventBatch(events);
      // Simulate optimistic allow
      store.resolvePermissionAllow("req-1");
      const mid = store.timeline.find((e) => e.kind === "tool" && e.id === "tool-1") as Extract<
        TimelineEntry,
        { kind: "tool" }
      >;
      expect(mid.tool.status).toBe("running");
      expect(mid.tool.permission_request_id).toBe("req-1");

      // Now idle arrives
      store.applyEvent({
        type: "run_state",
        run_id: "run-1",
        state: "idle",
      } as BusEvent);

      const after = store.timeline.find((e) => e.kind === "tool" && e.id === "tool-1") as Extract<
        TimelineEntry,
        { kind: "tool" }
      >;
      expect(after.tool.status).toBe("error");
    });

    it("idle does NOT resolve normal running tool (no permission_request_id)", () => {
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
      ];
      store.applyEventBatch(events);
      const before = store.timeline.find((e) => e.kind === "tool" && e.id === "tool-1") as Extract<
        TimelineEntry,
        { kind: "tool" }
      >;
      expect(before.tool.status).toBe("running");
      expect(before.tool.permission_request_id).toBeUndefined();

      store.applyEvent({
        type: "run_state",
        run_id: "run-1",
        state: "idle",
      } as BusEvent);

      const after = store.timeline.find((e) => e.kind === "tool" && e.id === "tool-1") as Extract<
        TimelineEntry,
        { kind: "tool" }
      >;
      expect(after.tool.status).toBe("running"); // unchanged
    });

    it("idle resolves stale subTimeline permission_prompt to error", () => {
      store.run = makeRun("run-1");
      store.phase = "running";
      const events: BusEvent[] = [
        {
          type: "tool_start",
          run_id: "run-1",
          tool_use_id: "task-parent",
          tool_name: "Task",
          input: { prompt: "do stuff" },
        },
        {
          type: "tool_start",
          run_id: "run-1",
          tool_use_id: "bash-child",
          tool_name: "Bash",
          input: { command: "ls" },
          parent_tool_use_id: "task-parent",
        },
        {
          type: "permission_prompt",
          run_id: "run-1",
          tool_use_id: "bash-child",
          tool_name: "Bash",
          tool_input: { command: "ls" },
          request_id: "req-sub",
          decision_reason: "",
        },
      ];
      store.applyEventBatch(events as BusEvent[]);

      store.applyEvent({
        type: "run_state",
        run_id: "run-1",
        state: "idle",
      } as BusEvent);

      const parent = store.timeline.find(
        (e) => e.kind === "tool" && e.id === "task-parent",
      ) as Extract<TimelineEntry, { kind: "tool" }>;
      const child = parent.subTimeline!.find(
        (e) => e.kind === "tool" && e.id === "bash-child",
      ) as Extract<TimelineEntry, { kind: "tool" }>;
      expect(child.tool.status).toBe("error");
    });

    it("idle resolves optimistic running in subTimeline to error", () => {
      store.run = makeRun("run-1");
      store.phase = "running";
      const events: BusEvent[] = [
        {
          type: "tool_start",
          run_id: "run-1",
          tool_use_id: "task-parent",
          tool_name: "Task",
          input: { prompt: "do stuff" },
        },
        {
          type: "tool_start",
          run_id: "run-1",
          tool_use_id: "bash-child",
          tool_name: "Bash",
          input: { command: "ls" },
          parent_tool_use_id: "task-parent",
        },
        {
          type: "permission_prompt",
          run_id: "run-1",
          tool_use_id: "bash-child",
          tool_name: "Bash",
          tool_input: { command: "ls" },
          request_id: "req-sub",
          decision_reason: "",
        },
      ];
      store.applyEventBatch(events as BusEvent[]);
      store.resolvePermissionAllow("req-sub");

      store.applyEvent({
        type: "run_state",
        run_id: "run-1",
        state: "idle",
      } as BusEvent);

      const parent = store.timeline.find(
        (e) => e.kind === "tool" && e.id === "task-parent",
      ) as Extract<TimelineEntry, { kind: "tool" }>;
      const child = parent.subTimeline!.find(
        (e) => e.kind === "tool" && e.id === "bash-child",
      ) as Extract<TimelineEntry, { kind: "tool" }>;
      expect(child.tool.status).toBe("error");
    });

    it("control_cancelled resolves optimistic running with matching request_id", () => {
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
          decision_reason: "",
        },
      ];
      store.applyEventBatch(events);
      store.resolvePermissionAllow("req-1");
      const mid = store.timeline.find((e) => e.kind === "tool" && e.id === "tool-1") as Extract<
        TimelineEntry,
        { kind: "tool" }
      >;
      expect(mid.tool.status).toBe("running");

      store.applyEvent({
        type: "control_cancelled",
        run_id: "run-1",
        request_id: "req-1",
      } as BusEvent);

      const after = store.timeline.find((e) => e.kind === "tool" && e.id === "tool-1") as Extract<
        TimelineEntry,
        { kind: "tool" }
      >;
      expect(after.tool.status).toBe("error");
    });

    it("control_cancelled resolves subTimeline permission_prompt with matching request_id", () => {
      store.run = makeRun("run-1");
      store.phase = "running";
      const events: BusEvent[] = [
        {
          type: "tool_start",
          run_id: "run-1",
          tool_use_id: "task-parent",
          tool_name: "Task",
          input: { prompt: "do stuff" },
        },
        {
          type: "tool_start",
          run_id: "run-1",
          tool_use_id: "bash-child",
          tool_name: "Bash",
          input: { command: "rm -rf /" },
          parent_tool_use_id: "task-parent",
        },
        {
          type: "permission_prompt",
          run_id: "run-1",
          tool_use_id: "bash-child",
          tool_name: "Bash",
          tool_input: { command: "rm -rf /" },
          request_id: "req-sub",
          decision_reason: "dangerous",
        },
        { type: "control_cancelled", run_id: "run-1", request_id: "req-sub" },
      ];
      store.applyEventBatch(events as BusEvent[]);

      const parent = store.timeline.find(
        (e) => e.kind === "tool" && e.id === "task-parent",
      ) as Extract<TimelineEntry, { kind: "tool" }>;
      const child = parent.subTimeline!.find(
        (e) => e.kind === "tool" && e.id === "bash-child",
      ) as Extract<TimelineEntry, { kind: "tool" }>;
      expect(child.tool.status).toBe("error");
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

    // CLI may return the same server from user/project/local scopes as
    // separate entries — UI collapses them to one row, first occurrence wins.
    it("deduplicates mcp_servers from session_init by name", () => {
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
            { name: "context7", status: "connected", scope: "user" },
            { name: "context7", status: "connected", scope: "project" },
            { name: "postgres", status: "connected" },
            { name: "context7", status: "failed", scope: "local" },
          ],
        },
      ];
      store.applyEventBatch(events as BusEvent[]);

      expect(store.mcpServers).toHaveLength(2);
      expect(store.mcpServers[0].name).toBe("context7");
      expect(store.mcpServers[0].scope).toBe("user");
      expect(store.mcpServers[1].name).toBe("postgres");
    });

    it("updateMcpServers deduplicates by name", () => {
      store.run = makeRun("run-1");
      store.phase = "running";
      store.updateMcpServers([
        { name: "context7", status: "connected" },
        { name: "context7", status: "connected" },
        { name: "github", status: "connected" },
      ]);
      expect(store.mcpServers).toHaveLength(2);
      expect(store.mcpServers[0].name).toBe("context7");
      expect(store.mcpServers[1].name).toBe("github");
    });
  });

  // ── canResumeRun ──

  describe("canResumeRun", () => {
    it("returns true for terminal run with session_id", () => {
      expect(canResumeRun({ session_id: "sess-1", status: "completed" }, "completed")).toBe(true);
      expect(canResumeRun({ session_id: "sess-1", status: "failed" }, "failed")).toBe(true);
      expect(canResumeRun({ session_id: "sess-1", status: "stopped" }, "stopped")).toBe(true);
    });

    it("returns false without session_id", () => {
      expect(canResumeRun({ status: "completed" }, "completed")).toBe(false);
    });

    it("returns false for active phases", () => {
      expect(canResumeRun({ session_id: "sess-1", status: "running" }, "running")).toBe(false);
      expect(canResumeRun({ session_id: "sess-1", status: "running" }, "spawning")).toBe(false);
    });

    it("returns true for conversation_ref without session_id (session_actor path)", () => {
      expect(
        canResumeRun(
          {
            conversation_ref: { kind: "claude_session", id: "s1" },
            execution_path: "session_actor",
            status: "completed",
          },
          "completed",
        ),
      ).toBe(true);
    });

    it("returns false for codex thread (resume UI not implemented)", () => {
      expect(
        canResumeRun(
          {
            conversation_ref: { kind: "codex_thread", id: "t1" },
            execution_path: "pipe_exec",
            status: "completed",
          },
          "completed",
        ),
      ).toBe(false);
    });

    it("returns false for null run", () => {
      expect(canResumeRun(null, "completed")).toBe(false);
    });

    it("returns false for non-terminal phases (idle, ready, empty)", () => {
      expect(canResumeRun({ session_id: "sess-1", status: "running" }, "idle")).toBe(false);
      expect(canResumeRun({ session_id: "sess-1" }, "ready")).toBe(false);
      expect(canResumeRun({ session_id: "sess-1" }, "empty")).toBe(false);
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
      expect(store.permissionMode).toBe("default"); // retains pre-reset value (user-level preference)
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
          tools: [],
          cwd: "",
          // output_style not present → should clear via ?? ""
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

    it("session_init does not overwrite permissionMode when permissionModeSetByUser is true", () => {
      store.run = makeRun("run-1");
      store.phase = "running";
      store.permissionMode = "bypassPermissions";
      store.permissionModeSetByUser = true;
      store.applyEventBatch([
        {
          type: "session_init",
          run_id: "run-1",
          session_id: "s1",
          permissionMode: "default",
        },
      ] as BusEvent[]);
      expect(store.permissionMode).toBe("bypassPermissions");
    });

    it("session_init fills permissionMode when permissionModeSetByUser is false", () => {
      store.run = makeRun("run-1");
      store.phase = "running";
      store.permissionMode = "bypassPermissions";
      store.permissionModeSetByUser = false;
      store.applyEventBatch([
        {
          type: "session_init",
          run_id: "run-1",
          session_id: "s1",
          permissionMode: "default",
        },
      ] as BusEvent[]);
      expect(store.permissionMode).toBe("default");
    });

    it("_clearContentState resets permissionModeSetByUser when permissionModePersistFailed", () => {
      store.permissionMode = "bypassPermissions";
      store.permissionModeSetByUser = true;
      store.permissionModePersistFailed = true;

      // Simulate loadRun (calls _clearContentState)
      (store as unknown as { _clearContentState(): void })._clearContentState();

      // Flag should be reset, mode retained
      expect(store.permissionModeSetByUser).toBe(false);
      expect(store.permissionModePersistFailed).toBe(false);
      expect(store.permissionMode).toBe("bypassPermissions"); // NOT cleared

      // Now session_init can re-sync
      store.run = makeRun("run-2");
      store.phase = "running";
      store.applyEventBatch([
        {
          type: "session_init",
          run_id: "run-2",
          session_id: "s2",
          permissionMode: "default",
        },
      ] as BusEvent[]);
      expect(store.permissionMode).toBe("default"); // re-synced from CLI
    });

    it("_clearContentState preserves permissionModeSetByUser when persist succeeded", () => {
      store.permissionMode = "bypassPermissions";
      store.permissionModeSetByUser = true;
      store.permissionModePersistFailed = false; // persist succeeded

      (store as unknown as { _clearContentState(): void })._clearContentState();

      expect(store.permissionModeSetByUser).toBe(true); // preserved
      expect(store.permissionMode).toBe("bypassPermissions"); // preserved
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
      { name: "protocol-events", runId: "run-1", events: protocolEvents },
      { name: "ralph-loop", runId: "run-ralph-1", events: ralphLoopEvents },
      { name: "scheduled-tasks", runId: "run-cron", events: scheduledTasksEvents },
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
        // NOTE: permissionMode intentionally excluded from snapshot — user-level preference
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

    describe("_tryApplySnapshot mcp dedup", () => {
      // Snapshots written before commit 550c8f4 may contain duplicate
      // mcpServers entries. Restoring must dedupe so old snapshots don't
      // continue to show 10 rows when CLI returned 3 distinct names.
      it("deduplicates mcpServers when restoring legacy snapshot", () => {
        const store1 = new SessionStore();
        const body = JSON.stringify({
          timeline: [],
          usage: { inputTokens: 0 },
          mcpServers: [
            { name: "context7", status: "connected", scope: "user" },
            { name: "context7", status: "connected", scope: "project" },
            { name: "github", status: "connected" },
            { name: "context7", status: "connected", scope: "local" },
          ],
        });
        const ok = (
          store1 as unknown as { _tryApplySnapshot(b: string): boolean }
        )._tryApplySnapshot(body);
        expect(ok).toBe(true);
        expect(store1.mcpServers).toHaveLength(2);
        expect(store1.mcpServers[0].name).toBe("context7");
        expect(store1.mcpServers[0].scope).toBe("user");
        expect(store1.mcpServers[1].name).toBe("github");
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
          { type: "run_state", run_id: "run-wg-1", state: "running" },
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

    // ── Idle snapshot paths ──

    describe("idle session snapshot", () => {
      it("snapshot hit (seq>0) → skip getBusEvents, phase = idle", async () => {
        const idleRun = makeRun("run-idle-1", { status: "idle", agent: "claude" });
        mockGetRun.mockResolvedValue(idleRun);

        // Build a snapshot with seq > 0
        const refStore = new SessionStore();
        refStore.run = idleRun;
        refStore.phase = "idle";
        refStore.applyEventBatch(simpleChatEvents as BusEvent[]);
        // Manually set _lastProcessedSeq > 0 to simulate a real snapshot
        (refStore as any)._lastProcessedSeq = 42;
        const snapshotBody = (refStore as any)._buildSnapshot();

        mockReadSnapshot.mockResolvedValue(snapshotBody);

        const testStore = new SessionStore();
        await testStore.loadRun("run-idle-1");

        expect(mockReadSnapshot).toHaveBeenCalledWith("run-idle-1", "idle");
        expect(mockGetBusEvents).not.toHaveBeenCalled();
        expect(testStore.phase).toBe("idle");
        expect(testStore.timeline.length).toBeGreaterThan(0);
        warnSpy.mockClear();
      });

      it("snapshot hit (seq=0) → deleteSnapshot + full replay", async () => {
        vi.useFakeTimers();
        const idleRun = makeRun("run-idle-2", { status: "idle", agent: "claude" });
        mockGetRun.mockResolvedValue(idleRun);

        // Build a snapshot with seq = 0 (no reliable seq)
        const refStore = new SessionStore();
        refStore.run = idleRun;
        refStore.phase = "idle";
        refStore.applyEventBatch(simpleChatEvents as BusEvent[]);
        (refStore as any)._lastProcessedSeq = 0; // seq=0
        const snapshotBody = (refStore as any)._buildSnapshot();

        mockReadSnapshot.mockResolvedValue(snapshotBody);
        mockGetBusEvents.mockResolvedValue(simpleChatEvents);

        const testStore = new SessionStore();
        await testStore.loadRun("run-idle-2");

        // seq=0 → snapshot skipped, stale entry deleted
        expect(mockDeleteSnapshot).toHaveBeenCalledWith("run-idle-2");
        // Full replay via getBusEvents
        expect(mockGetBusEvents).toHaveBeenCalledWith("run-idle-2");
        expect(testStore.timeline.length).toBeGreaterThan(0);
        // Flush deferred _saveSnapshotToIdb
        vi.advanceTimersByTime(1);
        vi.useRealTimers();
        warnSpy.mockClear();
      });

      it("snapshot miss → write snapshot for idle session", async () => {
        vi.useFakeTimers();
        const idleRun = makeRun("run-idle-3", { status: "idle", agent: "claude" });
        mockGetRun.mockResolvedValue(idleRun);
        mockReadSnapshot.mockResolvedValue(null);
        mockGetBusEvents.mockResolvedValue(simpleChatEvents);

        const testStore = new SessionStore();
        await testStore.loadRun("run-idle-3");

        // Flush deferred _saveSnapshotToIdb
        vi.advanceTimersByTime(1);
        expect(mockWriteSnapshot).toHaveBeenCalled();
        expect(testStore.timeline.length).toBeGreaterThan(0);
        vi.useRealTimers();
        warnSpy.mockClear();
      });

      it("idle→running transition → snapshot write blocked by status check", async () => {
        vi.useFakeTimers();
        const s = new SessionStore();
        s.run = makeRun("run-idle-race", { status: "idle", agent: "claude" });
        s.phase = "idle";
        s.applyEventBatch(simpleChatEvents as BusEvent[]);
        (s as any)._lastProcessedSeq = 10;

        // Trigger idle snapshot write (deferred via setTimeout)
        (s as any)._saveSnapshotToIdb("run-idle-race");

        // Simulate status change before setTimeout fires
        s.run = { ...s.run!, status: "running" };
        vi.advanceTimersByTime(1);

        // writeSnapshot should NOT have been called (status changed from idle→running)
        expect(mockWriteSnapshot).not.toHaveBeenCalled();
        vi.useRealTimers();
        warnSpy.mockClear();
      });

      it("consecutive idle with no new events → _lastSnapshotSeq throttles write", () => {
        const s = new SessionStore();
        s.run = makeRun("run-idle-throttle", { status: "running", agent: "claude" });
        s.phase = "running";

        // First idle transition: should write
        (s as any)._lastProcessedSeq = 5;
        (s as any)._lastSnapshotSeq = 0;
        const idleEvent = {
          type: "run_state",
          run_id: "run-idle-throttle",
          state: "idle",
        } as BusEvent;
        s.applyEvent(idleEvent);
        expect((s as any)._lastSnapshotSeq).toBe(5); // Updated

        // Second idle (no new events, seq unchanged): should NOT write
        // Reset to running first
        s.run = { ...s.run!, status: "running" };
        s.applyEvent({
          type: "run_state",
          run_id: "run-idle-throttle",
          state: "running",
        } as BusEvent);
        // Back to idle with same seq
        s.run = { ...s.run!, status: "running" };
        s.applyEvent(idleEvent);
        // _lastSnapshotSeq should still be 5 (not updated, seq didn't change)
        expect((s as any)._lastSnapshotSeq).toBe(5);
        warnSpy.mockClear();
      });
    });

    // ── Index fallback tests ──

    describe("reducer index fallback", () => {
      it("_findToolIdx fallback still correctly updates tool state", () => {
        const s = new SessionStore();
        s.run = makeRun("run-idx-1");
        s.phase = "running";

        // Add a tool via tool_start
        s.applyEvent({
          type: "tool_start",
          run_id: "run-idx-1",
          tool_use_id: "tool-fb-1",
          tool_name: "Bash",
          input: { command: "ls" },
        });

        // Corrupt the index to force fallback
        (s as any)._toolTlIndex.clear();

        // tool_end should still work via findIndex fallback
        s.applyEvent({
          type: "tool_end",
          run_id: "run-idx-1",
          tool_use_id: "tool-fb-1",
          tool_name: "Bash",
          output: { result: "ok" },
          status: "success",
        });

        const tool = s.timeline.find((e) => e.kind === "tool") as Extract<
          TimelineEntry,
          { kind: "tool" }
        >;
        expect(tool).toBeDefined();
        expect(tool.tool.status).toBe("success");
        // dbgWarn should have been called for the index miss
        warnSpy.mockClear();
      });

      it("seq=0 snapshot deleted, subsequent load does not re-hit stale entry", async () => {
        vi.useFakeTimers();
        const idleRun = makeRun("run-no-rehit", { status: "idle", agent: "claude" });
        mockGetRun.mockResolvedValue(idleRun);

        // First load: seq=0 snapshot
        const refStore = new SessionStore();
        refStore.run = idleRun;
        refStore.phase = "idle";
        refStore.applyEventBatch(simpleChatEvents as BusEvent[]);
        (refStore as any)._lastProcessedSeq = 0;
        const staleBody = (refStore as any)._buildSnapshot();

        mockReadSnapshot.mockResolvedValueOnce(staleBody);
        mockGetBusEvents.mockResolvedValue(simpleChatEvents);

        const testStore = new SessionStore();
        await testStore.loadRun("run-no-rehit");

        // deleteSnapshot called for seq=0
        expect(mockDeleteSnapshot).toHaveBeenCalledWith("run-no-rehit");

        // Second load: snapshot gone (readSnapshot returns null)
        mockReadSnapshot.mockResolvedValueOnce(null);
        mockGetBusEvents.mockResolvedValue(simpleChatEvents);

        const testStore2 = new SessionStore();
        await testStore2.loadRun("run-no-rehit");

        // Normal full replay — no snapshot hit
        expect(testStore2.timeline.length).toBeGreaterThan(0);
        vi.advanceTimersByTime(1);
        vi.useRealTimers();
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

    // ── Thinking text persistence ──

    describe("thinking text persistence", () => {
      it("persists thinkingText on main session message_complete", () => {
        const s = new SessionStore();
        s.run = makeRun("run-think-1");
        s.phase = "running";
        s.applyEventBatch([
          {
            type: "session_init",
            run_id: "run-think-1",
            model: "claude-opus-4-6",
            tools: [],
            cwd: "/",
            slash_commands: [],
            mcp_servers: [],
          } as BusEvent,
          {
            type: "run_state",
            run_id: "run-think-1",
            state: "running",
          } as BusEvent,
          {
            type: "thinking_delta",
            run_id: "run-think-1",
            text: "Let me reason about this...",
          } as BusEvent,
          {
            type: "thinking_delta",
            run_id: "run-think-1",
            text: " Step 2.",
          } as BusEvent,
          {
            type: "message_complete",
            run_id: "run-think-1",
            message_id: "msg-think-1",
            text: "Here is my answer.",
            ts: new Date().toISOString(),
          } as BusEvent,
        ]);

        const assistant = s.timeline.find(
          (e) => e.kind === "assistant" && e.id === "msg-think-1",
        ) as Extract<TimelineEntry, { kind: "assistant" }> | undefined;
        expect(assistant).toBeDefined();
        expect((assistant as Extract<TimelineEntry, { kind: "assistant" }>).thinkingText).toBe(
          "Let me reason about this... Step 2.",
        );
      });

      it("persists thinkingText on subagent message_complete", () => {
        const s = new SessionStore();
        s.run = makeRun("run-think-2");
        s.phase = "running";
        s.applyEventBatch([
          {
            type: "session_init",
            run_id: "run-think-2",
            model: "claude-opus-4-6",
            tools: [],
            cwd: "/",
            slash_commands: [],
            mcp_servers: [],
          } as BusEvent,
          {
            type: "run_state",
            run_id: "run-think-2",
            state: "running",
          } as BusEvent,
          // Parent tool start
          {
            type: "tool_start",
            run_id: "run-think-2",
            tool_use_id: "tu-parent",
            tool_name: "Task",
            input: {},
            ts: new Date().toISOString(),
          } as BusEvent,
          // Subagent thinking delta
          {
            type: "thinking_delta",
            run_id: "run-think-2",
            text: "Sub thinking...",
            parent_tool_use_id: "tu-parent",
          } as BusEvent,
          // Subagent message complete
          {
            type: "message_complete",
            run_id: "run-think-2",
            message_id: "msg-sub-1",
            text: "Subagent answer.",
            parent_tool_use_id: "tu-parent",
            ts: new Date().toISOString(),
          } as BusEvent,
        ]);

        // Find the parent tool entry
        const parentTool = s.timeline.find((e) => e.kind === "tool" && e.id === "tu-parent") as
          | Extract<TimelineEntry, { kind: "tool" }>
          | undefined;
        expect(parentTool).toBeDefined();
        expect(parentTool!.subTimeline).toBeDefined();
        const subAssistant = parentTool!.subTimeline!.find(
          (e) => e.kind === "assistant" && e.id === "msg-sub-1",
        ) as Extract<TimelineEntry, { kind: "assistant" }> | undefined;
        expect(subAssistant).toBeDefined();
        expect((subAssistant as Extract<TimelineEntry, { kind: "assistant" }>).thinkingText).toBe(
          "Sub thinking...",
        );
      });
    });
  });

  // ── Permission panel getters + resolve method improvements ──

  describe("permission panel getters", () => {
    function setupPermissionStore() {
      const s = new SessionStore();
      s.run = makeRun("run-perm") as any;
      s.phase = "running";
      return s;
    }

    function makeToolEntry(
      id: string,
      toolName: string,
      status: string,
      requestId?: string,
      subTimeline?: TimelineEntry[],
    ): TimelineEntry {
      return {
        kind: "tool",
        id,
        ts: new Date().toISOString(),
        tool: {
          tool_use_id: id,
          tool_name: toolName,
          status,
          permission_request_id: requestId,
          input: { file_path: `/src/${id}.ts` },
        } as any,
        subTimeline,
      } as any;
    }

    it("pendingToolPermissions collects top-level permission_prompt entries", () => {
      const s = setupPermissionStore();
      s.timeline = [
        makeToolEntry("t1", "Read", "permission_prompt", "req-1"),
        makeToolEntry("t2", "Write", "permission_prompt", "req-2"),
      ];
      const pending = s.pendingToolPermissions;
      expect(pending).toHaveLength(2);
      expect(pending[0].requestId).toBe("req-1");
      expect(pending[1].requestId).toBe("req-2");
    });

    it("pendingToolPermissions collects subTimeline permission_prompt entries", () => {
      const s = setupPermissionStore();
      s.timeline = [
        makeToolEntry("parent", "Task", "running", undefined, [
          makeToolEntry("child", "Bash", "permission_prompt", "req-child"),
        ]),
      ];
      const pending = s.pendingToolPermissions;
      expect(pending).toHaveLength(1);
      expect(pending[0].requestId).toBe("req-child");
      expect(pending[0].tool.tool_name).toBe("Bash");
    });

    it("pendingToolPermissions excludes AskUserQuestion and ExitPlanMode", () => {
      const s = setupPermissionStore();
      s.timeline = [
        makeToolEntry("t1", "AskUserQuestion", "permission_prompt", "req-ask"),
        makeToolEntry("t2", "ExitPlanMode", "permission_prompt", "req-exit"),
        makeToolEntry("t3", "Read", "permission_prompt", "req-read"),
      ];
      const pending = s.pendingToolPermissions;
      expect(pending).toHaveLength(1);
      expect(pending[0].requestId).toBe("req-read");
    });

    it("pendingToolPermissions excludes non-permission_prompt status", () => {
      const s = setupPermissionStore();
      s.timeline = [
        makeToolEntry("t1", "Read", "running"),
        makeToolEntry("t2", "Write", "success"),
        makeToolEntry("t3", "Edit", "permission_prompt", "req-edit"),
      ];
      const pending = s.pendingToolPermissions;
      expect(pending).toHaveLength(1);
      expect(pending[0].requestId).toBe("req-edit");
    });

    it("pendingToolPermissions excludes entries without permission_request_id", () => {
      const s = setupPermissionStore();
      s.timeline = [
        makeToolEntry("t1", "Read", "permission_prompt"), // no requestId
        makeToolEntry("t2", "Write", "permission_prompt", "req-write"),
      ];
      const pending = s.pendingToolPermissions;
      expect(pending).toHaveLength(1);
      expect(pending[0].requestId).toBe("req-write");
    });

    it("pendingToolPermissions deduplicates by requestId (last wins)", () => {
      const s = setupPermissionStore();
      s.timeline = [
        makeToolEntry("t1", "Read", "permission_prompt", "req-dup"),
        makeToolEntry("parent", "Task", "running", undefined, [
          makeToolEntry("t2", "Read", "permission_prompt", "req-dup"),
        ]),
      ];
      const pending = s.pendingToolPermissions;
      expect(pending).toHaveLength(1);
      expect(pending[0].requestId).toBe("req-dup");
      // Last one wins (subTimeline entry)
      expect(pending[0].tool.tool_use_id).toBe("t2");
    });

    it("hasPendingPermission recursively detects subTimeline permission_prompt", () => {
      const s = setupPermissionStore();
      s.timeline = [
        makeToolEntry("parent", "Task", "running", undefined, [
          makeToolEntry("child", "Read", "permission_prompt", "req-sub"),
        ]),
      ];
      expect(s.hasPendingPermission).toBe(true);
    });

    it("hasInlinePermission only matches AskUserQuestion and ExitPlanMode", () => {
      const s = setupPermissionStore();
      s.timeline = [
        makeToolEntry("t1", "Read", "permission_prompt", "req-1"),
        makeToolEntry("t2", "Write", "permission_prompt", "req-2"),
      ];
      expect(s.hasInlinePermission).toBe(false);

      s.timeline = [
        ...s.timeline,
        makeToolEntry("t3", "AskUserQuestion", "permission_prompt", "req-ask"),
      ];
      expect(s.hasInlinePermission).toBe(true);
    });

    it("hasInlinePermission recursively detects AskUserQuestion in subTimeline", () => {
      const s = setupPermissionStore();
      s.timeline = [
        makeToolEntry("parent", "Task", "running", undefined, [
          makeToolEntry("child", "AskUserQuestion", "permission_prompt", "req-ask-sub"),
        ]),
      ];
      expect(s.hasInlinePermission).toBe(true);
    });
  });

  describe("resolvePermissionDeny full traversal", () => {
    function makeToolEntry(
      id: string,
      toolName: string,
      status: string,
      requestId?: string,
      subTimeline?: TimelineEntry[],
    ): TimelineEntry {
      return {
        kind: "tool",
        id,
        ts: new Date().toISOString(),
        tool: {
          tool_use_id: id,
          tool_name: toolName,
          status,
          permission_request_id: requestId,
          input: {},
        } as any,
        subTimeline,
      } as any;
    }

    it("updates all entries matching requestId (no early return)", () => {
      const s = new SessionStore();
      s.run = makeRun("run-deny") as any;
      s.phase = "running";
      s.timeline = [
        makeToolEntry("t1", "Read", "permission_prompt", "req-dup"),
        makeToolEntry("parent", "Task", "running", undefined, [
          makeToolEntry("t2", "Read", "permission_prompt", "req-dup"),
        ]),
      ];

      s.resolvePermissionDeny("req-dup");

      // Both should be updated
      const top = s.timeline[0] as Extract<TimelineEntry, { kind: "tool" }>;
      expect(top.tool.status).toBe("permission_denied");

      const parent = s.timeline[1] as Extract<TimelineEntry, { kind: "tool" }>;
      const child = parent.subTimeline![0] as Extract<TimelineEntry, { kind: "tool" }>;
      expect(child.tool.status).toBe("permission_denied");
    });
  });

  describe("resolvePermissionAllow full traversal", () => {
    function makeToolEntry(
      id: string,
      toolName: string,
      status: string,
      requestId?: string,
      subTimeline?: TimelineEntry[],
    ): TimelineEntry {
      return {
        kind: "tool",
        id,
        ts: new Date().toISOString(),
        tool: {
          tool_use_id: id,
          tool_name: toolName,
          status,
          permission_request_id: requestId,
          input: {},
        } as any,
        subTimeline,
      } as any;
    }

    it("updates all entries matching requestId (no early return)", () => {
      const s = new SessionStore();
      s.run = makeRun("run-allow") as any;
      s.phase = "running";
      s.timeline = [
        makeToolEntry("t1", "Read", "permission_prompt", "req-dup"),
        makeToolEntry("parent", "Task", "running", undefined, [
          makeToolEntry("t2", "Read", "permission_prompt", "req-dup"),
        ]),
      ];

      s.resolvePermissionAllow("req-dup");

      const top = s.timeline[0] as Extract<TimelineEntry, { kind: "tool" }>;
      expect(top.tool.status).toBe("running");

      const parent = s.timeline[1] as Extract<TimelineEntry, { kind: "tool" }>;
      const child = parent.subTimeline![0] as Extract<TimelineEntry, { kind: "tool" }>;
      expect(child.tool.status).toBe("running");
    });

    it("skips AskUserQuestion but updates normal tools", () => {
      const s = new SessionStore();
      s.run = makeRun("run-allow-mix") as any;
      s.phase = "running";
      s.timeline = [
        makeToolEntry("t1", "AskUserQuestion", "permission_prompt", "req-mix"),
        makeToolEntry("t2", "Read", "permission_prompt", "req-mix"),
      ];

      s.resolvePermissionAllow("req-mix");

      const ask = s.timeline[0] as Extract<TimelineEntry, { kind: "tool" }>;
      expect(ask.tool.status).toBe("permission_prompt"); // unchanged

      const read = s.timeline[1] as Extract<TimelineEntry, { kind: "tool" }>;
      expect(read.tool.status).toBe("running"); // updated
    });

    it("skips AskUserQuestion in subTimeline", () => {
      const s = new SessionStore();
      s.run = makeRun("run-allow-sub") as any;
      s.phase = "running";
      s.timeline = [
        makeToolEntry("parent", "Task", "running", undefined, [
          makeToolEntry("ask-sub", "AskUserQuestion", "permission_prompt", "req-sub"),
          makeToolEntry("read-sub", "Read", "permission_prompt", "req-sub"),
        ]),
      ];

      s.resolvePermissionAllow("req-sub");

      const parent = s.timeline[0] as Extract<TimelineEntry, { kind: "tool" }>;
      const askChild = parent.subTimeline![0] as Extract<TimelineEntry, { kind: "tool" }>;
      expect(askChild.tool.status).toBe("permission_prompt"); // unchanged

      const readChild = parent.subTimeline![1] as Extract<TimelineEntry, { kind: "tool" }>;
      expect(readChild.tool.status).toBe("running"); // updated
    });
  });

  describe("multiple simultaneous permission_prompts via applyEvent (live mode)", () => {
    it("pendingToolPermissions returns 2 when two permission_prompts arrive sequentially", () => {
      const s = new SessionStore();
      s.run = makeRun("run-multi") as any;
      s.phase = "running";

      // tool_start A
      s.applyEvent({
        type: "tool_start",
        run_id: "run-multi",
        tool_use_id: "write-a",
        tool_name: "Write",
        input: { file_path: "/a.ts" },
      } as BusEvent);
      expect(s.timeline).toHaveLength(1);

      // permission_prompt A
      s.applyEvent({
        type: "permission_prompt",
        run_id: "run-multi",
        tool_use_id: "write-a",
        tool_name: "Write",
        request_id: "req-a",
        tool_input: { file_path: "/a.ts" },
        decision_reason: "",
      } as BusEvent);
      expect(s.pendingToolPermissions).toHaveLength(1);
      expect(s.pendingToolPermissions[0].requestId).toBe("req-a");

      // tool_start B
      s.applyEvent({
        type: "tool_start",
        run_id: "run-multi",
        tool_use_id: "write-b",
        tool_name: "Write",
        input: { file_path: "/b.ts" },
      } as BusEvent);
      expect(s.timeline).toHaveLength(2);

      // permission_prompt B
      s.applyEvent({
        type: "permission_prompt",
        run_id: "run-multi",
        tool_use_id: "write-b",
        tool_name: "Write",
        request_id: "req-b",
        tool_input: { file_path: "/b.ts" },
        decision_reason: "",
      } as BusEvent);

      // Both should be pending
      expect(s.pendingToolPermissions).toHaveLength(2);
      expect(s.pendingToolPermissions[0].requestId).toBe("req-a");
      expect(s.pendingToolPermissions[1].requestId).toBe("req-b");
      expect(s.hasPendingPermission).toBe(true);
    });

    it("pendingToolPermissions returns 2 via applyEventBatch (batched mode)", () => {
      const s = new SessionStore();
      s.run = makeRun("run-batch") as any;
      s.phase = "running";

      s.applyEventBatch([
        {
          type: "tool_start",
          run_id: "run-batch",
          tool_use_id: "read-1",
          tool_name: "Read",
          input: { file_path: "/x.ts" },
        },
        {
          type: "tool_start",
          run_id: "run-batch",
          tool_use_id: "read-2",
          tool_name: "Read",
          input: { file_path: "/y.ts" },
        },
        {
          type: "permission_prompt",
          run_id: "run-batch",
          tool_use_id: "read-1",
          tool_name: "Read",
          request_id: "req-1",
          tool_input: { file_path: "/x.ts" },
          decision_reason: "",
        },
        {
          type: "permission_prompt",
          run_id: "run-batch",
          tool_use_id: "read-2",
          tool_name: "Read",
          request_id: "req-2",
          tool_input: { file_path: "/y.ts" },
          decision_reason: "",
        },
      ] as BusEvent[]);

      expect(s.pendingToolPermissions).toHaveLength(2);
      expect(s.pendingToolPermissions[0].requestId).toBe("req-1");
      expect(s.pendingToolPermissions[1].requestId).toBe("req-2");
    });

    it("resolving one permission_prompt does not affect the other", () => {
      const s = new SessionStore();
      s.run = makeRun("run-resolve") as any;
      s.phase = "running";

      // Create two permission_prompts
      s.applyEventBatch([
        {
          type: "tool_start",
          run_id: "run-resolve",
          tool_use_id: "w1",
          tool_name: "Write",
          input: { file_path: "/a.ts" },
        },
        {
          type: "tool_start",
          run_id: "run-resolve",
          tool_use_id: "w2",
          tool_name: "Write",
          input: { file_path: "/b.ts" },
        },
        {
          type: "permission_prompt",
          run_id: "run-resolve",
          tool_use_id: "w1",
          tool_name: "Write",
          request_id: "req-w1",
          tool_input: { file_path: "/a.ts" },
          decision_reason: "",
        },
        {
          type: "permission_prompt",
          run_id: "run-resolve",
          tool_use_id: "w2",
          tool_name: "Write",
          request_id: "req-w2",
          tool_input: { file_path: "/b.ts" },
          decision_reason: "",
        },
      ] as BusEvent[]);

      expect(s.pendingToolPermissions).toHaveLength(2);

      // Allow only the first one
      s.resolvePermissionAllow("req-w1");

      // Only w2 should remain pending
      expect(s.pendingToolPermissions).toHaveLength(1);
      expect(s.pendingToolPermissions[0].requestId).toBe("req-w2");
      expect(s.pendingToolPermissions[0].tool.tool_name).toBe("Write");
    });

    it("synthetic permission_prompt (no preceding tool_start) still collected", () => {
      const s = new SessionStore();
      s.run = makeRun("run-synth") as any;
      s.phase = "running";

      // permission_prompt without preceding tool_start → creates synthetic entry
      s.applyEvent({
        type: "permission_prompt",
        run_id: "run-synth",
        tool_use_id: "synth-1",
        tool_name: "Bash",
        request_id: "req-synth-1",
        tool_input: { command: "ls" },
        decision_reason: "",
      } as BusEvent);

      s.applyEvent({
        type: "permission_prompt",
        run_id: "run-synth",
        tool_use_id: "synth-2",
        tool_name: "Bash",
        request_id: "req-synth-2",
        tool_input: { command: "cat foo" },
        decision_reason: "",
      } as BusEvent);

      expect(s.pendingToolPermissions).toHaveLength(2);
      expect(s.pendingToolPermissions[0].requestId).toBe("req-synth-1");
      expect(s.pendingToolPermissions[1].requestId).toBe("req-synth-2");
    });
  });

  // ── Codex agent state isolation ──

  describe("Codex agent state isolation", () => {
    it("codex agent sets protocol caps (bus-events only)", () => {
      store.agent = "codex";
      expect(store.caps.supportsBusEvents).toBe(true);
      expect(store.caps.supportsSessionInit).toBe(false);
      expect(store.caps.supportsPermissions).toBe(false);
      expect(store.useStreamSession).toBe(false);
    });

    it("claude agent sets full protocol caps", () => {
      store.agent = "claude";
      expect(store.caps.supportsBusEvents).toBe(true);
      expect(store.caps.supportsSessionInit).toBe(true);
      expect(store.caps.supportsPermissions).toBe(true);
      expect(store.caps.supportsSnapshots).toBe(true);
      expect(store.useStreamSession).toBe(true);
    });

    it("switching agent from claude to codex changes caps", () => {
      store.agent = "claude";
      expect(store.useStreamSession).toBe(true);

      store.agent = "codex";
      expect(store.useStreamSession).toBe(false);
      expect(store.caps.supportsSessionInit).toBe(false);
    });

    it("switching agent from codex to claude restores full caps", () => {
      store.agent = "codex";
      expect(store.caps.supportsSessionInit).toBe(false);

      store.agent = "claude";
      expect(store.caps.supportsSessionInit).toBe(true);
    });

    it("unknown agent gets minimal caps", () => {
      store.agent = "some-unknown";
      expect(store.caps.supportsBusEvents).toBe(false);
      expect(store.useStreamSession).toBe(false);
    });

    it("codex run loads without polluting Claude state", () => {
      // Simulate a codex run being loaded
      store.run = makeRun("codex-run-1", { agent: "codex", execution_path: "pipe_exec" });
      store.agent = "codex";
      store.phase = "running";

      // Model should be independent — codex doesn't use Claude model system
      store.model = "";
      expect(store.model).toBe("");

      // Caps should be codex
      expect(store.useStreamSession).toBe(false);
    });

    it("permissionModeSetByUser flag behavior", () => {
      // Initially false
      expect(store.permissionModeSetByUser).toBe(false);

      // Setting permissionMode directly doesn't set the flag
      store.permissionMode = "plan";
      expect(store.permissionModeSetByUser).toBe(false);

      // Explicitly setting the flag
      store.permissionModeSetByUser = true;
      expect(store.permissionModeSetByUser).toBe(true);

      // Resetting for agent switch
      store.permissionModeSetByUser = false;
      expect(store.permissionModeSetByUser).toBe(false);
    });
  });

  // ── Codex bus-events replay ──

  describe("Codex bus-events replay", () => {
    beforeEach(() => {
      store.agent = "codex";
      store.run = makeRun("codex-run-1", {
        agent: "codex",
        execution_path: "pipe_exec",
        conversation_ref: { kind: "codex_thread", id: "thread-abc" },
      });
      store._useChatTimelineForRun = true;
      store.phase = "running";
      store.applyEventBatch(codexSimpleEvents as BusEvent[]);
    });

    it("builds correct timeline from codex bus events", () => {
      // user_message + tool (bash) + message_complete = 3 entries
      expect(store.timeline).toHaveLength(3);
      expect(store.timeline[0].kind).toBe("user");
      expect(store.timeline[1].kind).toBe("tool");
      expect(store.timeline[2].kind).toBe("assistant");
    });

    it("user message has correct content", () => {
      const user = store.timeline[0];
      expect(user.kind).toBe("user");
      if (user.kind === "user") {
        expect(user.content).toBe("List files in the current directory");
      }
    });

    it("tool entry has Bash tool with success status", () => {
      const tool = store.timeline[1];
      expect(tool.kind).toBe("tool");
      if (tool.kind === "tool") {
        expect(tool.tool.tool_name).toBe("Bash");
        expect(tool.tool.status).toBe("success");
        expect(tool.tool.duration_ms).toBe(120);
      }
    });

    it("assistant message has correct text", () => {
      const assistant = store.timeline[2];
      expect(assistant.kind).toBe("assistant");
      if (assistant.kind === "assistant") {
        expect(assistant.content).toContain("README.md");
      }
    });

    it("tracks usage from turn.completed", () => {
      expect(store.usage.inputTokens).toBe(500);
      expect(store.usage.outputTokens).toBe(200);
      expect(store.usage.cacheReadTokens).toBe(100);
    });

    it("useChatTimeline returns true for codex with bus events", () => {
      expect(store.useChatTimeline).toBe(true);
      expect(store.useStreamSession).toBe(false); // pipe_exec
    });

    it("tools mirror is empty in bus-events mode (data in timeline)", () => {
      expect(store.tools).toHaveLength(0);
    });

    it("failed tool renders as error status (not success)", () => {
      const freshStore = new SessionStore();
      freshStore.agent = "codex";
      freshStore.run = makeRun("codex-err-1", {
        agent: "codex",
        execution_path: "pipe_exec",
      });
      freshStore._useChatTimelineForRun = true;
      freshStore.phase = "running";
      freshStore.applyEventBatch([
        {
          type: "tool_start",
          run_id: "codex-err-1",
          tool_use_id: "codex-1-1-cmd_fail",
          tool_name: "Bash",
          input: { command: "false" },
        },
        {
          type: "tool_end",
          run_id: "codex-err-1",
          tool_use_id: "codex-1-1-cmd_fail",
          tool_name: "Bash",
          output: { content: "" },
          status: "error", // backend maps Codex "failed" → "error"
          duration_ms: 10,
        },
      ] as BusEvent[]);
      const tool = freshStore.timeline.find(
        (e) => e.kind === "tool" && e.id === "codex-1-1-cmd_fail",
      ) as Extract<TimelineEntry, { kind: "tool" }>;
      expect(tool).toBeDefined();
      expect(tool.tool.status).toBe("error");
    });

    it("user_message with attachments restores on replay", () => {
      const freshStore = new SessionStore();
      freshStore.agent = "codex";
      freshStore.run = makeRun("codex-att-1", {
        agent: "codex",
        execution_path: "pipe_exec",
      });
      freshStore._useChatTimelineForRun = true;
      freshStore.phase = "running";
      freshStore.applyEventBatch(
        [
          {
            type: "user_message",
            run_id: "codex-att-1",
            text: "Check this image",
            attachments: [{ name: "screenshot.png", mime_type: "image/png", size: 12345 }],
          },
        ] as BusEvent[],
        { replayOnly: true },
      );
      const user = freshStore.timeline[0];
      expect(user.kind).toBe("user");
      if (user.kind === "user") {
        expect(user.attachments).toHaveLength(1);
        expect(user.attachments![0].name).toBe("screenshot.png");
        expect(user.attachments![0].type).toBe("image/png");
        expect(user.attachments![0].size).toBe(12345);
        expect(user.attachments![0].contentBase64).toBe("");
      }
    });

    it("client_uuid dedup merges optimistic entry", () => {
      // Simulate the flow: optimistic user push → bus event arrives with client_uuid
      const freshStore = new SessionStore();
      freshStore.agent = "codex";
      freshStore.run = makeRun("codex-run-2", {
        agent: "codex",
        execution_path: "pipe_exec",
      });
      freshStore._useChatTimelineForRun = true;
      freshStore.phase = "running";

      // Push optimistic with known id
      // @ts-expect-error — accessing private method for test
      const clientId = freshStore._pushOptimisticUser("hello codex");

      // Now apply the backend UserMessage with matching client_uuid
      freshStore.applyEvent({
        type: "user_message",
        run_id: "codex-run-2",
        text: "hello codex",
        client_uuid: clientId,
      } as BusEvent);

      // Should NOT duplicate — only 1 user entry
      const userEntries = freshStore.timeline.filter((e) => e.kind === "user");
      expect(userEntries).toHaveLength(1);
      expect(userEntries[0].content).toBe("hello codex");
    });

    it("old Phase 1 run (no conversation_ref) falls back to terminal", () => {
      const s = new SessionStore();
      s.agent = "codex";
      s.run = makeRun("codex-old-1", {
        agent: "codex",
        execution_path: "pipe_exec",
        status: "running",
        // No conversation_ref — Phase 1 run
      });
      s.phase = "running";
      // Simulate what loadRun does: no bus-events, active, no conversation_ref
      s._useChatTimelineForRun = false; // should NOT be true
      expect(s.useChatTimeline).toBe(false);
    });

    it("Phase 2 run with conversation_ref uses timeline", () => {
      const s = new SessionStore();
      s.agent = "codex";
      s.run = makeRun("codex-new-1", {
        agent: "codex",
        execution_path: "pipe_exec",
        status: "running",
        conversation_ref: { kind: "codex_thread", id: "thread-xyz" },
      });
      s._useChatTimelineForRun = true;
      s.phase = "running";
      expect(s.useChatTimeline).toBe(true);
    });

    it("replay duplicate user messages are NOT deduped (replayOnly=true)", () => {
      const s = new SessionStore();
      s.agent = "codex";
      s.run = makeRun("codex-dup-1", {
        agent: "codex",
        execution_path: "pipe_exec",
        conversation_ref: { kind: "codex_thread", id: "thread-dup" },
      });
      s._useChatTimelineForRun = true;
      s.phase = "running";
      // Two identical user messages in history — both must survive replay
      s.applyEventBatch(
        [
          { type: "user_message", run_id: "codex-dup-1", text: "same prompt" },
          { type: "message_complete", run_id: "codex-dup-1", message_id: "m1", text: "reply 1" },
          { type: "user_message", run_id: "codex-dup-1", text: "same prompt" },
          { type: "message_complete", run_id: "codex-dup-1", message_id: "m2", text: "reply 2" },
        ] as BusEvent[],
        { replayOnly: true },
      );
      const users = s.timeline.filter((e) => e.kind === "user");
      expect(users).toHaveLength(2);
    });
  });

  describe("Codex Wave-1 event rendering", () => {
    it("TodoWrite tool_end carries newTodos for the plan card (agent-neutral)", () => {
      const s = new SessionStore();
      s.applyEventBatch([
        {
          type: "tool_start",
          run_id: "codex-1",
          tool_use_id: "plan-turn-1",
          tool_name: "TodoWrite",
          input: { todos: [{ content: "step 1", status: "pending", activeForm: "doing 1" }] },
        },
        {
          type: "tool_end",
          run_id: "codex-1",
          tool_use_id: "plan-turn-1",
          tool_name: "TodoWrite",
          status: "success",
          tool_use_result: {
            newTodos: [{ content: "step 1", status: "in_progress", activeForm: "doing 1" }],
          },
        },
      ] as BusEvent[]);
      const tool = s.timeline.find(
        (e) => e.kind === "tool" && e.tool.tool_name === "TodoWrite",
      ) as Extract<TimelineEntry, { kind: "tool" }>;
      expect(tool).toBeDefined();
      expect(tool.tool.status).toBe("success");
      // ToolDetailView keys the styled todo card off tool_use_result.newTodos
      const result = tool.tool.tool_use_result as { newTodos: { status: string }[] };
      expect(result.newTodos).toHaveLength(1);
      expect(result.newTodos[0].status).toBe("in_progress");
    });

    it("repeated plan updates with a stable tool_use_id refresh the same card", () => {
      const s = new SessionStore();
      s.applyEventBatch([
        {
          type: "tool_start",
          run_id: "codex-1",
          tool_use_id: "plan-turn-1",
          tool_name: "TodoWrite",
          input: { todos: [] },
        },
        {
          type: "tool_end",
          run_id: "codex-1",
          tool_use_id: "plan-turn-1",
          tool_name: "TodoWrite",
          status: "success",
          tool_use_result: { newTodos: [{ content: "a", status: "pending", activeForm: "a" }] },
        },
        // Second update reuses the same tool_use_id
        {
          type: "tool_start",
          run_id: "codex-1",
          tool_use_id: "plan-turn-1",
          tool_name: "TodoWrite",
          input: { todos: [] },
        },
        {
          type: "tool_end",
          run_id: "codex-1",
          tool_use_id: "plan-turn-1",
          tool_name: "TodoWrite",
          status: "success",
          tool_use_result: { newTodos: [{ content: "a", status: "completed", activeForm: "a" }] },
        },
      ] as BusEvent[]);
      const tools = s.timeline.filter((e) => e.kind === "tool" && e.tool.tool_name === "TodoWrite");
      expect(tools).toHaveLength(1); // refreshed in place, not stacked
      const result = (tools[0] as Extract<TimelineEntry, { kind: "tool" }>).tool
        .tool_use_result as { newTodos: { status: string }[] };
      expect(result.newTodos[0].status).toBe("completed");
    });

    it("rate_limit_event populates store fields (agent-neutral, flows for Codex)", () => {
      const s = new SessionStore();
      s.applyEventBatch([
        {
          type: "rate_limit_event",
          run_id: "codex-1",
          status: "allowed_warning",
          rate_limit_type: "five_hour",
          utilization: 0.85,
          resets_at: 1711900000,
          data: {},
        },
      ] as BusEvent[]);
      expect(s.rateLimitStatus).toBe("allowed_warning");
      expect(s.rateLimitType).toBe("five_hour");
      expect(s.rateLimitUtilization).toBe(0.85);
      expect(s.rateLimitResetsAt).toBe(1711900000);
    });
  });

  // ── latestTodos getter (drives the TodoPanel) ──

  describe("latestTodos getter", () => {
    const td = (content: string, status: "pending" | "in_progress" | "completed") => ({
      content,
      status,
      activeForm: content,
    });

    function todoEvents(
      useId: string,
      todos: ReturnType<typeof td>[],
      status: "success" | "error" = "success",
    ): BusEvent[] {
      return [
        {
          type: "tool_start",
          run_id: "run-todo",
          tool_use_id: useId,
          tool_name: "TodoWrite",
          input: { todos },
        },
        {
          type: "tool_end",
          run_id: "run-todo",
          tool_use_id: useId,
          tool_name: "TodoWrite",
          status,
          tool_use_result: { oldTodos: [], newTodos: todos },
        },
      ] as BusEvent[];
    }

    beforeEach(() => {
      store.run = makeRun("run-todo");
      store.phase = "running";
    });

    it("returns the most recent TodoWrite's newTodos", () => {
      store.applyEventBatch(todoEvents("t1", [td("a", "completed"), td("b", "in_progress")]));
      store.applyEventBatch(
        todoEvents("t2", [td("a", "completed"), td("b", "completed"), td("c", "in_progress")]),
      );
      expect(store.latestTodos.map((t) => t.content)).toEqual(["a", "b", "c"]);
      expect(store.latestTodos[1].status).toBe("completed");
    });

    it("ignores non-TodoWrite tools", () => {
      store.applyEventBatch(todoEvents("t1", [td("a", "in_progress")]));
      store.applyEventBatch([
        {
          type: "tool_start",
          run_id: "run-todo",
          tool_use_id: "bash-1",
          tool_name: "Bash",
          input: { command: "ls" },
        },
        {
          type: "tool_end",
          run_id: "run-todo",
          tool_use_id: "bash-1",
          tool_name: "Bash",
          status: "success",
          tool_use_result: { stdout: "x" },
        },
      ] as BusEvent[]);
      expect(store.latestTodos.map((t) => t.content)).toEqual(["a"]);
    });

    it("returns [] when no TodoWrite has run", () => {
      expect(store.latestTodos).toEqual([]);
    });

    it("skips a failed TodoWrite and keeps the last successful one", () => {
      store.applyEventBatch(todoEvents("t1", [td("a", "completed")]));
      store.applyEventBatch(todoEvents("t2", [td("b", "in_progress")], "error"));
      expect(store.latestTodos.map((t) => t.content)).toEqual(["a"]);
    });
  });

  // ── taskList / panelTasks (Tasks system: TaskCreate/TaskUpdate) ──

  describe("taskList getter (Tasks system aggregation)", () => {
    function taskCreate(id: string, subject: string): BusEvent[] {
      return [
        {
          type: "tool_start",
          run_id: "run-task",
          tool_use_id: `c-${id}`,
          tool_name: "TaskCreate",
          input: { subject },
        },
        {
          type: "tool_end",
          run_id: "run-task",
          tool_use_id: `c-${id}`,
          tool_name: "TaskCreate",
          status: "success",
          tool_use_result: { task: { id, subject } },
        },
      ] as BusEvent[];
    }

    function taskUpdate(id: string, to: string, useId: string): BusEvent[] {
      return [
        {
          type: "tool_start",
          run_id: "run-task",
          tool_use_id: useId,
          tool_name: "TaskUpdate",
          input: { taskId: id, status: to },
        },
        {
          type: "tool_end",
          run_id: "run-task",
          tool_use_id: useId,
          tool_name: "TaskUpdate",
          status: "success",
          tool_use_result: { taskId: id, statusChange: { from: "pending", to }, success: true },
        },
      ] as BusEvent[];
    }

    beforeEach(() => {
      store.run = makeRun("run-task");
      store.phase = "running";
    });

    it("aggregates TaskCreate into a pending list, preserving creation order", () => {
      store.applyEventBatch(taskCreate("1", "build a.txt"));
      store.applyEventBatch(taskCreate("2", "build b.txt"));
      expect(store.taskList).toEqual([
        { id: "1", text: "build a.txt", status: "pending" },
        { id: "2", text: "build b.txt", status: "pending" },
      ]);
    });

    it("applies TaskUpdate status changes by taskId", () => {
      store.applyEventBatch(taskCreate("1", "a"));
      store.applyEventBatch(taskCreate("2", "b"));
      store.applyEventBatch(taskUpdate("1", "in_progress", "u1"));
      store.applyEventBatch(taskUpdate("1", "completed", "u2"));
      expect(store.taskList.map((t) => t.status)).toEqual(["completed", "pending"]);
    });

    it("removes a task when updated to deleted", () => {
      store.applyEventBatch(taskCreate("1", "a"));
      store.applyEventBatch(taskCreate("2", "b"));
      store.applyEventBatch(taskUpdate("1", "deleted", "u1"));
      expect(store.taskList.map((t) => t.id)).toEqual(["2"]);
    });
  });

  describe("panelTasks getter (unified source)", () => {
    beforeEach(() => {
      store.run = makeRun("run-panel");
      store.phase = "running";
    });

    it("prefers the Tasks system when TaskCreate events exist", () => {
      store.applyEventBatch([
        {
          type: "tool_start",
          run_id: "run-panel",
          tool_use_id: "c1",
          tool_name: "TaskCreate",
          input: {},
        },
        {
          type: "tool_end",
          run_id: "run-panel",
          tool_use_id: "c1",
          tool_name: "TaskCreate",
          status: "success",
          tool_use_result: { task: { id: "1", subject: "from tasks" } },
        },
      ] as BusEvent[]);
      expect(store.panelTasks).toEqual([{ id: "1", text: "from tasks", status: "pending" }]);
    });

    it("falls back to legacy TodoWrite when no Tasks events exist", () => {
      store.applyEventBatch([
        {
          type: "tool_start",
          run_id: "run-panel",
          tool_use_id: "w1",
          tool_name: "TodoWrite",
          input: {},
        },
        {
          type: "tool_end",
          run_id: "run-panel",
          tool_use_id: "w1",
          tool_name: "TodoWrite",
          status: "success",
          tool_use_result: {
            oldTodos: [],
            newTodos: [{ content: "from todo", status: "in_progress", activeForm: "from todo" }],
          },
        },
      ] as BusEvent[]);
      expect(store.panelTasks).toEqual([{ id: "0", text: "from todo", status: "in_progress" }]);
    });

    it("returns [] when neither source has run", () => {
      expect(store.panelTasks).toEqual([]);
    });
  });

  // ── context occupancy uses last request, not turn-summed usage (#149) ──

  describe("context occupancy (#149)", () => {
    beforeEach(() => {
      store.run = makeRun("run-ctx");
      store.phase = "running";
    });

    function msg(id: string, input: number, cacheRead: number, cacheCreation: number): BusEvent {
      return {
        type: "message_complete",
        run_id: "run-ctx",
        message_id: id,
        text: "ok",
        message_usage: {
          input_tokens: input,
          cache_read_input_tokens: cacheRead,
          cache_creation_input_tokens: cacheCreation,
        },
      } as BusEvent;
    }

    function result(cacheReadSummed: number, win: number): BusEvent {
      return {
        type: "usage_update",
        run_id: "run-ctx",
        input_tokens: 3000,
        output_tokens: 500,
        cache_read_tokens: cacheReadSummed,
        cache_write_tokens: 69000,
        total_cost_usd: 0.4,
        model_usage: {
          "claude-opus-4-8": {
            input_tokens: 3000,
            output_tokens: 500,
            cache_read_tokens: cacheReadSummed,
            cache_write_tokens: 69000,
            web_search_requests: 0,
            cost_usd: 0.4,
            context_window: win,
          },
        },
      } as BusEvent;
    }

    it("uses the last request's context, not the summed result usage", () => {
      store.applyEventBatch([
        { type: "user_message", run_id: "run-ctx", text: "go" },
        msg("m1", 1000, 0, 30000), // 31k
        msg("m2", 1000, 30000, 20000), // 51k
        msg("m3", 1000, 50000, 19000), // 70k ← last request
        result(80000, 1_000_000), // turn-summed cache_read inflated
      ] as BusEvent[]);
      expect(store.lastReqContextTokens).toBe(70000);
      // 70k, NOT the summed 3000 + 80000 + 69000 = 152000
      expect(store.contextTokens).toBe(70000);
    });

    it("falls back to summed usage when messages carry no per-request usage", () => {
      store.applyEventBatch([
        { type: "user_message", run_id: "run-ctx", text: "go" },
        { type: "message_complete", run_id: "run-ctx", message_id: "m1", text: "ok" },
        result(50000, 200_000),
      ] as BusEvent[]);
      expect(store.lastReqContextTokens).toBe(0);
      // fallback = input 3000 + cache_read 50000 + cache_write 69000
      expect(store.contextTokens).toBe(122000);
    });
  });
});

// ── Codex Wave-2: mid-turn steer routing (sendMessage) ──

describe("sendMessage routing", () => {
  let store: SessionStore;

  beforeEach(() => {
    vi.clearAllMocks();
    store = new SessionStore();
  });

  it("routes to steer when a Codex app-server turn is RUNNING", async () => {
    store.run = makeRun("codex-steer", { agent: "codex", status: "running" });
    store.phase = "running"; // isRunning + sessionAlive

    await store.sendMessage("focus on the auth module", []);

    expect(api.steerSession).toHaveBeenCalledWith("codex-steer", "focus on the auth module");
    expect(api.sendSessionMessage).not.toHaveBeenCalled();
  });

  it("enqueues (sendSessionMessage) for a Codex app-server turn that is NOT running", async () => {
    store.run = makeRun("codex-idle", { agent: "codex", status: "running" });
    store.phase = "idle"; // sessionAlive but not running

    await store.sendMessage("next task please", []);

    expect(api.sendSessionMessage).toHaveBeenCalledWith(
      "codex-idle",
      "next task please",
      undefined,
      undefined, // no skills
    );
    expect(api.steerSession).not.toHaveBeenCalled();
  });

  it("does NOT steer for a running Claude session (enqueues instead)", async () => {
    store.run = makeRun("claude-run", { agent: "claude", status: "running" });
    store.phase = "running";

    await store.sendMessage("more detail", []);

    expect(api.steerSession).not.toHaveBeenCalled();
    expect(api.sendSessionMessage).toHaveBeenCalledWith(
      "claude-run",
      "more detail",
      undefined,
      undefined, // no skills
    );
  });

  it("falls through to enqueue when a running Codex send carries attachments", async () => {
    store.run = makeRun("codex-attach", { agent: "codex", status: "running" });
    store.phase = "running";

    await store.sendMessage("see this", [
      { name: "a.png", type: "image/png", size: 3, contentBase64: "AAA" },
    ]);

    // turn/steer takes plain text only — attachments use the normal enqueue path.
    expect(api.steerSession).not.toHaveBeenCalled();
    expect(api.sendSessionMessage).toHaveBeenCalled();
  });

  it("forwards picked Codex skills to sendSessionMessage", async () => {
    store.run = makeRun("codex-skill", { agent: "codex", status: "running" });
    store.phase = "idle"; // sessionAlive but not running → enqueue path

    const skills = [{ name: "find-bugs", path: "/abs/find-bugs/SKILL.md" }];
    await store.sendMessage("scan this", [], skills);

    expect(api.sendSessionMessage).toHaveBeenCalledWith(
      "codex-skill",
      "scan this",
      undefined,
      skills,
    );
  });

  it("does NOT steer a running Codex turn when skills are attached (skills need turn/start)", async () => {
    store.run = makeRun("codex-skill-run", { agent: "codex", status: "running" });
    store.phase = "running"; // running → would normally steer, but skills force enqueue

    const skills = [{ name: "find-bugs", path: "/abs/find-bugs/SKILL.md" }];
    await store.sendMessage("scan this", [], skills);

    expect(api.steerSession).not.toHaveBeenCalled();
    expect(api.sendSessionMessage).toHaveBeenCalledWith(
      "codex-skill-run",
      "scan this",
      undefined,
      skills,
    );
  });
});

// ── Codex Wave-3: goal_update reducer + turn-based rewind ──

describe("SessionStore — Codex Wave-3 (goal + rewind)", () => {
  let store: SessionStore;

  beforeEach(() => {
    store = new SessionStore();
    store.run = makeRun("codex-w3", { agent: "codex" });
    store.phase = "running";
  });

  describe("goal_update reducer", () => {
    it("stores the goal payload on first update", () => {
      store.applyEvent({
        type: "goal_update",
        run_id: "codex-w3",
        goal: { objective: "Ship feature", status: "active", tokenBudget: 100000, tokensUsed: 0 },
      } as BusEvent);

      expect(store.goal).toEqual({
        objective: "Ship feature",
        status: "active",
        tokenBudget: 100000,
        tokensUsed: 0,
      });
    });

    it("merges successive updates (live progress climbs without losing objective)", () => {
      store.applyEvent({
        type: "goal_update",
        run_id: "codex-w3",
        goal: { objective: "Ship feature", status: "active", tokenBudget: 100000, tokensUsed: 0 },
      } as BusEvent);
      store.applyEvent({
        type: "goal_update",
        run_id: "codex-w3",
        goal: { tokensUsed: 4200, timeUsedSeconds: 30 },
      } as BusEvent);

      expect(store.goal).toEqual({
        objective: "Ship feature",
        status: "active",
        tokenBudget: 100000,
        tokensUsed: 4200,
        timeUsedSeconds: 30,
      });
    });

    it("resets to null when the goal is cleared (goal: null)", () => {
      store.applyEvent({
        type: "goal_update",
        run_id: "codex-w3",
        goal: { objective: "x", status: "active", tokensUsed: 10 },
      } as BusEvent);
      expect(store.goal).not.toBeNull();

      store.applyEvent({ type: "goal_update", run_id: "codex-w3", goal: null } as BusEvent);
      expect(store.goal).toBeNull();
    });

    it("is cleared by reset()", () => {
      store.applyEvent({
        type: "goal_update",
        run_id: "codex-w3",
        goal: { objective: "x", status: "active" },
      } as BusEvent);
      expect(store.goal).not.toBeNull();

      store.reset();
      expect(store.goal).toBeNull();
    });
  });

  describe("truncateToTurn", () => {
    // Build a 3-turn timeline: each turn = user_message + message_complete.
    function seedThreeTurns() {
      store.applyEventBatch([
        { type: "user_message", run_id: "codex-w3", text: "turn 1" },
        { type: "message_complete", run_id: "codex-w3", message_id: "m1", text: "reply 1" },
        { type: "user_message", run_id: "codex-w3", text: "turn 2" },
        { type: "message_complete", run_id: "codex-w3", message_id: "m2", text: "reply 2" },
        { type: "user_message", run_id: "codex-w3", text: "turn 3" },
        { type: "message_complete", run_id: "codex-w3", message_id: "m3", text: "reply 3" },
      ] as BusEvent[]);
    }

    it("drops the selected turn and everything after it", () => {
      seedThreeTurns();
      expect(store.timeline.filter((e) => e.kind === "user")).toHaveLength(3);

      // Rewind to turn index 1 (the 2nd turn) → drop turns 2 and 3.
      const dropped = store.truncateToTurn(1);

      expect(dropped).toBe(2);
      const users = store.timeline.filter((e) => e.kind === "user");
      expect(users).toHaveLength(1);
      expect((users[0] as { content: string }).content).toBe("turn 1");
      // numTurns is recomputed from surviving user messages on a real truncate.
      expect(store.numTurns).toBe(1);
    });

    it("keeps everything when index is past the last turn (no-op)", () => {
      seedThreeTurns();
      store.numTurns = 3; // as the result event would have set it
      const before = store.timeline.length;

      const dropped = store.truncateToTurn(3); // only indices 0..2 exist

      expect(dropped).toBe(0);
      expect(store.timeline).toHaveLength(before);
      expect(store.numTurns).toBe(3); // untouched on no-op
    });

    it("clears streaming buffers for the removed latest turn", () => {
      seedThreeTurns();
      store.streamingText = "partial";
      store.thinkingText = "thinking";

      store.truncateToTurn(0); // drop all turns

      expect(store.timeline.filter((e) => e.kind === "user")).toHaveLength(0);
      expect(store.streamingText).toBe("");
      expect(store.thinkingText).toBe("");
    });
  });
});

// ── Codex Wave-4: codex_hook_run reducer (hook lifecycle upsert) ──

describe("SessionStore — Codex Wave-4 (hook lifecycle)", () => {
  let store: SessionStore;

  beforeEach(() => {
    store = new SessionStore();
    store.run = makeRun("codex-w4", { agent: "codex" });
    store.phase = "running";
  });

  function hookEntries() {
    return store.timeline.filter((e) => e.kind === "hook") as Extract<
      (typeof store.timeline)[number],
      { kind: "hook" }
    >[];
  }

  it("creates a running hook card on hook/started", () => {
    store.applyEvent({
      type: "codex_hook_run",
      run_id: "codex-w4",
      hook_id: "hook-run-7",
      event_name: "preToolUse",
      status: "running",
    } as BusEvent);

    const hooks = hookEntries();
    expect(hooks).toHaveLength(1);
    expect(hooks[0].hookId).toBe("hook-run-7");
    expect(hooks[0].eventName).toBe("preToolUse");
    expect(hooks[0].status).toBe("running");
    // started carries no duration/message → optional fields stay unset.
    expect(hooks[0].durationMs).toBeUndefined();
    expect(hooks[0].statusMessage).toBeUndefined();
  });

  it("upserts: completed updates the SAME entry, not a second card", () => {
    store.applyEvent({
      type: "codex_hook_run",
      run_id: "codex-w4",
      hook_id: "hook-run-7",
      event_name: "preToolUse",
      status: "running",
    } as BusEvent);
    const startedId = hookEntries()[0].id;

    store.applyEvent({
      type: "codex_hook_run",
      run_id: "codex-w4",
      hook_id: "hook-run-7",
      event_name: "preToolUse",
      status: "completed",
      duration_ms: 1234,
    } as BusEvent);

    const hooks = hookEntries();
    // Still ONE card — keyed by hook_id, updated in place.
    expect(hooks).toHaveLength(1);
    expect(hooks[0].id).toBe(startedId);
    expect(hooks[0].status).toBe("completed");
    expect(hooks[0].durationMs).toBe(1234);
  });

  it("merges status_message on completion while keeping prior fields", () => {
    store.applyEvent({
      type: "codex_hook_run",
      run_id: "codex-w4",
      hook_id: "hook-run-8",
      event_name: "permissionRequest",
      status: "running",
    } as BusEvent);
    store.applyEvent({
      type: "codex_hook_run",
      run_id: "codex-w4",
      hook_id: "hook-run-8",
      event_name: "permissionRequest",
      status: "blocked",
      status_message: "denied by policy",
      duration_ms: 42,
    } as BusEvent);

    const hooks = hookEntries();
    expect(hooks).toHaveLength(1);
    expect(hooks[0].status).toBe("blocked");
    expect(hooks[0].statusMessage).toBe("denied by policy");
    expect(hooks[0].durationMs).toBe(42);
    // eventName from the started event is preserved across the upsert.
    expect(hooks[0].eventName).toBe("permissionRequest");
  });

  it("keeps distinct hook_ids as distinct cards", () => {
    store.applyEvent({
      type: "codex_hook_run",
      run_id: "codex-w4",
      hook_id: "hook-a",
      event_name: "preToolUse",
      status: "running",
    } as BusEvent);
    store.applyEvent({
      type: "codex_hook_run",
      run_id: "codex-w4",
      hook_id: "hook-b",
      event_name: "postToolUse",
      status: "running",
    } as BusEvent);

    const hooks = hookEntries();
    expect(hooks).toHaveLength(2);
    expect(hooks.map((h) => h.hookId).sort()).toEqual(["hook-a", "hook-b"]);
  });

  it("is a known event type under strictMode (0 unknown, no throw)", () => {
    // codex_hook_run must hit its reducer case, not the `default` unknown branch.
    const strict = new SessionStore();
    strict.strictMode = true;
    strict.run = makeRun("codex-w4", { agent: "codex" });
    strict.phase = "running";
    expect(() =>
      strict.applyEventBatch([
        {
          type: "codex_hook_run",
          run_id: "codex-w4",
          hook_id: "hook-strict",
          event_name: "sessionStart",
          status: "running",
        },
        {
          type: "codex_hook_run",
          run_id: "codex-w4",
          hook_id: "hook-strict",
          event_name: "sessionStart",
          status: "completed",
          duration_ms: 5,
        },
      ] as BusEvent[]),
    ).not.toThrow();
    expect(strict.unknownEventCount).toBe(0);
    expect(strict.rawFallbackCount).toBe(0);
  });
});

// ── Codex Wave-4: codex_mcp_status reducer (live MCP startup-state) ──

describe("SessionStore — Codex Wave-4 (MCP live status)", () => {
  let store: SessionStore;

  beforeEach(() => {
    store = new SessionStore();
    store.run = makeRun("codex-w4", { agent: "codex" });
    store.phase = "running";
  });

  it("maps Codex startup states to the panel vocab and upserts by name", () => {
    // Unknown server → appended.
    store.applyEvent({
      type: "codex_mcp_status",
      run_id: "codex-w4",
      name: "codex_apps",
      status: "starting",
    } as BusEvent);
    expect(store.mcpServers).toHaveLength(1);
    expect(store.mcpServers[0]).toMatchObject({ name: "codex_apps", status: "pending" });

    // Same name → updated in place (still one entry), ready → connected.
    store.applyEvent({
      type: "codex_mcp_status",
      run_id: "codex-w4",
      name: "codex_apps",
      status: "ready",
    } as BusEvent);
    expect(store.mcpServers).toHaveLength(1);
    expect(store.mcpServers[0].status).toBe("connected");

    // failed carries the error through; cancelled also maps to failed.
    store.applyEvent({
      type: "codex_mcp_status",
      run_id: "codex-w4",
      name: "codex_apps",
      status: "failed",
      error: "handshake failed",
    } as BusEvent);
    expect(store.mcpServers[0].status).toBe("failed");
    expect(store.mcpServers[0].error).toBe("handshake failed");

    store.applyEvent({
      type: "codex_mcp_status",
      run_id: "codex-w4",
      name: "codex_apps",
      status: "cancelled",
    } as BusEvent);
    expect(store.mcpServers[0].status).toBe("failed");
  });

  it("is a known event type (no unknown/raw fallback)", () => {
    const strict = new SessionStore();
    strict.run = makeRun("codex-w4", { agent: "codex" });
    strict.strictMode = true;
    expect(() =>
      strict.applyEvent({
        type: "codex_mcp_status",
        run_id: "codex-w4",
        name: "s1",
        status: "ready",
      } as BusEvent),
    ).not.toThrow();
    expect(strict.unknownEventCount).toBe(0);
  });
});

// ── Codex Wave-4 (G4): codex_turn_diff reducer (live aggregated turn diff) ──

describe("SessionStore — Codex Wave-4 (turn diff live)", () => {
  let store: SessionStore;

  beforeEach(() => {
    store = new SessionStore();
    store.run = makeRun("codex-w4d", { agent: "codex" });
    store.phase = "running";
  });

  it("stores the latest diff; a later event supersedes the earlier one", () => {
    expect(store.turnDiff).toBe("");

    store.applyEvent({
      type: "codex_turn_diff",
      run_id: "codex-w4d",
      turn_id: "tu1",
      diff: "--- a\n+++ b\n@@ -1 +1 @@\n-old\n+new",
    } as BusEvent);
    expect(store.turnDiff).toBe("--- a\n+++ b\n@@ -1 +1 @@\n-old\n+new");

    // Cumulative push — later event wins (diff is aggregated server-side).
    store.applyEvent({
      type: "codex_turn_diff",
      run_id: "codex-w4d",
      turn_id: "tu1",
      diff: "--- a\n+++ b\n@@ -1,2 +1,2 @@\n-old\n+new\n+extra",
    } as BusEvent);
    expect(store.turnDiff).toBe("--- a\n+++ b\n@@ -1,2 +1,2 @@\n-old\n+new\n+extra");
  });

  it("clears the diff at the next turn start (run_state→running)", () => {
    store.applyEvent({
      type: "codex_turn_diff",
      run_id: "codex-w4d",
      turn_id: "tu1",
      diff: "some diff",
    } as BusEvent);
    expect(store.turnDiff).toBe("some diff");

    // Next turn begins — the previous turn's aggregated diff is dropped.
    store.applyEvent({
      type: "run_state",
      run_id: "codex-w4d",
      state: "running",
    } as BusEvent);
    expect(store.turnDiff).toBe("");
  });

  it("is a known event type (no unknown/raw fallback)", () => {
    const strict = new SessionStore();
    strict.run = makeRun("codex-w4d", { agent: "codex" });
    strict.strictMode = true;
    expect(() =>
      strict.applyEvent({
        type: "codex_turn_diff",
        run_id: "codex-w4d",
        turn_id: "tu1",
        diff: "d",
      } as BusEvent),
    ).not.toThrow();
    expect(strict.unknownEventCount).toBe(0);
  });
});

describe("SessionStart sessionTitle auto-naming", () => {
  function sessionStartHook(runId: string, title: string | null): BusEvent {
    return {
      type: "hook_response",
      run_id: runId,
      hook_id: "h-sessiontitle",
      hook_event: "SessionStart",
      outcome: "success",
      data: {},
      stdout:
        title === null
          ? '{"hookSpecificOutput":{"hookEventName":"SessionStart"}}'
          : `{"hookSpecificOutput":{"hookEventName":"SessionStart","sessionTitle":${JSON.stringify(title)}}}`,
    } as BusEvent;
  }

  beforeEach(() => {
    vi.mocked(api.renameRun).mockClear();
  });

  it("sets run.name from sessionTitle when the run has no name", async () => {
    const store = new SessionStore();
    store.run = makeRun("run-st1");
    store.applyEvent(sessionStartHook("run-st1", "Refactor auth flow"));
    expect(store.run?.name).toBe("Refactor auth flow");
    await vi.waitFor(() =>
      expect(api.renameRun).toHaveBeenCalledWith("run-st1", "Refactor auth flow"),
    );
  });

  it("does NOT overwrite a user-assigned name (resume case)", () => {
    const store = new SessionStore();
    store.run = makeRun("run-st2", { name: "My Custom Name" });
    store.applyEvent(sessionStartHook("run-st2", "Hook Title"));
    expect(store.run?.name).toBe("My Custom Name");
    expect(api.renameRun).not.toHaveBeenCalled();
  });

  it("ignores a SessionStart hook with no sessionTitle", () => {
    const store = new SessionStore();
    store.run = makeRun("run-st3");
    store.applyEvent(sessionStartHook("run-st3", null));
    expect(store.run?.name).toBeUndefined();
    expect(api.renameRun).not.toHaveBeenCalled();
  });

  it("ignores sessionTitle from non-SessionStart hooks", () => {
    const store = new SessionStore();
    store.run = makeRun("run-st4");
    store.applyEvent({
      ...sessionStartHook("run-st4", "Should Be Ignored"),
      hook_event: "PreToolUse",
    } as BusEvent);
    expect(store.run?.name).toBeUndefined();
    expect(api.renameRun).not.toHaveBeenCalled();
  });
});
