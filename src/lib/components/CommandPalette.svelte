<script lang="ts">
  import { goto } from "$app/navigation";
  import {
    filterCommands,
    groupByCategory,
    categoryLabels,
    type CommandDef,
    type CommandCategory,
  } from "$lib/commands";
  import * as api from "$lib/api";
  import { dbgWarn } from "$lib/utils/debug";
  import { t } from "$lib/i18n/index.svelte";

  let {
    open = $bindable(false),
    agent = "claude",
    runId = "",
    cwd = "/",
    onSendPrompt,
    onTogglePlanMode,
    onOpenModelSelector,
    onOpenFolderBrowser,
  }: {
    open: boolean;
    agent?: string;
    runId?: string;
    cwd?: string;
    onSendPrompt?: (prompt: string) => void;
    onTogglePlanMode?: () => void;
    onOpenModelSelector?: () => void;
    onOpenFolderBrowser?: () => void;
  } = $props();

  let query = $state("");
  let selectedIndex = $state(0);
  let inputEl: HTMLInputElement | undefined = $state();

  let filtered = $derived(filterCommands(query, agent));
  let grouped = $derived(groupByCategory(filtered));
  let flatList = $derived(filtered);

  // Reset on open
  $effect(() => {
    if (open) {
      query = "";
      selectedIndex = 0;
      requestAnimationFrame(() => inputEl?.focus());
    }
  });

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.preventDefault();
      open = false;
      return;
    }
    if (e.key === "ArrowDown") {
      e.preventDefault();
      selectedIndex = Math.min(selectedIndex + 1, flatList.length - 1);
      scrollToSelected();
      return;
    }
    if (e.key === "ArrowUp") {
      e.preventDefault();
      selectedIndex = Math.max(selectedIndex - 1, 0);
      scrollToSelected();
      return;
    }
    if (e.key === "Enter" && flatList.length > 0) {
      e.preventDefault();
      executeCommand(flatList[selectedIndex]);
      return;
    }
  }

  function scrollToSelected() {
    const el = document.querySelector(`[data-cmd-idx="${selectedIndex}"]`);
    el?.scrollIntoView({ block: "nearest" });
  }

  async function executeCommand(cmd: CommandDef) {
    open = false;

    switch (cmd.action) {
      case "navigate":
        if (cmd.payload) goto(cmd.payload);
        break;

      case "send_prompt":
        if (cmd.payload) onSendPrompt?.(cmd.payload);
        break;

      case "toggle_state":
        if (cmd.payload === "plan_mode") onTogglePlanMode?.();
        break;

      case "open_modal":
        if (cmd.payload === "model-selector") onOpenModelSelector?.();
        else if (cmd.payload === "folder-browser") onOpenFolderBrowser?.();
        else if (cmd.payload === "version-info") showVersionInfo();
        break;

      case "ipc_command":
        await handleIpcCommand(cmd);
        break;
    }
  }

  async function handleIpcCommand(cmd: CommandDef) {
    switch (cmd.payload) {
      case "get_git_diff":
        try {
          const diff = await api.getGitDiff(cwd, false);
          showResultModal(t("cmd_gitDiff"), diff || t("cmd_noChanges"));
        } catch (e) {
          showResultModal(t("cmd_error"), String(e));
        }
        break;

      case "get_git_status":
        try {
          const status = await api.getGitStatus(cwd);
          showResultModal(t("cmd_gitStatus"), status || t("cmd_workingTreeClean"));
        } catch (e) {
          showResultModal(t("cmd_error"), String(e));
        }
        break;

      case "get_run_artifacts":
        if (runId) {
          try {
            const a = await api.getRunArtifacts(runId);
            const info = [
              `Cost: ${a.cost_estimate != null ? "$" + a.cost_estimate.toFixed(4) : "N/A"}`,
              `Files changed: ${a.files_changed.length}`,
              `Commands: ${a.commands.length}`,
            ].join("\n");
            showResultModal(t("cmd_runInfo"), info);
          } catch (e) {
            showResultModal(t("cmd_error"), String(e));
          }
        }
        break;

      case "export_conversation":
        if (runId) {
          try {
            const md = await api.exportConversation(runId);
            const { save } = await import("@tauri-apps/plugin-dialog");
            const path = await save({
              defaultPath: `conversation-${runId.slice(0, 8)}.md`,
              filters: [{ name: "Markdown", extensions: ["md"] }],
            });
            if (path) await api.writeTextFile(path, md);
          } catch (e) {
            dbgWarn("cmd", "command error", e);
          }
        }
        break;

      case "stop_run":
        if (runId) {
          try {
            await api.stopRun(runId);
          } catch (e) {
            dbgWarn("cmd", "stop_run error", e);
          }
        }
        break;

      case "check_agent_cli":
        try {
          const claude = await api.checkAgentCli("claude");
          const lines = [
            `Claude: ${claude.found ? t("cmd_cliInstalled") : t("cmd_cliNotFound")}`,
            claude.path ? `  Path: ${claude.path}` : "",
            claude.version ? `  Version: ${claude.version}` : "",
          ]
            .filter(Boolean)
            .join("\n");
          showResultModal(t("cmd_doctor"), lines);
        } catch (e) {
          showResultModal(t("cmd_error"), String(e));
        }
        break;
    }
  }

  // Simple result modal state
  let resultModalOpen = $state(false);
  let resultModalTitle = $state("");
  let resultModalContent = $state("");

  function showResultModal(title: string, content: string) {
    resultModalTitle = title;
    resultModalContent = content;
    resultModalOpen = true;
  }

  function showVersionInfo() {
    showResultModal(t("cmd_versionInfo"), t("cmd_versionContent"));
  }

  // Compute global index for each command in grouped view
  let indexMap = $derived.by(() => {
    const map = new Map<string, number>();
    let idx = 0;
    const categoryOrder: CommandCategory[] = [
      "chat",
      "tools",
      "navigation",
      "settings",
      "diagnostics",
    ];
    for (const cat of categoryOrder) {
      for (const cmd of grouped[cat]) {
        map.set(cmd.id, idx++);
      }
    }
    return map;
  });
</script>

{#if open}
  <div
    class="fixed inset-0 z-50 flex items-start justify-center pt-[20vh]"
    role="dialog"
    aria-modal="true"
  >
    <!-- Backdrop -->
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div
      class="fixed inset-0 bg-black/50 backdrop-blur-sm"
      onclick={() => (open = false)}
      onkeydown={() => {}}
    ></div>

    <!-- Palette -->
    <div
      class="relative z-50 w-full max-w-xl rounded-lg border bg-background shadow-2xl animate-fade-in"
    >
      <!-- Search -->
      <div class="flex items-center gap-2 border-b px-4 py-3">
        <svg
          class="h-4 w-4 text-muted-foreground shrink-0"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
          stroke-linecap="round"
          stroke-linejoin="round"><circle cx="11" cy="11" r="8" /><path d="m21 21-4.3-4.3" /></svg
        >
        <input
          bind:this={inputEl}
          bind:value={query}
          onkeydown={handleKeydown}
          class="flex-1 bg-transparent text-sm outline-none placeholder:text-muted-foreground"
          placeholder={t("cmd_placeholder")}
        />
        <kbd class="hidden sm:inline text-xs text-muted-foreground bg-muted rounded px-1.5 py-0.5"
          >{t("cmd_esc")}</kbd
        >
      </div>

      <!-- Results -->
      <div class="max-h-[40vh] overflow-y-auto p-2">
        {#each ["chat", "tools", "navigation", "settings", "diagnostics"] as cat}
          {#if grouped[cat as CommandCategory].length > 0}
            <div class="mb-1">
              <p
                class="px-2 py-1 text-[10px] font-semibold text-muted-foreground uppercase tracking-wider"
              >
                {categoryLabels[cat as CommandCategory]}
              </p>
              {#each grouped[cat as CommandCategory] as cmd}
                {@const idx = indexMap.get(cmd.id) ?? 0}
                <button
                  data-cmd-idx={idx}
                  class="flex w-full items-center gap-3 rounded-md px-3 py-2 text-sm transition-colors
                    {idx === selectedIndex
                    ? 'bg-accent text-accent-foreground'
                    : 'hover:bg-accent/50'}"
                  onclick={() => executeCommand(cmd)}
                  onmouseenter={() => (selectedIndex = idx)}
                >
                  <span class="flex-1 text-left">{cmd.name}</span>
                  <span class="text-xs text-muted-foreground">{cmd.description}</span>
                  {#if cmd.shortcut}
                    <kbd class="text-[10px] text-muted-foreground bg-muted rounded px-1 py-0.5"
                      >{cmd.shortcut}</kbd
                    >
                  {/if}
                </button>
              {/each}
            </div>
          {/if}
        {/each}

        {#if flatList.length === 0}
          <div class="py-6 text-center text-sm text-muted-foreground">
            {t("cmd_noCommandsFound")}
          </div>
        {/if}
      </div>
    </div>
  </div>
{/if}

<!-- Result modal -->
{#if resultModalOpen}
  <div
    class="fixed inset-0 z-[60] flex items-center justify-center"
    role="dialog"
    aria-modal="true"
    tabindex="-1"
    onkeydown={(e) => {
      if (e.key === "Escape") resultModalOpen = false;
    }}
  >
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div
      class="fixed inset-0 bg-black/50 backdrop-blur-sm"
      onclick={() => (resultModalOpen = false)}
      onkeydown={(e) => e.key === "Escape" && (resultModalOpen = false)}
    ></div>
    <div
      class="relative z-[60] w-full max-w-lg rounded-lg border bg-background p-6 shadow-lg animate-fade-in"
    >
      <div class="flex items-center justify-between mb-4">
        <h2 class="text-lg font-semibold">{resultModalTitle}</h2>
        <button
          class="rounded-md p-1 hover:bg-accent transition-colors"
          onclick={() => (resultModalOpen = false)}
        >
          <svg
            class="h-4 w-4"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"><path d="M18 6 6 18" /><path d="m6 6 12 12" /></svg
          >
        </button>
      </div>
      <pre
        class="max-h-[50vh] overflow-auto rounded-lg bg-muted/50 p-4 text-xs font-mono leading-relaxed whitespace-pre-wrap">{resultModalContent}</pre>
    </div>
  </div>
{/if}
