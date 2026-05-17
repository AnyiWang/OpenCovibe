import type { McpServerInfo } from "$lib/types";

export function statusDotClass(status: string): string {
  switch (status) {
    case "connected":
      return "bg-emerald-500";
    case "failed":
      return "bg-destructive";
    case "needs-auth":
    case "pending":
      return "bg-amber-500";
    case "disabled":
      return "bg-muted-foreground/30";
    default:
      return "bg-muted-foreground/50";
  }
}

export function statusLabel(status: string): string {
  switch (status) {
    case "connected":
      return "Connected";
    case "failed":
      return "Failed";
    case "needs-auth":
      return "Needs Auth";
    case "pending":
      return "Pending";
    case "disabled":
      return "Disabled";
    default:
      return status;
  }
}

/** Deduplicate MCP servers by name. CLI may report the same server from
 *  multiple scopes (user/project/local) as separate entries; UI treats them
 *  as a single logical server. First occurrence wins. */
export function dedupeMcpServersByName(servers: McpServerInfo[]): McpServerInfo[] {
  const seen = new Set<string>();
  return servers.filter((s) => {
    if (seen.has(s.name)) return false;
    seen.add(s.name);
    return true;
  });
}

export function parseServersFromResponse(response: Record<string, unknown>): McpServerInfo[] {
  // The response shape from get_mcp_status is not fully documented.
  // Try common field names defensively.
  const arr = (response.servers ?? response.mcp_servers ?? response.data) as unknown;
  if (Array.isArray(arr)) {
    const parsed = arr.map((s: Record<string, unknown>) => ({
      name: String(s.name ?? "unknown"),
      status: String(s.status ?? "pending"),
      server_type: (s.type as string | undefined) ?? (s.server_type as string | undefined),
      scope: s.scope as string | undefined,
      error: s.error as string | undefined,
    }));
    return dedupeMcpServersByName(parsed);
  }
  return [];
}
