export type CommandCategory = "chat" | "tools" | "navigation" | "settings" | "diagnostics";
export type CommandAgent = "claude" | "codex" | "both";
export type CommandAction =
  | "send_prompt"
  | "navigate"
  | "ipc_command"
  | "toggle_state"
  | "open_modal";

export interface CommandDef {
  id: string;
  name: string;
  description: string;
  category: CommandCategory;
  agent: CommandAgent;
  shortcut?: string;
  action: CommandAction;
  payload?: string;
}

export const commands: CommandDef[] = [
  // Chat
  {
    id: "switch-model",
    name: "Switch Model",
    description: "Change the AI model for the next message",
    category: "chat",
    agent: "both",
    action: "open_modal",
    payload: "model-selector",
  },
  {
    id: "compact",
    name: "Compact Conversation",
    description: "Compress the conversation to free up context",
    category: "chat",
    agent: "claude",
    action: "send_prompt",
    payload: "/compact",
  },
  {
    id: "toggle-plan",
    name: "Toggle Plan Mode",
    description: "Switch between plan mode (read-only) and normal mode",
    category: "chat",
    agent: "claude",
    action: "toggle_state",
    payload: "plan_mode",
  },
  {
    id: "review",
    name: "Review Changes",
    description: "Ask the agent to review recent code changes",
    category: "chat",
    agent: "claude",
    action: "send_prompt",
    payload:
      "Review my recent changes. Look at the git diff and provide feedback on code quality, potential bugs, and improvements.",
  },
  {
    id: "export-chat",
    name: "Export Chat",
    description: "Export the conversation as a Markdown file",
    category: "chat",
    agent: "both",
    shortcut: "Cmd+Shift+E",
    action: "ipc_command",
    payload: "export_conversation",
  },
  {
    id: "new-claude",
    name: "New Claude Chat",
    description: "Start a new Claude Code conversation",
    category: "chat",
    agent: "both",
    action: "navigate",
    payload: "/chat?agent=claude",
  },
  // Codex disabled
  // {
  //   id: "new-codex",
  //   name: "New Codex Chat",
  //   description: "Start a new Codex conversation",
  //   category: "chat",
  //   agent: "both",
  //   action: "navigate",
  //   payload: "/chat?agent=codex",
  // },
  {
    id: "stop-run",
    name: "Stop Run",
    description: "Stop the currently running agent process",
    category: "chat",
    agent: "both",
    action: "ipc_command",
    payload: "stop_run",
  },

  // Tools
  {
    id: "git-diff",
    name: "Git Diff",
    description: "View current git changes",
    category: "tools",
    agent: "both",
    shortcut: "Cmd+Shift+D",
    action: "ipc_command",
    payload: "get_git_diff",
  },
  {
    id: "git-status",
    name: "Git Status",
    description: "View git status summary",
    category: "tools",
    agent: "both",
    action: "ipc_command",
    payload: "get_git_status",
  },
  {
    id: "token-cost",
    name: "Token Cost",
    description: "View token usage and cost for current run",
    category: "tools",
    agent: "both",
    action: "ipc_command",
    payload: "get_run_artifacts",
  },

  // Navigation
  {
    id: "go-chat",
    name: "Go to Chat",
    description: "Navigate to the chat page",
    category: "navigation",
    agent: "both",
    action: "navigate",
    payload: "/chat",
  },
  {
    id: "go-settings",
    name: "Go to Settings",
    description: "Navigate to settings",
    category: "navigation",
    agent: "both",
    action: "navigate",
    payload: "/settings",
  },
  {
    id: "go-memory",
    name: "Go to Memory",
    description: "Navigate to the memory editor",
    category: "navigation",
    agent: "both",
    action: "navigate",
    payload: "/memory",
  },
  {
    id: "go-usage",
    name: "Go to Usage",
    description: "Navigate to usage statistics",
    category: "navigation",
    agent: "both",
    action: "navigate",
    payload: "/usage",
  },
  // Codex disabled
  // {
  //   id: "go-codex-config",
  //   name: "Go to Codex Config",
  //   description: "Navigate to Codex agent configuration",
  //   category: "navigation",
  //   agent: "both",
  //   action: "navigate",
  //   payload: "/config/codex",
  // },
  {
    id: "go-plugins",
    name: "Go to Plugins",
    description: "Browse plugins and skills",
    category: "navigation",
    agent: "both",
    action: "navigate",
    payload: "/plugins",
  },

  // Settings
  {
    id: "set-model",
    name: "Set Default Model",
    description: "Change the default model for the agent",
    category: "settings",
    agent: "both",
    action: "open_modal",
    payload: "model-selector",
  },
  {
    id: "set-cwd",
    name: "Set Working Directory",
    description: "Change the project working directory",
    category: "settings",
    agent: "both",
    action: "open_modal",
    payload: "folder-browser",
  },
  {
    id: "configure-tools",
    name: "Configure Tools",
    description: "Set allowed tools for the agent",
    category: "settings",
    agent: "both",
    action: "navigate",
    payload: "/settings",
  },

  // Diagnostics
  {
    id: "doctor",
    name: "Run Doctor",
    description: "Check if agent CLIs are installed and working",
    category: "diagnostics",
    agent: "both",
    action: "ipc_command",
    payload: "check_agent_cli",
  },
  {
    id: "version",
    name: "Version Info",
    description: "Show OpenCovibe Desktop version information",
    category: "diagnostics",
    agent: "both",
    action: "open_modal",
    payload: "version-info",
  },
];

export function filterCommands(query: string, agent?: string): CommandDef[] {
  const q = query.toLowerCase();
  return commands.filter((cmd) => {
    if (agent && cmd.agent !== "both" && cmd.agent !== agent) return false;
    if (!q) return true;
    return (
      cmd.name.toLowerCase().includes(q) ||
      cmd.description.toLowerCase().includes(q) ||
      cmd.id.includes(q)
    );
  });
}

export function groupByCategory(cmds: CommandDef[]): Record<CommandCategory, CommandDef[]> {
  const groups: Record<CommandCategory, CommandDef[]> = {
    chat: [],
    tools: [],
    navigation: [],
    settings: [],
    diagnostics: [],
  };
  for (const cmd of cmds) {
    groups[cmd.category].push(cmd);
  }
  return groups;
}

export const categoryLabels: Record<CommandCategory, string> = {
  chat: "Chat",
  tools: "Tools",
  navigation: "Navigation",
  settings: "Settings",
  diagnostics: "Diagnostics",
};
