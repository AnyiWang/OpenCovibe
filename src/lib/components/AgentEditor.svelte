<script lang="ts">
  import { t } from "$lib/i18n/index.svelte";
  import { dbg, dbgWarn } from "$lib/utils/debug";
  import { createAgentFile, updateAgentFile, readAgentFile } from "$lib/api";
  import {
    serializeAgentFile,
    parseAgentFile,
    validateAgentForm,
    validateSourceContent,
    extractFrontmatterName,
    defaultFormData,
    type AgentFormData,
  } from "$lib/utils/agent-editor";
  import type { AgentDefinitionSummary } from "$lib/types";

  let {
    mode,
    agent = null,
    projectCwd = "",
    existingAgentNames = [],
    onSave,
    onCancel,
  }: {
    mode: "create" | "edit";
    agent?: AgentDefinitionSummary | null;
    projectCwd: string;
    existingAgentNames: string[];
    onSave: () => void;
    onCancel: () => void;
  } = $props();

  // ── State ──
  let editorMode = $state<"form" | "source">(mode === "create" ? "form" : "source");
  let formData = $state<AgentFormData>(defaultFormData());
  let sourceContent = $state("");
  let scope = $state<"user" | "project">("user");
  let saving = $state(false);
  let errors = $state<string[]>([]);
  let toolInput = $state("");

  // ── Init ──
  $effect(() => {
    if (mode === "edit" && agent) {
      scope = agent.scope as "user" | "project";
      loadAgentContent();
    } else {
      formData = defaultFormData();
      sourceContent = "";
      scope = "user";
    }
  });

  async function loadAgentContent() {
    if (!agent) return;
    try {
      const content = await readAgentFile(
        agent.scope as "user" | "project",
        agent.file_name,
        projectCwd || undefined,
      );
      sourceContent = content;
      formData = parseAgentFile(content);
      dbg("agent-editor", "loaded content", { fileName: agent.file_name });
    } catch (e) {
      dbgWarn("agent-editor", "failed to load", e);
      errors = [`Failed to load agent: ${e}`];
    }
  }

  // ── Save ──
  async function handleSave() {
    saving = true;
    errors = [];

    try {
      if (mode === "create") {
        if (editorMode === "form") {
          // Form mode: validate form data + serialize
          const validationErrors = validateAgentForm(formData);
          if (validationErrors.length > 0) {
            errors = validationErrors.map((e) => `${e.field}: ${e.message}`);
            saving = false;
            return;
          }
          const content = serializeAgentFile(formData);
          const fileName = formData.name;
          await createAgentFile(scope, fileName, content, projectCwd || undefined);
          dbg("agent-editor", "created (form)", { fileName, scope });
        } else {
          // Source mode: validate source content + extract name from frontmatter
          const validation = validateSourceContent(sourceContent, existingAgentNames);
          if (!validation.valid) {
            errors = validation.warnings;
            saving = false;
            return;
          }
          const fileName = extractFrontmatterName(sourceContent) || "new-agent";
          await createAgentFile(scope, fileName, sourceContent, projectCwd || undefined);
          dbg("agent-editor", "created (source)", { fileName, scope });
        }
      } else if (mode === "edit" && agent) {
        // Source mode: light validation
        if (editorMode === "source") {
          const validation = validateSourceContent(
            sourceContent,
            existingAgentNames.filter((n) => n !== agent?.name),
          );
          if (!validation.valid) {
            errors = validation.warnings;
            saving = false;
            return;
          }
        }
        const content = editorMode === "source" ? sourceContent : serializeAgentFile(formData);
        await updateAgentFile(
          agent.scope as "user" | "project",
          agent.file_name,
          content,
          projectCwd || undefined,
        );
        dbg("agent-editor", "updated", { fileName: agent.file_name });
      }
      onSave();
    } catch (e) {
      dbgWarn("agent-editor", "save failed", e);
      errors = [String(e)];
    } finally {
      saving = false;
    }
  }

  function handleForceSave() {
    errors = [];
    // Re-run save without validation
    saving = true;
    const content = editorMode === "source" ? sourceContent : serializeAgentFile(formData);
    const doSave = async () => {
      try {
        if (mode === "create") {
          const fileName = editorMode === "form" ? formData.name : formData.name || "new-agent";
          await createAgentFile(scope, fileName, content, projectCwd || undefined);
        } else if (agent) {
          await updateAgentFile(
            agent.scope as "user" | "project",
            agent.file_name,
            content,
            projectCwd || undefined,
          );
        }
        onSave();
      } catch (e) {
        errors = [String(e)];
      } finally {
        saving = false;
      }
    };
    void doSave();
  }

  function addTool() {
    const tool = toolInput.trim();
    if (tool && !formData.tools.includes(tool)) {
      formData.tools = [...formData.tools, tool];
      toolInput = "";
    }
  }

  function removeTool(tool: string) {
    formData.tools = formData.tools.filter((t) => t !== tool);
  }

  const AVAILABLE_TOOLS = [
    "Read",
    "Write",
    "Edit",
    "Bash",
    "Glob",
    "Grep",
    "WebFetch",
    "WebSearch",
    "Task",
    "NotebookEdit",
  ];
</script>

<div class="space-y-4">
  <!-- Header with mode toggle -->
  <div class="flex items-center justify-between">
    <h3 class="text-sm font-semibold text-foreground">
      {mode === "create" ? t("agent_createAgent") : t("agent_editAgent")}
    </h3>
    <div class="flex gap-1 rounded-md bg-muted p-0.5">
      <button
        class="px-2 py-0.5 text-[11px] rounded transition-colors
          {editorMode === 'form'
          ? 'bg-background text-foreground shadow-sm'
          : 'text-muted-foreground hover:text-foreground'}"
        onclick={() => {
          editorMode = "form";
          if (sourceContent) {
            formData = parseAgentFile(sourceContent);
          }
        }}
      >
        {t("agent_formMode")}
        {#if mode === "edit"}
          <span class="text-[9px] opacity-60">(view)</span>
        {/if}
      </button>
      <button
        class="px-2 py-0.5 text-[11px] rounded transition-colors
          {editorMode === 'source'
          ? 'bg-background text-foreground shadow-sm'
          : 'text-muted-foreground hover:text-foreground'}"
        onclick={() => {
          editorMode = "source";
          if (mode === "create" && !sourceContent) {
            sourceContent = serializeAgentFile(formData);
          }
        }}
      >
        {t("agent_sourceMode")}
      </button>
    </div>
  </div>

  <!-- Errors -->
  {#if errors.length > 0}
    <div class="rounded-md border border-destructive/30 bg-destructive/10 p-3 space-y-1">
      {#each errors as error}
        <p class="text-xs text-destructive">{error}</p>
      {/each}
      {#if mode === "edit" && editorMode === "source"}
        <button
          class="text-xs text-muted-foreground underline hover:text-foreground mt-1"
          onclick={handleForceSave}
        >
          Force save (skip validation)
        </button>
      {/if}
    </div>
  {/if}

  {#if editorMode === "form"}
    <!-- Form Mode -->
    <div class="space-y-3">
      <!-- Name -->
      <div>
        <label class="text-[11px] font-medium text-foreground block mb-1">{t("agent_name")} *</label
        >
        <input
          type="text"
          class="w-full rounded-md border border-border bg-background px-3 py-1.5 text-xs text-foreground
            focus:outline-none focus:ring-1 focus:ring-primary disabled:opacity-50"
          placeholder="code-reviewer"
          bind:value={formData.name}
          disabled={mode === "edit"}
        />
        <p class="text-[10px] text-muted-foreground mt-0.5">{t("agent_nameFormat")}</p>
      </div>

      <!-- Description -->
      <div>
        <label class="text-[11px] font-medium text-foreground block mb-1"
          >{t("agent_description")} *</label
        >
        <textarea
          class="w-full rounded-md border border-border bg-background px-3 py-1.5 text-xs text-foreground
            focus:outline-none focus:ring-1 focus:ring-primary resize-none disabled:opacity-50"
          rows="2"
          placeholder="Expert code reviewer for quality..."
          bind:value={formData.description}
          disabled={mode === "edit"}
        ></textarea>
      </div>

      <!-- Scope (create only) -->
      {#if mode === "create"}
        <div>
          <label class="text-[11px] font-medium text-foreground block mb-1">Scope</label>
          <div class="flex gap-2">
            <button
              class="px-3 py-1 text-xs rounded-md border transition-colors
                {scope === 'user'
                ? 'border-primary bg-primary/10 text-foreground'
                : 'border-border text-muted-foreground hover:text-foreground'}"
              onclick={() => (scope = "user")}
            >
              {t("agent_scopeUser")}
            </button>
            <button
              class="px-3 py-1 text-xs rounded-md border transition-colors
                {scope === 'project'
                ? 'border-primary bg-primary/10 text-foreground'
                : 'border-border text-muted-foreground hover:text-foreground'}"
              onclick={() => (scope = "project")}
            >
              {t("agent_scopeProject")}
            </button>
          </div>
        </div>
      {/if}

      <!-- Model -->
      <div>
        <label class="text-[11px] font-medium text-foreground block mb-1">{t("agent_model")}</label>
        <select
          class="w-full rounded-md border border-border bg-background px-3 py-1.5 text-xs text-foreground
            focus:outline-none focus:ring-1 focus:ring-primary disabled:opacity-50"
          bind:value={formData.model}
          disabled={mode === "edit"}
        >
          <option value="inherit">{t("agent_inherit")}</option>
          <option value="sonnet">sonnet</option>
          <option value="opus">opus</option>
          <option value="haiku">haiku</option>
        </select>
      </div>

      <!-- Tools -->
      <div>
        <label class="text-[11px] font-medium text-foreground block mb-1">{t("agent_tools")}</label>
        {#if formData.tools.length > 0}
          <div class="flex flex-wrap gap-1 mb-1.5">
            {#each formData.tools as tool}
              <span
                class="inline-flex items-center gap-1 rounded bg-muted px-1.5 py-0.5 text-[10px] text-foreground"
              >
                {tool}
                {#if mode === "create"}
                  <button
                    class="text-muted-foreground hover:text-destructive"
                    onclick={() => removeTool(tool)}>×</button
                  >
                {/if}
              </span>
            {/each}
          </div>
        {:else}
          <p class="text-[10px] text-muted-foreground mb-1.5">{t("agent_allTools")}</p>
        {/if}
        {#if mode === "create"}
          <div class="flex gap-1">
            <select
              class="flex-1 rounded-md border border-border bg-background px-2 py-1 text-xs"
              bind:value={toolInput}
            >
              <option value="">Add tool...</option>
              {#each AVAILABLE_TOOLS.filter((t) => !formData.tools.includes(t)) as tool}
                <option value={tool}>{tool}</option>
              {/each}
            </select>
            <button
              class="rounded-md bg-muted px-2 py-1 text-xs text-foreground hover:bg-muted/80"
              onclick={addTool}
              disabled={!toolInput}>+</button
            >
          </div>
        {/if}
      </div>

      <!-- Permission Mode -->
      <div>
        <label class="text-[11px] font-medium text-foreground block mb-1"
          >{t("agent_permissionMode")}</label
        >
        <select
          class="w-full rounded-md border border-border bg-background px-3 py-1.5 text-xs text-foreground
            focus:outline-none focus:ring-1 focus:ring-primary disabled:opacity-50"
          bind:value={formData.permissionMode}
          disabled={mode === "edit"}
        >
          <option value="default">default</option>
          <option value="acceptEdits">acceptEdits</option>
          <option value="dontAsk">dontAsk</option>
          <option value="bypassPermissions">bypassPermissions</option>
          <option value="plan">plan</option>
        </select>
      </div>

      <!-- Max Turns -->
      <div>
        <label class="text-[11px] font-medium text-foreground block mb-1"
          >{t("agent_maxTurns")}</label
        >
        <input
          type="number"
          class="w-24 rounded-md border border-border bg-background px-3 py-1.5 text-xs text-foreground
            focus:outline-none focus:ring-1 focus:ring-primary disabled:opacity-50"
          placeholder="10"
          value={formData.maxTurns ?? ""}
          oninput={(e) => {
            const v = (e.target as HTMLInputElement).value;
            formData.maxTurns = v ? parseInt(v, 10) : null;
          }}
          disabled={mode === "edit"}
        />
      </div>

      <!-- Memory -->
      <div>
        <label class="text-[11px] font-medium text-foreground block mb-1">{t("agent_memory")}</label
        >
        <input
          type="text"
          class="w-full rounded-md border border-border bg-background px-3 py-1.5 text-xs text-foreground
            focus:outline-none focus:ring-1 focus:ring-primary disabled:opacity-50"
          placeholder="MEMORY.md"
          bind:value={formData.memory}
          disabled={mode === "edit"}
        />
        <p class="text-[10px] text-muted-foreground mt-0.5">{t("agent_memory_hint")}</p>
      </div>

      <!-- Checkboxes -->
      <div class="flex gap-4">
        <label class="flex items-center gap-1.5 text-xs text-foreground">
          <input type="checkbox" bind:checked={formData.background} disabled={mode === "edit"} />
          {t("agent_background")}
        </label>
        <label class="flex items-center gap-1.5 text-xs text-foreground">
          <input
            type="checkbox"
            checked={formData.isolation === "worktree"}
            onchange={(e) => {
              formData.isolation = (e.target as HTMLInputElement).checked ? "worktree" : "";
            }}
            disabled={mode === "edit"}
          />
          {t("agent_isolation")}
        </label>
      </div>

      <!-- System Prompt -->
      <div>
        <label class="text-[11px] font-medium text-foreground block mb-1"
          >{t("agent_systemPrompt")}</label
        >
        <textarea
          class="w-full rounded-md border border-border bg-background px-3 py-2 text-xs text-foreground font-mono
            focus:outline-none focus:ring-1 focus:ring-primary resize-y disabled:opacity-50"
          rows="8"
          placeholder="You are a specialized agent..."
          bind:value={formData.systemPrompt}
          disabled={mode === "edit"}
        ></textarea>
      </div>
    </div>
  {:else}
    <!-- Source Mode -->
    <div>
      {#if mode === "create"}
        <div class="mb-2">
          <label class="text-[11px] font-medium text-foreground block mb-1">Scope</label>
          <div class="flex gap-2">
            <button
              class="px-3 py-1 text-xs rounded-md border transition-colors
                {scope === 'user'
                ? 'border-primary bg-primary/10 text-foreground'
                : 'border-border text-muted-foreground hover:text-foreground'}"
              onclick={() => (scope = "user")}
            >
              {t("agent_scopeUser")}
            </button>
            <button
              class="px-3 py-1 text-xs rounded-md border transition-colors
                {scope === 'project'
                ? 'border-primary bg-primary/10 text-foreground'
                : 'border-border text-muted-foreground hover:text-foreground'}"
              onclick={() => (scope = "project")}
            >
              {t("agent_scopeProject")}
            </button>
          </div>
        </div>
      {:else}
        <p class="text-[10px] text-muted-foreground mb-2">
          Scope: {agent?.scope === "user" ? t("agent_scopeUser") : t("agent_scopeProject")}
        </p>
      {/if}
      <textarea
        class="w-full rounded-md border border-border bg-background px-3 py-2 text-xs text-foreground font-mono
          focus:outline-none focus:ring-1 focus:ring-primary resize-y"
        rows="20"
        bind:value={sourceContent}
        spellcheck="false"
      ></textarea>
    </div>
  {/if}

  <!-- Footer -->
  <div class="flex justify-end gap-2 pt-2 border-t border-border">
    <button
      class="rounded-md border border-border px-3 py-1.5 text-xs text-muted-foreground hover:text-foreground"
      onclick={onCancel}
    >
      Cancel
    </button>
    <button
      class="rounded-md bg-primary px-3 py-1.5 text-xs text-primary-foreground hover:bg-primary/90 disabled:opacity-50"
      onclick={handleSave}
      disabled={saving}
    >
      {saving ? "Saving..." : mode === "create" ? t("agent_createAgent") : "Save"}
    </button>
  </div>
</div>
