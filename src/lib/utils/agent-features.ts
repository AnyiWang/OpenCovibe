/** Per-agent UI feature flags. Pure display logic — no protocol/transport/CLI claims. */
export interface AgentFeatures {
  effortSelector: boolean;
  planModeToggle: boolean;
  permissionModeSwitch: boolean;
  slashCommandMenu: boolean;
  addDirAction: boolean;
}

const CLAUDE_FEATURES: AgentFeatures = {
  effortSelector: true,
  planModeToggle: true,
  permissionModeSwitch: true,
  slashCommandMenu: true,
  addDirAction: true,
};

const CODEX_FEATURES: AgentFeatures = {
  // Codex reads reasoning effort from the selected model's supportedReasoningEfforts.
  // Over the app-server transport the model/effort/permission overrides apply live on
  // the next turn (control protocol: set_effort / set_model / set_permission_mode);
  // they also persist to agent settings for future spawns.
  effortSelector: true,
  planModeToggle: true,
  permissionModeSwitch: true,
  slashCommandMenu: true,
  addDirAction: true,
};

const MINIMAL_FEATURES: AgentFeatures = {
  effortSelector: false,
  planModeToggle: false,
  permissionModeSwitch: false,
  slashCommandMenu: true,
  addDirAction: false,
};

const FEATURES_MAP: Record<string, AgentFeatures> = {
  claude: CLAUDE_FEATURES,
  codex: CODEX_FEATURES,
};

/** Get UI feature flags for a given agent. Unknown agents get minimal features. */
export function getAgentFeatures(agent: string): AgentFeatures {
  return FEATURES_MAP[agent] ?? MINIMAL_FEATURES;
}

/** Check if an agent is registered in the features map. */
export function isKnownAgent(agent: string): boolean {
  return agent in FEATURES_MAP;
}
