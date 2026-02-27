<script lang="ts">
  import type { SessionInfoData } from "$lib/types";
  import { goto } from "$app/navigation";
  import { t } from "$lib/i18n/index.svelte";
  import { dbg } from "$lib/utils/debug";
  import { fmtNumber } from "$lib/i18n/format";

  let {
    info = null,
    activeTab = "info",
  }: {
    info: SessionInfoData | null;
    activeTab?: string;
  } = $props();

  // ── Duration timer (running sessions only) ──
  let elapsed = $state(0);

  $effect(() => {
    // Only tick when panel is visible AND session is running
    if (activeTab !== "info" || !info || info.status !== "running" || !info.startedAt) {
      return;
    }
    elapsed = Date.now() - new Date(info.startedAt).getTime();
    const timer = setInterval(() => {
      elapsed = Date.now() - new Date(info!.startedAt!).getTime();
    }, 1000);
    return () => clearInterval(timer);
  });

  let displayDuration = $derived.by(() => {
    if (!info?.startedAt) return null;
    if (info.status === "running") {
      return formatDuration(elapsed);
    }
    if (info.endedAt) {
      return formatDuration(new Date(info.endedAt).getTime() - new Date(info.startedAt).getTime());
    }
    return null;
  });

  // ── Helpers ──

  function formatDuration(ms: number): string {
    if (ms < 0) ms = 0;
    const totalSec = Math.floor(ms / 1000);
    const h = Math.floor(totalSec / 3600);
    const m = Math.floor((totalSec % 3600) / 60);
    const s = totalSec % 60;
    if (h > 0) return `${h}h ${m}m ${s}s`;
    if (m > 0) return `${m}m ${s}s`;
    return `${s}s`;
  }

  function shortSessionId(id: string | undefined): string {
    if (!id) return "—";
    return id.slice(0, 8);
  }

  let copied = $state(false);
  function copySessionId() {
    if (!info?.sessionId) return;
    navigator.clipboard.writeText(info.sessionId).then(() => {
      copied = true;
      setTimeout(() => (copied = false), 1500);
    });
  }

  $effect(() => {
    if (info) {
      dbg("info-panel", "render", { status: info.status });
    }
  });
</script>

<div class="flex-1 overflow-y-auto">
  {#if !info}
    <div class="flex items-center justify-center h-32 text-xs text-muted-foreground/50">
      {t("infoPanel_noInfo")}
    </div>
  {:else}
    <!-- Session section -->
    <div class="px-3 py-2 border-b border-border/50">
      <div class="text-[10px] font-semibold text-muted-foreground uppercase tracking-wider mb-1.5">
        {t("infoPanel_session")}
      </div>
      <div class="space-y-1">
        {#if info.sessionId}
          <div class="flex items-center justify-between text-[11px]">
            <span class="text-muted-foreground">{t("infoPanel_sessionId")}</span>
            <button
              class="font-mono text-foreground/80 hover:text-foreground transition-colors"
              onclick={copySessionId}
              title={info.sessionId}
            >
              {copied ? t("infoPanel_copied") : shortSessionId(info.sessionId)}
            </button>
          </div>
        {/if}
        {#if info.runName}
          <div class="flex items-center justify-between text-[11px]">
            <span class="text-muted-foreground">{t("infoPanel_runName")}</span>
            <span class="text-foreground/80 truncate ml-2 max-w-[140px]">{info.runName}</span>
          </div>
        {/if}
        <div class="flex items-center justify-between text-[11px]">
          <span class="text-muted-foreground">{t("infoPanel_status")}</span>
          <span
            class="font-medium {info.status === 'running'
              ? 'text-emerald-500'
              : info.status === 'failed'
                ? 'text-destructive'
                : 'text-foreground/80'}"
          >
            {info.status}
          </span>
        </div>
        {#if displayDuration}
          <div class="flex items-center justify-between text-[11px]">
            <span class="text-muted-foreground">{t("infoPanel_duration")}</span>
            <span class="text-foreground/80 font-mono tabular-nums">{displayDuration}</span>
          </div>
        {/if}
        {#if info.lastTurnDurationMs > 0}
          <div class="flex items-center justify-between text-[11px]">
            <span class="text-muted-foreground">{t("infoPanel_lastTurn")}</span>
            <span class="text-foreground/80 font-mono tabular-nums"
              >{formatDuration(info.lastTurnDurationMs)}</span
            >
          </div>
        {/if}
        <div class="flex items-center justify-between text-[11px]">
          <span class="text-muted-foreground">{t("infoPanel_turns")}</span>
          <span class="text-foreground/80">{info.numTurns}</span>
        </div>
      </div>
    </div>

    <!-- Model section -->
    <div class="px-3 py-2 border-b border-border/50">
      <div class="text-[10px] font-semibold text-muted-foreground uppercase tracking-wider mb-1.5">
        {t("infoPanel_modelSection")}
      </div>
      <div class="space-y-1">
        <div class="flex items-center justify-between text-[11px]">
          <span class="text-muted-foreground">{t("infoPanel_model")}</span>
          <span class="text-foreground/80 truncate ml-2 max-w-[140px]">{info.model || "—"}</span>
        </div>
        <div class="flex items-center justify-between text-[11px]">
          <span class="text-muted-foreground">{t("infoPanel_agent")}</span>
          <span class="text-foreground/80">{info.agent || "—"}</span>
        </div>
        {#if info.cliVersion}
          <div class="flex items-center justify-between text-[11px]">
            <span class="text-muted-foreground">{t("infoPanel_cliVersion")}</span>
            <span class="text-foreground/80">
              v{info.cliVersion}
              {#if info.cliUpdateAvailable}
                <span class="text-primary/80 ml-1"
                  >{t("infoPanel_cliUpdateAvailable", { version: info.cliUpdateAvailable })}</span
                >
              {/if}
            </span>
          </div>
        {/if}
        <div class="flex items-center justify-between text-[11px]">
          <span class="text-muted-foreground">{t("infoPanel_permissionMode")}</span>
          <span class="text-foreground/80">{info.permissionMode || "—"}</span>
        </div>
        {#if info.fastModeState}
          <div class="flex items-center justify-between text-[11px]">
            <span class="text-muted-foreground">{t("infoPanel_fastMode")}</span>
            <span class="text-foreground/80">{info.fastModeState}</span>
          </div>
        {/if}
      </div>
    </div>

    <!-- Environment section -->
    <div class="px-3 py-2 border-b border-border/50">
      <div class="text-[10px] font-semibold text-muted-foreground uppercase tracking-wider mb-1.5">
        {t("infoPanel_environment")}
      </div>
      <div class="space-y-1">
        <div class="flex items-start justify-between text-[11px]">
          <span class="text-muted-foreground shrink-0">{t("infoPanel_cwd")}</span>
          <span
            class="text-foreground/80 truncate ml-2 max-w-[160px] text-right font-mono text-[10px]"
            >{info.cwd || "—"}</span
          >
        </div>
        {#if info.remoteHostName}
          <div class="flex items-center justify-between text-[11px]">
            <span class="text-muted-foreground">{t("infoPanel_remote")}</span>
            <span class="text-foreground/80">{info.remoteHostName}</span>
          </div>
        {/if}
        {#if info.platformId}
          <div class="flex items-center justify-between text-[11px]">
            <span class="text-muted-foreground">{t("infoPanel_platform")}</span>
            <span class="text-foreground/80">{info.platformId}</span>
          </div>
        {/if}
        {#if info.mcpServers.length > 0}
          <div class="text-[11px]">
            <span class="text-muted-foreground"
              >{t("infoPanel_mcpServers")} ({info.mcpServers.length})</span
            >
            <div class="mt-1 space-y-0.5">
              {#each info.mcpServers as server}
                <div class="flex items-center gap-1.5 pl-2">
                  <span
                    class="h-1.5 w-1.5 rounded-full shrink-0 {server.status === 'connected' ||
                    server.status === 'running'
                      ? 'bg-emerald-500'
                      : server.status === 'error' || server.status === 'failed'
                        ? 'bg-destructive'
                        : 'bg-muted-foreground/40'}"
                  ></span>
                  <span class="text-[10px] text-foreground/70 min-w-0 truncate">{server.name}</span>
                </div>
              {/each}
            </div>
          </div>
        {/if}
      </div>
    </div>

    <!-- Authentication section -->
    {#if info.authSourceLabel || info.platformName}
      <div class="px-3 py-2 border-b border-border/50">
        <div
          class="text-[10px] font-semibold text-muted-foreground uppercase tracking-wider mb-1.5"
        >
          {t("infoPanel_auth")}
        </div>
        <div class="space-y-1">
          {#if info.authSourceLabel}
            <div class="flex items-center justify-between text-[11px]">
              <span class="text-muted-foreground">{t("infoPanel_authSource")}</span>
              <span class="text-foreground/80">{info.authSourceLabel}</span>
            </div>
          {/if}
          {#if info.platformName}
            <div class="flex items-center justify-between text-[11px]">
              <span class="text-muted-foreground">{t("infoPanel_platformName")}</span>
              <span class="text-foreground/80">{info.platformName}</span>
            </div>
          {/if}
        </div>
      </div>
    {/if}

    <!-- Context section -->
    {#if info.contextWindow > 0}
      <div class="px-3 py-2 border-b border-border/50">
        <div
          class="text-[10px] font-semibold text-muted-foreground uppercase tracking-wider mb-1.5"
        >
          {t("infoPanel_contextSection")}
        </div>
        <div class="space-y-1">
          <div class="flex items-center justify-between text-[11px]">
            <span class="text-muted-foreground">{t("infoPanel_contextWindow")}</span>
            <span class="text-foreground/80">{fmtNumber(info.contextWindow)} tokens</span>
          </div>
          {#if info.contextUtilization > 0}
            {@const pct = Math.round(info.contextUtilization * 100)}
            {@const barColor =
              pct >= 90 ? "bg-orange-500" : pct >= 70 ? "bg-amber-500" : "bg-emerald-500"}
            <div class="flex items-center justify-between text-[11px]">
              <span class="text-muted-foreground">{t("infoPanel_contextUsage")}</span>
              <span class="flex items-center gap-1.5">
                <span class="inline-flex h-1.5 w-16 rounded-full bg-foreground/10 overflow-hidden">
                  <span
                    class="h-full rounded-full transition-all duration-500 {barColor}"
                    style="width: {pct}%"
                  ></span>
                </span>
                <span class="text-foreground/80 tabular-nums">{pct}%</span>
              </span>
            </div>
          {/if}
          {#if info.compactCount > 0 || info.microcompactCount > 0}
            <div class="flex items-center justify-between text-[11px]">
              <span class="text-muted-foreground">{t("infoPanel_compactions")}</span>
              <span class="text-foreground/80"
                >{info.compactCount} full + {info.microcompactCount} micro</span
              >
            </div>
          {/if}
        </div>
      </div>
    {/if}

    <!-- Quick links -->
    <div class="px-3 py-2">
      <div class="flex items-center gap-2">
        <button
          class="text-[10px] text-muted-foreground hover:text-foreground transition-colors px-1.5 py-0.5 rounded hover:bg-accent"
          onclick={() => goto("/settings?tab=cli-config")}
        >
          {t("infoPanel_goConfig")}
        </button>
        <button
          class="text-[10px] text-muted-foreground hover:text-foreground transition-colors px-1.5 py-0.5 rounded hover:bg-accent"
          onclick={() => goto("/usage")}
        >
          {t("infoPanel_goUsage")}
        </button>
      </div>
    </div>
  {/if}
</div>
