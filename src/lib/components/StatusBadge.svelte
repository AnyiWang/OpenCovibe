<script lang="ts">
  import type { RunStatus } from "$lib/types";

  let {
    status,
    class: className = "",
  }: {
    status: RunStatus;
    class?: string;
  } = $props();

  const colors: Record<RunStatus, string> = {
    pending: "bg-amber-500/20 text-amber-600 dark:text-amber-400",
    running: "bg-blue-500/20 text-blue-600 dark:text-blue-400",
    completed: "bg-emerald-500/20 text-emerald-600 dark:text-emerald-400",
    failed: "bg-red-500/20 text-red-600 dark:text-red-400",
    stopped: "bg-gray-500/20 text-gray-600 dark:text-gray-400",
  };

  const dots: Record<RunStatus, string> = {
    pending: "bg-amber-500",
    running: "bg-blue-500 animate-pulse",
    completed: "bg-emerald-500",
    failed: "bg-red-500",
    stopped: "bg-gray-500",
  };
</script>

<span
  class="inline-flex items-center gap-1.5 rounded-full px-2 py-0.5 text-xs font-medium {colors[
    status
  ]} {className}"
>
  <span class="h-1.5 w-1.5 rounded-full {dots[status]}"></span>
  {status}
</span>
