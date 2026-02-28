<script lang="ts">
  import { checkForUpdates } from "$lib/api";
  import { dbg, dbgWarn } from "$lib/utils/debug";
  import { t } from "$lib/i18n/index.svelte";
  import { onMount } from "svelte";

  let hasUpdate = $state(false);
  let latestVersion = $state("");
  let downloadUrl = $state("");

  function isDismissed(version: string): boolean {
    return sessionStorage.getItem(`ocv:update-dismissed:${version}`) === "1";
  }

  function dismiss() {
    dbg("update-banner", "dismissed", latestVersion);
    sessionStorage.setItem(`ocv:update-dismissed:${latestVersion}`, "1");
    hasUpdate = false;
  }

  async function openDownload() {
    dbg("update-banner", "opening download", downloadUrl);
    try {
      const { open } = await import("@tauri-apps/plugin-shell");
      await open(downloadUrl);
    } catch {
      window.open(downloadUrl, "_blank");
    }
  }

  onMount(() => {
    const timerId = setTimeout(async () => {
      try {
        const info = await checkForUpdates();
        dbg("update-banner", "check result", info);
        if (info.hasUpdate && !isDismissed(info.latestVersion)) {
          hasUpdate = true;
          latestVersion = info.latestVersion;
          downloadUrl = info.downloadUrl;
        }
      } catch (e) {
        dbgWarn("update-banner", "check failed", e);
      }
    }, 3000);
    return () => clearTimeout(timerId);
  });
</script>

{#if hasUpdate}
  <div
    class="flex items-center justify-between gap-2 border-b border-primary/30 bg-primary/10 px-4 py-1.5 text-sm"
  >
    <span class="text-foreground">
      {t("appUpdate_available", { version: latestVersion })}
    </span>
    <div class="flex items-center gap-2">
      <button
        class="rounded-md bg-primary px-3 py-0.5 text-xs font-medium text-primary-foreground transition-colors hover:bg-primary/90"
        onclick={openDownload}
      >
        {t("appUpdate_download")}
      </button>
      <button
        class="rounded-md px-2 py-0.5 text-xs text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
        onclick={dismiss}
        title={t("appUpdate_dismiss")}
      >
        <svg
          class="h-3.5 w-3.5"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
          stroke-linecap="round"
          stroke-linejoin="round"><path d="M18 6 6 18" /><path d="m6 6 12 12" /></svg
        >
      </button>
    </div>
  </div>
{/if}
