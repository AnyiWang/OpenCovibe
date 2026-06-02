<script lang="ts">
  import { providerIcon } from "$lib/utils/provider-icon";

  let {
    id,
    name,
    size = "md",
  }: {
    id: string;
    name: string;
    size?: "sm" | "md";
  } = $props();

  let icon = $derived(providerIcon(id, name));
  let dim = $derived(
    size === "sm" ? "h-4 w-4 text-[8px] rounded" : "h-7 w-7 text-[11px] rounded-md",
  );
</script>

<span
  class="inline-flex items-center justify-center font-semibold leading-none shrink-0 {dim}"
  style="background-color:{icon.bg};color:{icon.fg};"
  aria-hidden="true"
>
  {icon.label}
</span>
