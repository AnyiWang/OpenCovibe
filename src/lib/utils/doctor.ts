import { runDiagnostics } from "$lib/api";
import { t } from "$lib/i18n/index.svelte";
import { dbg } from "$lib/utils/debug";
import type { McpServerInfo, DiagnosticsReport, ConfigIssue } from "$lib/types";

const LARGE_FILE_THRESHOLD = 10000;

/**
 * Build a Markdown diagnostics report (mirrors CLI /doctor output).
 * Calls the Rust `run_diagnostics` IPC command, then formats the result.
 */
export async function buildDoctorReport(
  cwd: string,
  mcpServers?: McpServerInfo[],
): Promise<string> {
  dbg("doctor", "buildDoctorReport", { cwd, hasMcpServers: !!mcpServers });
  const report = await runDiagnostics(cwd);
  return formatReport(report, mcpServers);
}

function formatReport(r: DiagnosticsReport, mcpServers?: McpServerInfo[]): string {
  const lines: string[] = [];

  // ── Installation ──
  lines.push(`## ${t("doctor_sectionInstallation")}`);
  if (r.cli.found) {
    lines.push(`✅ ${t("doctor_cliFound", { version: r.cli.version ?? "unknown" })}`);
    if (r.cli.path) {
      lines.push(`└ ${t("doctor_cliPath", { path: r.cli.path })}`);
    }
    if (r.cli.latest) {
      if (r.cli.version === r.cli.latest) {
        lines.push(`└ Latest: v${r.cli.latest} (${t("doctor_cliUpToDate")})`);
      } else {
        lines.push(`└ ⚠️ ${t("doctor_cliUpdateAvailable", { latest: r.cli.latest })}`);
      }
    }
    if (r.cli.auto_update_channel) {
      lines.push(`└ ${t("doctor_cliAutoUpdate", { channel: r.cli.auto_update_channel })}`);
    }
    lines.push(
      r.cli.ripgrep_available
        ? `└ ${t("doctor_cliRipgrepOk")}`
        : `└ ⚠️ ${t("doctor_cliRipgrepMissing")}`,
    );
  } else {
    lines.push(`❌ ${t("doctor_cliNotFound")}`);
  }

  // ── Authentication ──
  lines.push("");
  lines.push(`## ${t("doctor_sectionAuth")}`);
  if (r.auth.has_oauth) {
    lines.push(
      r.auth.oauth_account
        ? `✅ ${t("doctor_authOauth", { account: r.auth.oauth_account })}`
        : `✅ ${t("doctor_authOauthNoAccount")}`,
    );
  } else {
    lines.push(`⚠️ ${t("doctor_authNoOauth")}`);
  }
  if (r.auth.has_api_key) {
    lines.push(
      `✅ ${t("doctor_authApiKey", { source: r.auth.api_key_source ?? "unknown", hint: r.auth.api_key_hint ?? "***" })}`,
    );
  } else {
    lines.push(`⚠️ ${t("doctor_authNoApiKey")}`);
  }
  if (r.auth.app_has_credentials) {
    lines.push(`✅ ${t("doctor_authAppCreds", { name: r.auth.app_platform_name ?? "custom" })}`);
  } else {
    lines.push(`⚠️ ${t("doctor_authAppNoCreds")}`);
  }

  // ── Project ──
  lines.push("");
  lines.push(`## ${t("doctor_sectionProject")}`);
  if (r.project.skipped_project_scope) {
    lines.push(`⚠️ ${t("doctor_projectSkipped")}`);
  }
  if (r.project.has_claude_md) {
    lines.push(`✅ ${t("doctor_projectClaudeMd")}`);
  } else if (!r.project.skipped_project_scope) {
    lines.push(`⚠️ ${t("doctor_projectNoClaudeMd")}`);
  }
  for (const f of r.project.claude_md_files) {
    if (f.size_chars > LARGE_FILE_THRESHOLD) {
      lines.push(
        `⚠️ ${t("doctor_projectLargeFile", { path: f.path, size: String(f.size_chars) })}`,
      );
    }
  }

  // ── Configuration ──
  lines.push("");
  lines.push(`## ${t("doctor_sectionConfig")}`);
  const allIssues: ConfigIssue[] = [
    ...r.configs.settings_issues,
    ...r.configs.keybinding_issues,
    ...r.configs.mcp_issues,
    ...r.configs.env_var_issues,
  ];
  if (allIssues.length === 0) {
    lines.push(`✅ ${t("doctor_configSettingsOk")}`);
    lines.push(`✅ ${t("doctor_configKeybindingsOk")}`);
    lines.push(`✅ ${t("doctor_configMcpOk")}`);
    lines.push(`✅ ${t("doctor_configEnvOk")}`);
  } else {
    // Group non-issue categories
    if (r.configs.settings_issues.length === 0) lines.push(`✅ ${t("doctor_configSettingsOk")}`);
    if (r.configs.keybinding_issues.length === 0)
      lines.push(`✅ ${t("doctor_configKeybindingsOk")}`);
    if (r.configs.mcp_issues.length === 0) lines.push(`✅ ${t("doctor_configMcpOk")}`);
    if (r.configs.env_var_issues.length === 0) lines.push(`✅ ${t("doctor_configEnvOk")}`);
    for (const issue of allIssues) {
      const icon = issue.severity === "error" ? "❌" : "⚠️";
      lines.push(`${icon} ${issue.file}: ${issue.message}`);
    }
  }

  // ── MCP Servers (session) ──
  if (mcpServers && mcpServers.length > 0) {
    lines.push("");
    lines.push(`## ${t("doctor_sectionMcpSession")}`);
    for (const s of mcpServers) {
      const typeStr = s.server_type ? ` (${s.server_type})` : "";
      if (s.status === "connected" || s.status === "running") {
        lines.push(`✅ ${s.name}${typeStr}`);
      } else if (s.error) {
        lines.push(`❌ ${s.name}${typeStr} — ${s.error}`);
      } else {
        lines.push(`⚠️ ${s.name}${typeStr} — ${s.status}`);
      }
    }
  }

  // ── External Services ──
  lines.push("");
  lines.push(`## ${t("doctor_sectionServices")}`);
  if (r.services.community_registry === true) {
    lines.push(`✅ ${t("doctor_serviceCommunityOk")}`);
  } else if (r.services.community_registry === false) {
    lines.push(`❌ ${t("doctor_serviceCommunityFail")}`);
  } else {
    lines.push(`⚠️ ${t("doctor_serviceCommunityUnknown")}`);
  }
  if (r.services.mcp_registry === true) {
    lines.push(`✅ ${t("doctor_serviceMcpOk")}`);
  } else if (r.services.mcp_registry === false) {
    lines.push(`❌ ${t("doctor_serviceMcpFail")}`);
  } else {
    lines.push(`⚠️ ${t("doctor_serviceMcpUnknown")}`);
  }

  // ── System ──
  lines.push("");
  lines.push(`## ${t("doctor_sectionSystem")}`);
  if (r.system.sandbox_available === true) {
    lines.push(`✅ ${t("doctor_systemSandboxOk")}`);
  } else if (r.system.sandbox_available === false) {
    lines.push(`⚠️ ${t("doctor_systemSandboxMissing")}`);
  }
  if (r.system.lock_files.length === 0) {
    lines.push(`└ ${t("doctor_systemNoLocks")}`);
  } else {
    lines.push(`⚠️ ${t("doctor_systemLocks", { count: String(r.system.lock_files.length) })}`);
    for (const f of r.system.lock_files) {
      lines.push(`└ ${f}`);
    }
  }

  // Markdown requires trailing "  " (two spaces) for hard line breaks within a block.
  // Headings and blank lines don't need it.
  return lines.map((l) => (l === "" || l.startsWith("##") ? l : l + "  ")).join("\n");
}
