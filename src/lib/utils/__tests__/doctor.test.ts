import { describe, it, expect, vi, beforeEach } from "vitest";
import type { DiagnosticsReport, McpServerInfo } from "$lib/types";

// Mock api.ts — must be before dynamic import
vi.mock("$lib/api", () => ({
  runDiagnostics: vi.fn(),
}));

// Provide stable t() for test — returns key with interpolated values
vi.mock("$lib/i18n/index.svelte", () => ({
  t: (key: string, params?: Record<string, string>) => {
    if (!params) return key;
    let result = key;
    for (const [k, v] of Object.entries(params)) {
      result += `:${k}=${v}`;
    }
    return result;
  },
}));

function makeReport(overrides: Partial<DiagnosticsReport> = {}): DiagnosticsReport {
  return {
    cli: {
      found: true,
      version: "2.1.59",
      path: "/usr/local/bin/claude",
      latest: "2.1.59",
      stable: "2.1.50",
      auto_update_channel: "latest",
      ripgrep_available: true,
    },
    auth: {
      has_oauth: true,
      oauth_account: "user@example.com",
      has_api_key: true,
      api_key_hint: "...xxxx",
      api_key_source: "settings",
      app_has_credentials: false,
      app_platform_name: null,
    },
    project: {
      cwd: "/tmp/project",
      has_claude_md: true,
      claude_md_files: [{ path: "/tmp/project/CLAUDE.md", size_chars: 500 }],
      skipped_project_scope: false,
    },
    configs: {
      settings_issues: [],
      keybinding_issues: [],
      mcp_issues: [],
      env_var_issues: [],
    },
    services: {
      community_registry: true,
      mcp_registry: true,
    },
    system: {
      sandbox_available: true,
      lock_files: [],
    },
    ...overrides,
  };
}

describe("buildDoctorReport", () => {
  let buildDoctorReport: (cwd: string, mcpServers?: McpServerInfo[]) => Promise<string>;
  let mockRunDiagnostics: ReturnType<typeof vi.fn>;

  beforeEach(async () => {
    vi.resetModules();
    const apiModule = await import("$lib/api");
    mockRunDiagnostics = apiModule.runDiagnostics as ReturnType<typeof vi.fn>;
    const doctorModule = await import("../doctor");
    buildDoctorReport = doctorModule.buildDoctorReport;
  });

  it("renders all sections for a healthy report", async () => {
    mockRunDiagnostics.mockResolvedValue(makeReport());
    const text = await buildDoctorReport("/tmp/project");
    expect(text).toContain("doctor_sectionInstallation");
    expect(text).toContain("doctor_sectionAuth");
    expect(text).toContain("doctor_sectionProject");
    expect(text).toContain("doctor_sectionConfig");
    expect(text).toContain("doctor_sectionServices");
    expect(text).toContain("doctor_sectionSystem");
  });

  it("shows CLI version when found", async () => {
    mockRunDiagnostics.mockResolvedValue(makeReport());
    const text = await buildDoctorReport("/tmp/project");
    expect(text).toContain("✅");
    expect(text).toContain("doctor_cliFound");
    expect(text).toContain("2.1.59");
  });

  it("shows not found when CLI missing", async () => {
    mockRunDiagnostics.mockResolvedValue(
      makeReport({
        cli: {
          found: false,
          version: null,
          path: null,
          latest: null,
          stable: null,
          auto_update_channel: null,
          ripgrep_available: false,
        },
      }),
    );
    const text = await buildDoctorReport("/tmp/project");
    expect(text).toContain("❌");
    expect(text).toContain("doctor_cliNotFound");
  });

  it("shows config issues with correct icons", async () => {
    mockRunDiagnostics.mockResolvedValue(
      makeReport({
        configs: {
          settings_issues: [
            {
              scope: "user",
              file: "~/.claude/settings.json",
              severity: "error",
              message: "Invalid JSON",
            },
          ],
          keybinding_issues: [],
          mcp_issues: [
            {
              scope: "project",
              file: ".mcp.json",
              severity: "warning",
              message: 'missing "command"',
            },
          ],
          env_var_issues: [],
        },
      }),
    );
    const text = await buildDoctorReport("/tmp/project");
    expect(text).toContain("❌ ~/.claude/settings.json: Invalid JSON");
    expect(text).toContain('⚠️ .mcp.json: missing "command"');
  });

  it("shows MCP session servers when provided", async () => {
    mockRunDiagnostics.mockResolvedValue(makeReport());
    const mcpServers: McpServerInfo[] = [
      { name: "context7", status: "connected", server_type: "stdio" },
      { name: "broken", status: "error", error: "connection refused" },
    ];
    const text = await buildDoctorReport("/tmp/project", mcpServers);
    expect(text).toContain("doctor_sectionMcpSession");
    expect(text).toContain("✅ context7 (stdio)");
    expect(text).toContain("❌ broken — connection refused");
  });

  it("omits MCP session section when no servers", async () => {
    mockRunDiagnostics.mockResolvedValue(makeReport());
    const text = await buildDoctorReport("/tmp/project");
    expect(text).not.toContain("doctor_sectionMcpSession");
  });

  it("shows skipped project scope warning", async () => {
    mockRunDiagnostics.mockResolvedValue(
      makeReport({
        project: {
          cwd: "",
          has_claude_md: false,
          claude_md_files: [],
          skipped_project_scope: true,
        },
      }),
    );
    const text = await buildDoctorReport("");
    expect(text).toContain("doctor_projectSkipped");
  });

  it("warns about large CLAUDE.md files", async () => {
    mockRunDiagnostics.mockResolvedValue(
      makeReport({
        project: {
          cwd: "/tmp/project",
          has_claude_md: true,
          claude_md_files: [{ path: "~/.claude/CLAUDE.md", size_chars: 15000 }],
          skipped_project_scope: false,
        },
      }),
    );
    const text = await buildDoctorReport("/tmp/project");
    expect(text).toContain("doctor_projectLargeFile");
    expect(text).toContain("15000");
  });

  it("shows service health status", async () => {
    mockRunDiagnostics.mockResolvedValue(
      makeReport({
        services: {
          community_registry: false,
          mcp_registry: null,
        },
      }),
    );
    const text = await buildDoctorReport("/tmp/project");
    expect(text).toContain("❌");
    expect(text).toContain("doctor_serviceCommunityFail");
    expect(text).toContain("doctor_serviceMcpUnknown");
  });

  it("shows lock files when present", async () => {
    mockRunDiagnostics.mockResolvedValue(
      makeReport({
        system: {
          sandbox_available: true,
          lock_files: ["session-abc.lock", "session-def.lock"],
        },
      }),
    );
    const text = await buildDoctorReport("/tmp/project");
    expect(text).toContain("doctor_systemLocks");
    expect(text).toContain("session-abc.lock");
    expect(text).toContain("session-def.lock");
  });
});
