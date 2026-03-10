<script lang="ts">
  import { onMount } from "svelte";
  import { checkForUpdates } from "$lib/api";
  import { renderMarkdown } from "$lib/utils/markdown";
  import { currentLocale, t } from "$lib/i18n/index.svelte";
  import readmeEn from "../../../README.md?raw";
  import readmeZhCN from "../../../README.zh-CN.md?raw";

  let { open = $bindable(false) }: { open: boolean } = $props();

  let appVersion = $state("");
  let checkingUpdate = $state(false);
  onMount(async () => {
    try {
      const { getVersion } = await import("@tauri-apps/api/app");
      appVersion = await getVersion();
    } catch {
      appVersion = "";
    }
  });

  /** Fix image paths for Tauri webview and remove redundant language switcher. */
  function processReadme(html: string): string {
    return html
      .replace(/src="static\//g, 'src="/')
      .replace(/<p align="center">[\s\S]*?<\/p>/g, (match) =>
        match.includes("README") ? "" : match,
      )
      .replace(/href="LICENSE"/g, 'href="#"')
      .trim();
  }

  const readmeHtmlMap: Record<string, string> = {
    en: processReadme(renderMarkdown(readmeEn)),
    "zh-CN": processReadme(renderMarkdown(readmeZhCN)),
  };

  let readmeHtml = $derived(readmeHtmlMap[currentLocale()] ?? readmeHtmlMap.en);

  function handleBackdropClick(e: MouseEvent) {
    if (e.target === e.currentTarget) open = false;
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === "Escape") open = false;
  }

  async function updateToLatest() {
    if (checkingUpdate) return;
    checkingUpdate = true;
    try {
      const info = await checkForUpdates();
      if (!info.latestVersion) {
        window.alert(t("appUpdate_checkFailed"));
        return;
      }
      if (!info.hasUpdate) {
        window.alert(
          t("appUpdate_upToDate", { version: info.currentVersion || appVersion || "-" }),
        );
        return;
      }
      if (!info.downloadUrl) {
        window.alert(t("appUpdate_checkFailed"));
        return;
      }
      try {
        const { open } = await import("@tauri-apps/plugin-shell");
        await open(info.downloadUrl);
      } catch {
        window.open(info.downloadUrl, "_blank");
      }
    } catch {
      window.alert(t("appUpdate_checkFailed"));
    } finally {
      checkingUpdate = false;
    }
  }
</script>

{#if open}
  <div
    class="fixed inset-0 z-50 flex items-center justify-center bg-black/50 backdrop-blur-sm"
    role="dialog"
    aria-modal="true"
    onclick={handleBackdropClick}
    onkeydown={handleKeydown}
  >
    <div
      class="relative flex max-h-[85vh] w-full max-w-3xl flex-col rounded-xl border border-border bg-background shadow-2xl"
    >
      <!-- Header -->
      <div class="flex items-center justify-between border-b border-border px-6 py-4">
        <div class="flex items-center gap-3">
          <span class="text-xs text-muted-foreground"
            >{appVersion ? `OpenCovibe v${appVersion}` : ""}</span
          >
          <button
            class="rounded-md border border-border px-2.5 py-1 text-xs font-medium text-foreground transition-colors hover:bg-muted disabled:cursor-not-allowed disabled:opacity-60"
            onclick={updateToLatest}
            disabled={checkingUpdate}
          >
            {checkingUpdate ? t("appUpdate_checking") : t("appUpdate_manual")}
          </button>
        </div>
        <button
          class="rounded-md p-1.5 text-muted-foreground hover:bg-muted hover:text-foreground transition-colors"
          onclick={() => (open = false)}
          aria-label="Close"
        >
          <svg
            class="h-5 w-5"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"><path d="M18 6 6 18M6 6l12 12" /></svg
          >
        </button>
      </div>

      <!-- Content -->
      <div class="flex-1 overflow-y-auto px-6 py-4">
        <article class="prose prose-sm dark:prose-invert max-w-none">
          {@html readmeHtml}
        </article>
      </div>

      <!-- Footer -->
      <div
        class="flex items-center justify-between border-t border-border px-6 py-3 text-xs text-muted-foreground"
      >
        <span>Apache License 2.0</span>
        <span>Copyright 2025-2026 OpenCovibe Contributors</span>
      </div>
    </div>
  </div>
{/if}
