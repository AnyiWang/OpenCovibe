import { describe, it, expect, vi } from "vitest";
import { executeAddDir, type AddDirDeps } from "../add-dir";

function makeDeps(overrides: Partial<AddDirDeps> = {}): AddDirDeps {
  return {
    openDirectoryDialog: vi.fn().mockResolvedValue("/selected/dir"),
    sendMessage: vi.fn().mockResolvedValue(undefined),
    getAgentSettings: vi.fn().mockResolvedValue({ add_dirs: [] }),
    updateAgentSettings: vi.fn().mockResolvedValue(undefined),
    appendOutput: vi.fn(),
    t: vi.fn((key: string, params?: Record<string, string>) => {
      if (params) return `[${key}] ${JSON.stringify(params)}`;
      return `[${key}]`;
    }),
    ...overrides,
  };
}

describe("executeAddDir", () => {
  it("unknown agent shows unsupported message, does not open dialog", async () => {
    const deps = makeDeps();
    await executeAddDir({ agent: "unknown-agent", sessionAlive: false, args: "" }, deps);

    expect(deps.appendOutput).toHaveBeenCalledWith("[chat_addDirUnsupported]");
    expect(deps.openDirectoryDialog).not.toHaveBeenCalled();
  });

  // ── Codex-specific tests ──

  it("codex + sessionAlive saves to settings (does not sendMessage), reports next-session", async () => {
    // Codex lacks supportsLiveAddDir: it consumes writableRoots once at spawn
    // (first turn/start), so a mid-session add-dir is persisted and only takes
    // effect on the next session / new thread — never the current thread. With a
    // live session we surface the "next session" wording.
    const deps = makeDeps({
      getAgentSettings: vi.fn().mockResolvedValue({ add_dirs: [] }),
    });
    await executeAddDir({ agent: "codex", sessionAlive: true, args: "/tmp/codex-dir" }, deps);

    expect(deps.sendMessage).not.toHaveBeenCalled();
    expect(deps.updateAgentSettings).toHaveBeenCalledWith("codex", {
      add_dirs: ["/tmp/codex-dir"],
    });
    const call = (deps.appendOutput as ReturnType<typeof vi.fn>).mock.calls[0][0] as string;
    expect(call).toContain("chat_addDirNextSession");
  });

  it("codex + sessionAlive=false is a plain pre-session save", async () => {
    const deps = makeDeps({
      getAgentSettings: vi.fn().mockResolvedValue({ add_dirs: [] }),
    });
    await executeAddDir({ agent: "codex", sessionAlive: false, args: "/tmp/codex-dir" }, deps);

    expect(deps.sendMessage).not.toHaveBeenCalled();
    expect(deps.updateAgentSettings).toHaveBeenCalledWith("codex", {
      add_dirs: ["/tmp/codex-dir"],
    });
    const call = (deps.appendOutput as ReturnType<typeof vi.fn>).mock.calls[0][0] as string;
    expect(call).toContain("chat_addDirSaved");
  });

  it("user cancels dialog — no further action", async () => {
    const deps = makeDeps({
      openDirectoryDialog: vi.fn().mockResolvedValue(null),
    });
    await executeAddDir({ agent: "claude", sessionAlive: true, args: "" }, deps);

    expect(deps.openDirectoryDialog).toHaveBeenCalled();
    expect(deps.sendMessage).not.toHaveBeenCalled();
    expect(deps.updateAgentSettings).not.toHaveBeenCalled();
    expect(deps.appendOutput).not.toHaveBeenCalled();
  });

  it("session alive + valid path (dialog) sends /add-dir to CLI", async () => {
    const deps = makeDeps({
      openDirectoryDialog: vi.fn().mockResolvedValue("/my/project"),
    });
    await executeAddDir({ agent: "claude", sessionAlive: true, args: "" }, deps);

    expect(deps.sendMessage).toHaveBeenCalledWith('/add-dir "/my/project"');
    expect(deps.updateAgentSettings).not.toHaveBeenCalled();
  });

  it("session alive + path with newline shows error, does not send", async () => {
    const deps = makeDeps({
      openDirectoryDialog: vi.fn().mockResolvedValue("/path/with\nnewline"),
    });
    await executeAddDir({ agent: "claude", sessionAlive: true, args: "" }, deps);

    expect(deps.sendMessage).not.toHaveBeenCalled();
    expect(deps.appendOutput).toHaveBeenCalledOnce();
    const call = (deps.appendOutput as ReturnType<typeof vi.fn>).mock.calls[0][0] as string;
    expect(call).toContain("chat_addDirFailed");
  });

  it("pre-session + new path saves to agent settings", async () => {
    const deps = makeDeps({
      openDirectoryDialog: vi.fn().mockResolvedValue("/new/dir"),
      getAgentSettings: vi.fn().mockResolvedValue({ add_dirs: ["/existing"] }),
    });
    await executeAddDir({ agent: "claude", sessionAlive: false, args: "" }, deps);

    expect(deps.updateAgentSettings).toHaveBeenCalledWith("claude", {
      add_dirs: ["/existing", "/new/dir"],
    });
    const call = (deps.appendOutput as ReturnType<typeof vi.fn>).mock.calls[0][0] as string;
    expect(call).toContain("chat_addDirSaved");
  });

  it("pre-session + duplicate path shows duplicate message", async () => {
    const deps = makeDeps({
      openDirectoryDialog: vi.fn().mockResolvedValue("/existing"),
      getAgentSettings: vi.fn().mockResolvedValue({ add_dirs: ["/existing"] }),
    });
    await executeAddDir({ agent: "claude", sessionAlive: false, args: "" }, deps);

    expect(deps.updateAgentSettings).not.toHaveBeenCalled();
    const call = (deps.appendOutput as ReturnType<typeof vi.fn>).mock.calls[0][0] as string;
    expect(call).toContain("chat_addDirDuplicate");
  });

  it("pre-session + trailing slash normalization detects duplicate", async () => {
    const deps = makeDeps({
      openDirectoryDialog: vi.fn().mockResolvedValue("/path/to/dir/"),
      getAgentSettings: vi.fn().mockResolvedValue({ add_dirs: ["/path/to/dir"] }),
    });
    await executeAddDir({ agent: "claude", sessionAlive: false, args: "" }, deps);

    expect(deps.updateAgentSettings).not.toHaveBeenCalled();
    const call = (deps.appendOutput as ReturnType<typeof vi.fn>).mock.calls[0][0] as string;
    expect(call).toContain("chat_addDirDuplicate");
  });

  it("pre-session + Windows case-insensitive detects duplicate", async () => {
    const deps = makeDeps({
      openDirectoryDialog: vi.fn().mockResolvedValue("C:\\Foo"),
      getAgentSettings: vi.fn().mockResolvedValue({ add_dirs: ["c:\\foo"] }),
    });
    await executeAddDir({ agent: "claude", sessionAlive: false, args: "" }, deps);

    expect(deps.updateAgentSettings).not.toHaveBeenCalled();
    const call = (deps.appendOutput as ReturnType<typeof vi.fn>).mock.calls[0][0] as string;
    expect(call).toContain("chat_addDirDuplicate");
  });

  it("pre-session + empty add_dirs saves first directory", async () => {
    const deps = makeDeps({
      openDirectoryDialog: vi.fn().mockResolvedValue("/first/dir"),
      getAgentSettings: vi.fn().mockResolvedValue({}),
    });
    await executeAddDir({ agent: "claude", sessionAlive: false, args: "" }, deps);

    expect(deps.updateAgentSettings).toHaveBeenCalledWith("claude", {
      add_dirs: ["/first/dir"],
    });
  });

  // ── args-provided path (skips dialog) ──

  it("session alive + args provided sends path directly, skips dialog", async () => {
    const deps = makeDeps();
    await executeAddDir({ agent: "claude", sessionAlive: true, args: "/my/project" }, deps);

    expect(deps.openDirectoryDialog).not.toHaveBeenCalled();
    expect(deps.sendMessage).toHaveBeenCalledWith('/add-dir "/my/project"');
  });

  it("pre-session + args provided saves to settings, skips dialog", async () => {
    const deps = makeDeps({
      getAgentSettings: vi.fn().mockResolvedValue({ add_dirs: [] }),
    });
    await executeAddDir({ agent: "claude", sessionAlive: false, args: "/from/args" }, deps);

    expect(deps.openDirectoryDialog).not.toHaveBeenCalled();
    expect(deps.updateAgentSettings).toHaveBeenCalledWith("claude", {
      add_dirs: ["/from/args"],
    });
  });

  it("args with newline in session alive shows error", async () => {
    const deps = makeDeps();
    await executeAddDir({ agent: "claude", sessionAlive: true, args: "/bad\npath" }, deps);

    expect(deps.openDirectoryDialog).not.toHaveBeenCalled();
    expect(deps.sendMessage).not.toHaveBeenCalled();
    const call = (deps.appendOutput as ReturnType<typeof vi.fn>).mock.calls[0][0] as string;
    expect(call).toContain("chat_addDirFailed");
  });
});
