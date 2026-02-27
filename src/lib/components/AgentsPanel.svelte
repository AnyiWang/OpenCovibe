<script lang="ts">
  import { listAgents, readAgentFile, deleteAgentFile, createAgentFile } from "$lib/api";
  import { t } from "$lib/i18n/index.svelte";
  import { dbg, dbgWarn } from "$lib/utils/debug";
  import type { AgentDefinitionSummary } from "$lib/types";
  import AgentEditor from "./AgentEditor.svelte";

  let {
    projectCwd = "",
    showToast,
  }: {
    projectCwd: string;
    showToast: (message: string, type: "success" | "error") => void;
  } = $props();

  // ── Built-in agents (hardcoded, read-only) ──
  const builtInAgents: AgentDefinitionSummary[] = [
    {
      file_name: "Explore",
      name: "Explore",
      description:
        "Fast agent specialized for exploring codebases. Use for finding files, searching code, or answering questions about the codebase.",
      model: "haiku",
      source: "built-in",
      scope: "user",
      tools: ["Read", "Grep", "Glob", "WebFetch", "WebSearch"],
      readonly: true,
    },
    {
      file_name: "Plan",
      name: "Plan",
      description:
        "Software architect agent for designing implementation plans. Returns step-by-step plans and identifies critical files.",
      model: undefined,
      source: "built-in",
      scope: "user",
      tools: ["Read", "Grep", "Glob", "WebFetch", "WebSearch"],
      readonly: true,
    },
    {
      file_name: "general-purpose",
      name: "general-purpose",
      description:
        "General-purpose agent for researching complex questions, searching for code, and executing multi-step tasks.",
      model: undefined,
      source: "built-in",
      scope: "user",
      readonly: true,
    },
    {
      file_name: "claude-code-guide",
      name: "claude-code-guide",
      description:
        "Agent for answering questions about Claude Code features, hooks, slash commands, MCP servers, settings, and IDE integrations.",
      model: "haiku",
      source: "built-in",
      scope: "user",
      tools: ["Glob", "Grep", "Read", "WebFetch", "WebSearch"],
      readonly: true,
    },
    {
      file_name: "statusline-setup",
      name: "statusline-setup",
      description: "Agent to configure the user's Claude Code status line setting.",
      model: "sonnet",
      source: "built-in",
      scope: "user",
      tools: ["Read", "Edit"],
      readonly: true,
    },
  ];

  // ── State ──
  let loading = $state(true);
  let customAgents = $state<AgentDefinitionSummary[]>([]);
  let pluginAgents = $state<AgentDefinitionSummary[]>([]);
  let pluginError = $state(false);
  let selectedAgent = $state<AgentDefinitionSummary | null>(null);
  let selectedContent = $state<string | null>(null);
  let activeGroup = $state<"built-in" | "custom" | "plugin">("built-in");
  let confirmDelete = $state<AgentDefinitionSummary | null>(null);
  let editorState = $state<{
    mode: "create" | "edit";
    agent: AgentDefinitionSummary | null;
  } | null>(null);
  let renameState = $state<{
    agent: AgentDefinitionSummary;
    newName: string;
    error: string;
  } | null>(null);

  // ── Data loading ──
  async function loadAgents() {
    loading = true;
    pluginError = false;
    try {
      const all = await listAgents(projectCwd || undefined);
      dbg("agents-panel", "loaded agents", { count: all.length });
      customAgents = all.filter((a) => a.scope === "user" || a.scope === "project");
      const plugins = all.filter((a) => a.scope === "plugin");
      pluginAgents = plugins;
      pluginError = false;
    } catch (e) {
      dbgWarn("agents-panel", "failed to load agents", e);
      customAgents = [];
      pluginAgents = [];
      pluginError = true;
    } finally {
      loading = false;
    }
  }

  // Auto-load when component becomes visible
  $effect(() => {
    void loadAgents();
  });

  // ── Computed ──
  let existingAgentNames = $derived(customAgents.map((a) => a.name));

  let displayedAgents = $derived.by(() => {
    if (activeGroup === "built-in") return builtInAgents;
    if (activeGroup === "custom") return customAgents;
    return pluginAgents;
  });

  // ── Actions ──
  async function selectAgent(agent: AgentDefinitionSummary) {
    selectedAgent = agent;
    selectedContent = null;

    if (agent.raw_content != null) {
      selectedContent = agent.raw_content;
      return;
    }
    if (agent.source === "built-in") {
      return;
    }
    // Load raw content for custom agents
    if (agent.scope === "user" || agent.scope === "project") {
      try {
        selectedContent = await readAgentFile(
          agent.scope as "user" | "project",
          agent.file_name,
          projectCwd || undefined,
        );
      } catch (e) {
        dbgWarn("agents-panel", "failed to read agent file", e);
      }
    }
  }

  async function handleDelete(agent: AgentDefinitionSummary) {
    try {
      await deleteAgentFile(
        agent.scope as "user" | "project",
        agent.file_name,
        projectCwd || undefined,
      );
      showToast(t("agent_deleted", { name: agent.name }), "success");
      selectedAgent = null;
      selectedContent = null;
      confirmDelete = null;
      await loadAgents();
    } catch (e) {
      showToast(t("agent_deleteFailed", { error: String(e) }), "error");
      confirmDelete = null;
    }
  }

  async function handleRename(agent: AgentDefinitionSummary, newFileName: string) {
    if (!renameState) return;
    try {
      // Read current content
      const content = await readAgentFile(
        agent.scope as "user" | "project",
        agent.file_name,
        projectCwd || undefined,
      );
      // Create new file (must not exist)
      await createAgentFile(
        agent.scope as "user" | "project",
        newFileName,
        content,
        projectCwd || undefined,
      );
      // Delete old file
      await deleteAgentFile(
        agent.scope as "user" | "project",
        agent.file_name,
        projectCwd || undefined,
      );
      showToast(t("agent_saved"), "success");
      renameState = null;
      selectedAgent = null;
      selectedContent = null;
      await loadAgents();
    } catch (e) {
      renameState = { ...renameState!, error: String(e) };
    }
  }

  function scopeLabel(agent: AgentDefinitionSummary): string {
    if (agent.source === "built-in") return t("agent_builtIn");
    if (agent.scope === "user") return t("agent_scopeUser");
    if (agent.scope === "project") return t("agent_scopeProject");
    if (agent.scope === "plugin") {
      // source = "plugin:marketplace:plugin_name"
      const parts = agent.source.split(":");
      return parts.length >= 3 ? parts[2] : t("agent_sourcePlugin");
    }
    return agent.scope;
  }

  function scopeColor(agent: AgentDefinitionSummary): string {
    if (agent.source === "built-in") return "bg-muted text-muted-foreground";
    if (agent.scope === "user") return "bg-blue-500/10 text-blue-600 dark:text-blue-400";
    if (agent.scope === "project") return "bg-green-500/10 text-green-600 dark:text-green-400";
    if (agent.scope === "plugin") return "bg-purple-500/10 text-purple-600 dark:text-purple-400";
    return "bg-muted text-muted-foreground";
  }

  function modelLabel(model: string | undefined): string {
    if (!model) return t("agent_inherit");
    return model;
  }

  function toolsSummary(agent: AgentDefinitionSummary): string {
    if (agent.tools && agent.tools.length > 0) {
      return agent.tools.join(", ");
    }
    return t("agent_allTools");
  }
</script>

<!-- Delete Confirmation -->
{#if confirmDelete}
  <div
    class="fixed inset-0 z-50 flex items-center justify-center bg-black/50"
    role="dialog"
    aria-modal="true"
    onclick={() => (confirmDelete = null)}
    onkeydown={(e) => e.key === "Escape" && (confirmDelete = null)}
    tabindex="-1"
  >
    <div
      class="rounded-lg border border-border bg-background p-6 shadow-xl max-w-sm"
      onclick={(e) => e.stopPropagation()}
      onkeydown={() => {}}
      role="document"
      tabindex="-1"
    >
      <h3 class="text-sm font-semibold text-foreground mb-2">{t("agent_deleteAgent")}</h3>
      <p class="text-xs text-muted-foreground mb-4">
        {t("agent_deleteConfirm", { name: confirmDelete.name })}
      </p>
      <div class="flex justify-end gap-2">
        <button
          class="rounded-md border border-border px-3 py-1.5 text-xs text-muted-foreground hover:text-foreground"
          onclick={() => (confirmDelete = null)}
        >
          Cancel
        </button>
        <button
          class="rounded-md bg-destructive px-3 py-1.5 text-xs text-destructive-foreground hover:bg-destructive/90"
          onclick={() => confirmDelete && handleDelete(confirmDelete)}
        >
          {t("agent_deleteAgent")}
        </button>
      </div>
    </div>
  </div>
{/if}

<div>
  <!-- Header -->
  <div class="flex items-center justify-between mb-3">
    <div>
      <h2 class="text-sm font-semibold text-foreground">{t("agent_title")}</h2>
      <p class="text-[11px] text-muted-foreground">{t("agent_desc")}</p>
    </div>
    <button
      class="rounded-md bg-primary px-3 py-1.5 text-xs text-primary-foreground hover:bg-primary/90"
      onclick={() => {
        editorState = { mode: "create", agent: null };
      }}
    >
      + {t("agent_createAgent")}
    </button>
  </div>

  <!-- Group tabs -->
  <div class="flex gap-1 mb-4">
    {#each [{ id: "built-in" as const, label: t("agent_builtIn"), count: builtInAgents.length }, { id: "custom" as const, label: t("agent_custom"), count: customAgents.length }, { id: "plugin" as const, label: t("agent_plugin"), count: pluginAgents.length }] as tab}
      <button
        class="px-3 py-1 text-xs rounded-md transition-colors
          {activeGroup === tab.id
          ? 'bg-primary text-primary-foreground'
          : 'bg-muted text-muted-foreground hover:text-foreground'}"
        onclick={() => {
          activeGroup = tab.id;
          selectedAgent = null;
          selectedContent = null;
        }}
      >
        {tab.label}
        <span class="ml-1 opacity-60">{tab.count}</span>
      </button>
    {/each}
  </div>

  {#if loading}
    <div class="flex items-center justify-center py-12">
      <div
        class="h-5 w-5 border-2 border-primary/30 border-t-primary rounded-full animate-spin"
      ></div>
    </div>
  {:else}
    <div class="flex gap-4" style="min-height: 400px;">
      <!-- Left: Agent list -->
      <div class="w-1/2 space-y-1 overflow-y-auto pr-2" style="max-height: 600px;">
        {#if displayedAgents.length === 0}
          <div class="flex flex-col items-center justify-center py-12 text-center">
            {#if activeGroup === "custom"}
              <p class="text-xs text-muted-foreground font-medium">{t("agent_noCustomAgents")}</p>
              <p class="text-[11px] text-muted-foreground mt-1">
                {t("agent_noCustomAgentsDesc")}
              </p>
            {:else if activeGroup === "plugin"}
              {#if pluginError}
                <p class="text-xs text-muted-foreground">{t("agent_pluginUnavailable")}</p>
              {:else}
                <p class="text-xs text-muted-foreground">{t("agent_noPluginAgents")}</p>
                <p class="text-[11px] text-muted-foreground mt-1">
                  {t("agent_noPluginAgentsDesc")}
                </p>
              {/if}
            {/if}
          </div>
        {:else}
          {#each displayedAgents as agent}
            <button
              class="w-full text-left rounded-md border px-3 py-2.5 transition-colors
                {selectedAgent?.file_name === agent.file_name &&
              selectedAgent?.source === agent.source
                ? 'border-primary bg-primary/5'
                : 'border-transparent hover:bg-muted/50'}"
              onclick={() => selectAgent(agent)}
            >
              <div class="flex items-center gap-2 mb-0.5">
                <span class="text-xs font-medium text-foreground truncate">{agent.name}</span>
                {#if agent.name !== agent.file_name && agent.source !== "built-in"}
                  <span class="text-[10px] text-muted-foreground truncate">
                    {t("agent_fileNameHint", { fileName: agent.file_name })}
                  </span>
                {/if}
              </div>
              <div class="flex items-center gap-1.5 mt-1">
                <span
                  class="inline-flex items-center rounded px-1.5 py-0.5 text-[10px] font-medium {scopeColor(
                    agent,
                  )}"
                >
                  {scopeLabel(agent)}
                </span>
                {#if agent.model}
                  <span
                    class="inline-flex items-center rounded bg-muted px-1.5 py-0.5 text-[10px] text-muted-foreground"
                  >
                    {agent.model}
                  </span>
                {/if}
                {#if agent.tools}
                  <span class="text-[10px] text-muted-foreground">
                    {agent.tools.length} tools
                  </span>
                {/if}
              </div>
              {#if agent.description}
                <p class="text-[11px] text-muted-foreground mt-1 line-clamp-1">
                  {agent.description}
                </p>
              {/if}
            </button>
          {/each}
        {/if}
      </div>

      <!-- Right: Agent detail -->
      <div class="w-1/2 border-l border-border pl-4 overflow-y-auto" style="max-height: 600px;">
        {#if selectedAgent}
          <div class="space-y-3">
            <!-- Name + badges -->
            <div>
              <h3 class="text-sm font-semibold text-foreground">{selectedAgent.name}</h3>
              {#if selectedAgent.name !== selectedAgent.file_name && selectedAgent.source !== "built-in"}
                <p class="text-[10px] text-muted-foreground">
                  {t("agent_fileNameHint", { fileName: selectedAgent.file_name })}
                </p>
              {/if}
              <div class="flex items-center gap-1.5 mt-1.5">
                <span
                  class="inline-flex items-center rounded px-1.5 py-0.5 text-[10px] font-medium {scopeColor(
                    selectedAgent,
                  )}"
                >
                  {scopeLabel(selectedAgent)}
                </span>
                {#if selectedAgent.readonly}
                  <span
                    class="inline-flex items-center rounded bg-muted px-1.5 py-0.5 text-[10px] text-muted-foreground"
                  >
                    {t("agent_readonlyBadge")}
                  </span>
                {/if}
              </div>
            </div>

            <!-- Description -->
            {#if selectedAgent.description}
              <div>
                <p
                  class="text-[10px] font-medium text-muted-foreground uppercase tracking-wider mb-0.5"
                >
                  {t("agent_description")}
                </p>
                <p class="text-xs text-foreground">{selectedAgent.description}</p>
              </div>
            {/if}

            <!-- Model -->
            <div>
              <p
                class="text-[10px] font-medium text-muted-foreground uppercase tracking-wider mb-0.5"
              >
                {t("agent_model")}
              </p>
              <p class="text-xs text-foreground">{modelLabel(selectedAgent.model)}</p>
            </div>

            <!-- Tools -->
            <div>
              <p
                class="text-[10px] font-medium text-muted-foreground uppercase tracking-wider mb-0.5"
              >
                {t("agent_tools")}
              </p>
              <p class="text-xs text-foreground">{toolsSummary(selectedAgent)}</p>
            </div>

            <!-- Disallowed Tools -->
            {#if selectedAgent.disallowed_tools && selectedAgent.disallowed_tools.length > 0}
              <div>
                <p
                  class="text-[10px] font-medium text-muted-foreground uppercase tracking-wider mb-0.5"
                >
                  {t("agent_disallowedTools")}
                </p>
                <p class="text-xs text-foreground">{selectedAgent.disallowed_tools.join(", ")}</p>
              </div>
            {/if}

            <!-- Permission Mode -->
            {#if selectedAgent.permission_mode}
              <div>
                <p
                  class="text-[10px] font-medium text-muted-foreground uppercase tracking-wider mb-0.5"
                >
                  {t("agent_permissionMode")}
                </p>
                <p class="text-xs text-foreground">{selectedAgent.permission_mode}</p>
              </div>
            {/if}

            <!-- Max Turns -->
            {#if selectedAgent.max_turns}
              <div>
                <p
                  class="text-[10px] font-medium text-muted-foreground uppercase tracking-wider mb-0.5"
                >
                  {t("agent_maxTurns")}
                </p>
                <p class="text-xs text-foreground">{selectedAgent.max_turns}</p>
              </div>
            {/if}

            <!-- Background -->
            {#if selectedAgent.background}
              <div>
                <p
                  class="text-[10px] font-medium text-muted-foreground uppercase tracking-wider mb-0.5"
                >
                  {t("agent_background")}
                </p>
                <p class="text-xs text-foreground">Yes</p>
              </div>
            {/if}

            <!-- Isolation -->
            {#if selectedAgent.isolation}
              <div>
                <p
                  class="text-[10px] font-medium text-muted-foreground uppercase tracking-wider mb-0.5"
                >
                  {t("agent_isolation")}
                </p>
                <p class="text-xs text-foreground">{selectedAgent.isolation}</p>
              </div>
            {/if}

            <!-- System Prompt preview -->
            {#if selectedContent}
              <div>
                <p
                  class="text-[10px] font-medium text-muted-foreground uppercase tracking-wider mb-0.5"
                >
                  {t("agent_systemPrompt")}
                </p>
                <pre
                  class="text-[11px] text-foreground bg-muted/50 rounded-md p-3 overflow-auto max-h-64 whitespace-pre-wrap font-mono">{selectedContent}</pre>
              </div>
            {/if}

            <!-- Actions (only for custom agents) -->
            {#if !selectedAgent.readonly && (selectedAgent.scope === "user" || selectedAgent.scope === "project")}
              <div class="flex gap-2 pt-2 border-t border-border">
                <button
                  class="rounded-md bg-primary/10 px-3 py-1.5 text-xs text-primary hover:bg-primary/20 transition-colors"
                  onclick={() => {
                    editorState = { mode: "edit", agent: selectedAgent };
                  }}
                >
                  {t("agent_editAgent")}
                </button>
                <button
                  class="rounded-md bg-muted px-3 py-1.5 text-xs text-foreground hover:bg-muted/80 transition-colors"
                  onclick={() => {
                    if (selectedAgent) {
                      renameState = {
                        agent: selectedAgent,
                        newName: selectedAgent.file_name,
                        error: "",
                      };
                    }
                  }}
                >
                  {t("agent_renameAgent")}
                </button>
                <button
                  class="rounded-md bg-destructive/10 px-3 py-1.5 text-xs text-destructive hover:bg-destructive/20 transition-colors"
                  onclick={() => (confirmDelete = selectedAgent)}
                >
                  {t("agent_deleteAgent")}
                </button>
              </div>
              <p class="text-[10px] text-muted-foreground italic">
                {t("agent_changesNextSession")}
              </p>
            {/if}
          </div>
        {:else}
          <div class="flex items-center justify-center h-full text-muted-foreground text-xs">
            Select an agent to view details
          </div>
        {/if}
      </div>
    </div>
  {/if}
</div>

<!-- Rename Dialog -->
{#if renameState}
  <div
    class="fixed inset-0 z-50 flex items-center justify-center bg-black/50"
    role="dialog"
    aria-modal="true"
    onclick={() => (renameState = null)}
    onkeydown={(e) => e.key === "Escape" && (renameState = null)}
    tabindex="-1"
  >
    <div
      class="rounded-lg border border-border bg-background p-6 shadow-xl max-w-sm w-full"
      onclick={(e) => e.stopPropagation()}
      onkeydown={() => {}}
      role="document"
      tabindex="-1"
    >
      <h3 class="text-sm font-semibold text-foreground mb-3">{t("agent_renameTitle")}</h3>
      <input
        type="text"
        class="w-full rounded-md border border-border bg-background px-3 py-1.5 text-xs text-foreground
          focus:outline-none focus:ring-1 focus:ring-primary mb-1"
        bind:value={renameState.newName}
        onkeydown={(e) => {
          if (e.key === "Enter" && renameState) {
            handleRename(renameState.agent, renameState.newName);
          }
        }}
      />
      <p class="text-[10px] text-muted-foreground mb-3">{t("agent_nameFormat")}</p>
      {#if renameState.error}
        <p class="text-xs text-destructive mb-3">{renameState.error}</p>
      {/if}
      <div class="flex justify-end gap-2">
        <button
          class="rounded-md border border-border px-3 py-1.5 text-xs text-muted-foreground hover:text-foreground"
          onclick={() => (renameState = null)}
        >
          Cancel
        </button>
        <button
          class="rounded-md bg-primary px-3 py-1.5 text-xs text-primary-foreground hover:bg-primary/90
            disabled:opacity-50"
          disabled={!renameState.newName || renameState.newName === renameState.agent.file_name}
          onclick={() => renameState && handleRename(renameState.agent, renameState.newName)}
        >
          {t("agent_renameConfirm")}
        </button>
      </div>
    </div>
  </div>
{/if}

<!-- Editor Panel (slide-over) -->
{#if editorState}
  <div
    class="fixed inset-0 z-40 flex justify-end bg-black/30"
    onclick={() => (editorState = null)}
    onkeydown={(e) => e.key === "Escape" && (editorState = null)}
    role="dialog"
    aria-modal="true"
    tabindex="-1"
  >
    <div
      class="w-full max-w-lg bg-background border-l border-border shadow-xl overflow-y-auto p-6"
      onclick={(e) => e.stopPropagation()}
      onkeydown={() => {}}
      role="document"
      tabindex="-1"
    >
      <AgentEditor
        mode={editorState.mode}
        agent={editorState.agent}
        {projectCwd}
        {existingAgentNames}
        onSave={async () => {
          showToast(
            editorState?.mode === "create" ? t("agent_created") : t("agent_saved"),
            "success",
          );
          editorState = null;
          selectedAgent = null;
          selectedContent = null;
          await loadAgents();
        }}
        onCancel={() => (editorState = null)}
      />
    </div>
  </div>
{/if}
