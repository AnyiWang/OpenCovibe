<script lang="ts">
  import { goto } from "$app/navigation";
  import { onMount } from "svelte";
  import { t } from "$lib/i18n/index.svelte";

  onMount(() => {
    goto("/chat");
  });
</script>

<div class="flex h-full items-center justify-center">
  <p class="text-muted-foreground">{t("common_redirecting")}</p>
</div>
