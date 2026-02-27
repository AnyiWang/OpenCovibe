import { quoteCliArg, normalizeDirPath, pathsEqual } from "./slash-commands";
import { dbg } from "./debug";

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
  if (ctx.agent !== "claude") {
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

  if (ctx.sessionAlive) {
    const quoted = quoteCliArg(dirPath);
    if (!quoted) {
      deps.appendOutput(deps.t("chat_addDirFailed", { error: deps.t("chat_addDirInvalidPath") }));
      return;
    }
    await deps.sendMessage(`/add-dir ${quoted}`);
    dbg("chat", "add-dir: sent to CLI", { path: dirPath });
  } else {
    const settings = await deps.getAgentSettings(ctx.agent);
    const current = (settings.add_dirs ?? []).map(normalizeDirPath);
    if (!current.some((c) => pathsEqual(c, dirPath))) {
      await deps.updateAgentSettings(ctx.agent, {
        add_dirs: [...(settings.add_dirs ?? []), dirPath],
      });
      deps.appendOutput(deps.t("chat_addDirSaved", { path: dirPath }));
      dbg("chat", "add-dir: saved to settings", { path: dirPath, agent: ctx.agent });
    } else {
      deps.appendOutput(deps.t("chat_addDirDuplicate", { path: dirPath }));
    }
  }
}
