<script lang="ts">
  import Modal from "./Modal.svelte";
  import { t } from "$lib/i18n/index.svelte";
  import { getCliPermissions, updateCliPermissions, type CliPermissions } from "$lib/api";
  import { isAbsolutePath } from "$lib/utils/format";
  import { filterRules } from "$lib/utils/permissions-helpers";
  import { dbg, dbgWarn } from "$lib/utils/debug";

  let {
    open = $bindable(false),
    cwd = "",
  }: {
    open: boolean;
    cwd?: string;
  } = $props();

  // ── State ──
  let permissions = $state<CliPermissions | null>(null);
  let loading = $state(false);
  let error = $state<string | null>(null);
  let activeTab = $state<"allow" | "deny" | "projectAllow" | "projectDeny">("allow");
  let search = $state("");
  let saving = $state(false);
  let addInput = $state("");
  let _loadSeq = 0;
  let _opSeq = 0;

  // ── Derived ──
  const hasValidProjectCwd = $derived(!!cwd && isAbsolutePath(cwd) && !cwd.startsWith("~/"));
  const showProjectTabs = $derived(hasValidProjectCwd && !permissions?.projectError);

  // Active rules array based on current tab
  const activeRules = $derived.by(() => {
    if (!permissions) return [];
    switch (activeTab) {
      case "allow":
        return permissions.user.allow;
      case "deny":
        return permissions.user.deny;
      case "projectAllow":
        return permissions.project.allow;
      case "projectDeny":
        return permissions.project.deny;
      default:
        return [];
    }
  });

  const filteredRules = $derived(filterRules(activeRules, search));

  // ── Close reset ──
  $effect(() => {
    if (!open) {
      _loadSeq++;
      _opSeq++; // invalidate in-flight saves so they don't write back after close
      permissions = null;
      loading = false;
      error = null;
      activeTab = "allow";
      search = "";
      addInput = "";
      saving = false;
      prevCwd = null; // null sentinel ≠ any real cwd (including ""), guarantees load on next open
    }
  });

  // ── Load on open + cwd drift guard (single effect to avoid double-load) ──
  // null = "not yet loaded"; any string (including "") = last loaded cwd value
  let prevCwd: string | null = null;
  $effect(() => {
    if (!open) return;
    if (cwd !== prevCwd) {
      if (prevCwd !== null) {
        // cwd actually changed while modal was already open — reset project tab
        if (activeTab.startsWith("project")) {
          activeTab = "allow";
        }
      }
      prevCwd = cwd;
      load();
    }
  });

  async function load() {
    const seq = ++_loadSeq;
    loading = true;
    error = null;
    try {
      const result = await getCliPermissions(cwd || undefined);
      if (seq !== _loadSeq) return;
      permissions = result;
      dbg("permissions", "loaded", {
        userAllow: result.user.allow.length,
        userDeny: result.user.deny.length,
        projectAllow: result.project.allow.length,
        projectDeny: result.project.deny.length,
      });
      // Tab safety on projectError arrival
      if (result.projectError && activeTab.startsWith("project")) {
        activeTab = "allow";
      }
    } catch (e) {
      if (seq !== _loadSeq) return;
      dbgWarn("permissions", "load error", e);
      error = String(e);
    } finally {
      if (seq === _loadSeq) loading = false;
    }
  }

  // ── Scope/category helpers ──
  function getScope(tab: string): "user" | "project" {
    return tab.startsWith("project") ? "project" : "user";
  }

  function getCategory(tab: string): "allow" | "deny" {
    if (tab === "deny" || tab === "projectDeny") return "deny";
    return "allow";
  }

  function getRulesRef(
    perms: CliPermissions,
    scope: "user" | "project",
    category: "allow" | "deny",
  ): string[] {
    return perms[scope][category];
  }

  function setRulesRef(
    perms: CliPermissions,
    scope: "user" | "project",
    category: "allow" | "deny",
    rules: string[],
  ) {
    perms[scope][category] = rules;
  }

  // ── Save with serialization ──
  async function saveRules(scope: "user" | "project", category: "allow" | "deny", rules: string[]) {
    const seq = ++_opSeq;
    saving = true;
    try {
      await updateCliPermissions(scope, category, rules, cwd || undefined);
      if (seq !== _opSeq) return;
      dbg("permissions", "saved", { scope, category, count: rules.length });
    } catch (e) {
      if (seq !== _opSeq) return;
      if (!open) return; // modal closed while save was in-flight
      dbgWarn("permissions", "save error", e);
      error = String(e);
      await load(); // reload from disk to recover consistent state
    } finally {
      if (seq === _opSeq) saving = false;
    }
  }

  // ── Add rule ──
  function handleAdd() {
    const trimmed = addInput.trim();
    if (!trimmed || !permissions) return;

    const scope = getScope(activeTab);
    const category = getCategory(activeTab);
    const current = getRulesRef(permissions, scope, category);

    // Deduplicate
    if (current.includes(trimmed)) {
      addInput = "";
      return;
    }

    const updated = [...current, trimmed];
    setRulesRef(permissions, scope, category, updated);
    addInput = "";

    dbg("permissions", "add", { scope, category, count: updated.length });
    saveRules(scope, category, updated);
  }

  // ── Delete rule ──
  function handleDelete(rule: string) {
    if (!permissions) return;

    const scope = getScope(activeTab);
    const category = getCategory(activeTab);
    const current = getRulesRef(permissions, scope, category);

    const updated = current.filter((r) => r !== rule);
    setRulesRef(permissions, scope, category, updated);

    dbg("permissions", "delete", { scope, category, count: updated.length });
    saveRules(scope, category, updated);
  }

  function handleAddKeydown(e: KeyboardEvent) {
    if (e.key === "Enter") {
      e.preventDefault();
      handleAdd();
    }
  }

  // Tab definitions
  type TabDef = { id: typeof activeTab; labelKey: string; count: number };
  const tabs = $derived.by((): TabDef[] => {
    const base: TabDef[] = [
      { id: "allow", labelKey: "permissions_allow", count: permissions?.user.allow.length ?? 0 },
      { id: "deny", labelKey: "permissions_deny", count: permissions?.user.deny.length ?? 0 },
    ];
    if (showProjectTabs) {
      base.push(
        {
          id: "projectAllow",
          labelKey: "permissions_projectAllow",
          count: permissions?.project.allow.length ?? 0,
        },
        {
          id: "projectDeny",
          labelKey: "permissions_projectDeny",
          count: permissions?.project.deny.length ?? 0,
        },
      );
    }
    return base;
  });
</script>

<Modal bind:open title={t("permissions_title")}>
  {#if loading && !permissions}
    <div class="flex items-center justify-center py-8">
      <div
        class="h-5 w-5 animate-spin rounded-full border-2 border-muted-foreground border-t-transparent"
      ></div>
    </div>
  {:else if error && !permissions}
    <div class="rounded-md bg-destructive/10 p-3 text-sm text-destructive">
      {error}
    </div>
  {:else if permissions}
    <!-- Tab bar -->
    <div class="flex gap-1 mb-3 flex-wrap">
      {#each tabs as tab}
        <button
          class="px-2.5 py-1 text-xs rounded-md transition-colors
            {activeTab === tab.id
            ? 'bg-accent text-accent-foreground font-medium'
            : 'text-muted-foreground hover:bg-accent/50'}"
          onclick={() => (activeTab = tab.id)}
        >
          {(t as (key: string) => string)(tab.labelKey)}
          {#if tab.count > 0}
            <span class="ml-1 text-[10px] opacity-60">{tab.count}</span>
          {/if}
        </button>
      {/each}
    </div>

    <!-- Project error warning -->
    {#if hasValidProjectCwd && permissions.projectError}
      <div class="mb-3 rounded-md bg-muted/50 px-3 py-2 text-xs text-muted-foreground">
        {t("permissions_projectUnavailable", { reason: permissions.projectError })}
      </div>
    {/if}

    <!-- Search + Add -->
    <div class="flex gap-2 mb-3">
      <input
        bind:value={search}
        class="flex-1 rounded-md border bg-transparent px-2.5 py-1.5 text-sm outline-none placeholder:text-muted-foreground focus:border-ring"
        placeholder={t("permissions_search")}
      />
    </div>

    <div class="flex gap-2 mb-3">
      <input
        bind:value={addInput}
        onkeydown={handleAddKeydown}
        class="flex-1 rounded-md border bg-transparent px-2.5 py-1.5 text-sm outline-none placeholder:text-muted-foreground focus:border-ring"
        placeholder={t("permissions_addPlaceholder")}
        disabled={saving}
      />
      <button
        class="shrink-0 rounded-md bg-primary px-3 py-1.5 text-sm text-primary-foreground transition-colors hover:bg-primary/90 disabled:opacity-50"
        onclick={handleAdd}
        disabled={saving || !addInput.trim()}
      >
        {t("permissions_addRule")}
      </button>
    </div>

    <!-- Rule list -->
    <div class="max-h-[40vh] overflow-y-auto rounded-md border">
      {#if filteredRules.length === 0}
        <div class="py-6 text-center text-sm text-muted-foreground">
          {search ? t("permissions_noResults") : t("permissions_noRules")}
        </div>
      {:else}
        {#each filteredRules as rule}
          <div
            class="group flex items-center justify-between gap-2 border-b last:border-b-0 px-3 py-2"
          >
            <code class="flex-1 text-xs break-all font-mono">{rule}</code>
            <button
              class="shrink-0 rounded-md p-1 text-muted-foreground opacity-0 transition-colors hover:bg-destructive/10 hover:text-destructive group-hover:opacity-100"
              onclick={() => handleDelete(rule)}
              disabled={saving}
              title={t("permissions_deleteConfirm")}
            >
              <svg
                class="h-3.5 w-3.5"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                stroke-width="2"
                stroke-linecap="round"
                stroke-linejoin="round"
              >
                <path d="M18 6 6 18" /><path d="m6 6 12 12" />
              </svg>
            </button>
          </div>
        {/each}
      {/if}
    </div>

    <!-- Inline error -->
    {#if error}
      <div class="mt-2 rounded-md bg-destructive/10 p-2 text-xs text-destructive">
        {error}
      </div>
    {/if}

    <!-- Saving indicator -->
    {#if saving}
      <div class="mt-2 flex items-center gap-1.5 text-xs text-muted-foreground">
        <div
          class="h-3 w-3 animate-spin rounded-full border border-muted-foreground border-t-transparent"
        ></div>
        Saving...
      </div>
    {/if}
  {/if}
</Modal>
