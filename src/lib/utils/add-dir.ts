import { quoteCliArg, normalizeDirPath, pathsEqual } from "./path-utils";
import { dbg } from "./debug";
import { getAgentFeatures } from "./agent-features";
import { getAgentCaps } from "./agent-caps";

export interface AddDirDeps {
  openDirectoryDialog: (title: string) => Promise<string | null>;
  sendMessage: (text: string) => Promise<void>;
  getAgentSettings: (agent: string) => Promise<{ add_dirs?: string[] }>;
  updateAgentSettings: (agent: string, patch: { add_dirs: string[] }) => Promise<unknown>;
  appendOutput: (text: string) => void;
  t: (key: string, params?: Record<string, string>) => string;
}

export interface AddDirContext {
  agent: string;
  sessionAlive: boolean;
  args: string;
}

export async function executeAddDir(ctx: AddDirContext, deps: AddDirDeps): Promise<void> {
  if (!getAgentFeatures(ctx.agent).addDirAction) {
    deps.appendOutput(deps.t("chat_addDirUnsupported"));
    return;
  }

  // If args provided, use directly; otherwise open directory picker
  let raw: string | null;
  if (ctx.args) {
    raw = ctx.args;
  } else {
    raw = await deps.openDirectoryDialog(deps.t("chat_addDirTitle"));
    if (typeof raw !== "string" || !raw) return;
  }

  const dirPath = normalizeDirPath(raw);

  // Agents whose CLI honors add-dir live (Claude) push it to the running session;
  // others (Codex) read writable roots only at thread/start, so we persist to
  // settings and the dir is picked up on the next spawn / new thread.
  const liveAddDir = getAgentCaps(ctx.agent).supportsLiveAddDir;

  if (ctx.sessionAlive && liveAddDir) {
    // Live: send /add-dir to CLI (instant effect)
    const quoted = quoteCliArg(dirPath);
    if (!quoted) {
      deps.appendOutput(deps.t("chat_addDirFailed", { error: deps.t("chat_addDirInvalidPath") }));
      return;
    }
    await deps.sendMessage(`/add-dir ${quoted}`);
    dbg("chat", "add-dir: sent to CLI", { path: dirPath });
  } else {
    // No live support, or no active session: persist to agent settings
    const settings = await deps.getAgentSettings(ctx.agent);
    const current = (settings.add_dirs ?? []).map(normalizeDirPath);
    if (!current.some((c) => pathsEqual(c, dirPath))) {
      await deps.updateAgentSettings(ctx.agent, {
        add_dirs: [...(settings.add_dirs ?? []), dirPath],
      });
      // A running session that can't apply add-dir live consumes its writable roots
      // once at spawn (first turn/start), so a mid-session add only lands on the next
      // session / new thread — not the current thread's next turn. Pre-session it's a
      // plain save (picked up when the session next starts).
      deps.appendOutput(
        ctx.sessionAlive
          ? deps.t("chat_addDirNextSession", { path: dirPath })
          : deps.t("chat_addDirSaved", { path: dirPath }),
      );
      dbg("chat", "add-dir: saved to settings", { path: dirPath, agent: ctx.agent });
    } else {
      deps.appendOutput(deps.t("chat_addDirDuplicate", { path: dirPath }));
    }
  }
}
