<script lang="ts">
  import { page } from "$app/stores";
  import { t } from "$lib/i18n/index.svelte";
</script>

<div class="flex h-full items-center justify-center">
  <div class="text-center max-w-md p-8">
    <div
      class="mx-auto mb-4 flex h-14 w-14 items-center justify-center rounded-2xl bg-destructive/10"
    >
      <svg
        class="h-7 w-7 text-destructive"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        stroke-width="2"
        stroke-linecap="round"
        stroke-linejoin="round"
      >
        <circle cx="12" cy="12" r="10" />
        <line x1="12" x2="12" y1="8" y2="12" />
        <line x1="12" x2="12.01" y1="16" y2="16" />
      </svg>
    </div>
    <h1 class="text-xl font-bold mb-2">{t("error_title")}</h1>
    <p class="text-sm text-muted-foreground mb-4">
      {$page.error?.message ?? t("error_defaultMessage")}
    </p>
    <a
      href="/chat"
      class="inline-flex items-center gap-2 rounded-md bg-primary px-4 py-2 text-sm font-medium text-primary-foreground hover:bg-primary/90 transition-colors"
    >
      {t("error_backToChat")}
    </a>
  </div>
</div>
