<script lang="ts">
  import { onMount, getContext } from "svelte";
  import { goto } from "$app/navigation";
  import { page } from "$app/stores";
  import * as api from "$lib/api";
  import { loadCliInfo, KeybindingStore } from "$lib/stores";
  import type {
    UserSettings,
    AgentSettings,
    CliConfigSettingDef,
    RemoteHost,
    RemoteTestResult,
    SshKeyInfo,
    CodexAuthResult,
  } from "$lib/types";
  import Card from "$lib/components/Card.svelte";
  import Button from "$lib/components/Button.svelte";
  import Input from "$lib/components/Input.svelte";
  import KeybindingEditor from "$lib/components/KeybindingEditor.svelte";
  import ProviderIcon from "$lib/components/ProviderIcon.svelte";
  import { formatKeyDisplay } from "$lib/stores/keybindings.svelte";
  import {
    PLATFORM_PRESETS,
    PRESET_CATEGORIES,
    buildPlatformList,
    isCustomPlatform,
    findCredential,
    expandModelsToTiers,
    compressModelsFromTiers,
  } from "$lib/utils/platform-presets";
  import type { PlatformPreset, PlatformCredential, CodexProviderCredential } from "$lib/types";
  import {
    CODEX_PROVIDER_PRESETS,
    type CodexProviderPreset,
  } from "$lib/utils/codex-provider-presets";
  import {
    isDebugMode,
    setDebugMode,
    copyDebugLogs,
    getDebugLogCount,
    clearDebugLogs,
    getDebugFilter,
  } from "$lib/utils/debug";
  import { dbg, dbgWarn, redactSensitive } from "$lib/utils/debug";
  import { splitPath } from "$lib/utils/format";
  import { IS_WINDOWS } from "$lib/utils/platform";
  import { t, LOCALE_REGISTRY, currentLocale, switchLocale } from "$lib/i18n/index.svelte";
  import { getTransport } from "$lib/transport";

  // ── Tab state ──
  type SettingsTab = "general" | "connection" | "cli-config" | "shortcuts" | "remote" | "debug";
  const VALID_TABS: SettingsTab[] = [
    "general",
    "connection",
    "cli-config",
    "shortcuts",
    "remote",
    "debug",
  ];
  const urlTab = $page.url.searchParams.get("tab");
  const initialTab: SettingsTab = VALID_TABS.includes(urlTab as SettingsTab)
    ? (urlTab as SettingsTab)
    : "general";
  let activeTab = $state<SettingsTab>(initialTab);

  const tabLabels: Record<SettingsTab, () => string> = {
    general: () => t("settings_tab_general"),
    connection: () => t("settings_tab_connection"),
    "cli-config": () => t("settings_tab_cliConfig"),
    shortcuts: () => t("settings_tab_shortcuts"),
    remote: () => t("settings_tab_remote"),
    debug: () => t("settings_tab_debug"),
  };

  const tabs: { id: SettingsTab; icon: string }[] = [
    {
      id: "general",
      icon: "M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z M12 8a4 4 0 1 0 0 8 4 4 0 0 0 0-8z",
    },
    {
      id: "connection",
      icon: "M12 2a4 4 0 0 0-4 4c0 1.1.9 2 2 2h4a2 2 0 0 0 2-2 4 4 0 0 0-4-4z M8 8v2a4 4 0 0 0 8 0V8 M12 14v4 M8 18h8",
    },
    {
      id: "cli-config",
      icon: "M4 17l6-6-6-6 M12 19h8",
    },
    {
      id: "shortcuts",
      icon: "M10 8h.01 M12 12h.01 M14 8h.01 M16 12h.01 M18 8h.01 M6 8h.01 M7 16h10 M8 12h.01 M2 4h20v16H2z",
    },
    {
      id: "remote",
      icon: "M4 14a1 1 0 0 1-.78-1.63l9.9-10.2a.5.5 0 0 1 .86.46l-1.92 6.02A1 1 0 0 0 13 10h7a1 1 0 0 1 .78 1.63l-9.9 10.2a.5.5 0 0 1-.86-.46l1.92-6.02A1 1 0 0 0 11 14z",
    },
    { id: "debug", icon: "m18 16 4-4-4-4 M6 8l-4 4 4 4 M14.5 4l-5 16" },
  ];

  let settings = $state<UserSettings | null>(null);
  let authMode = $state("cli");
  let anthropicApiKey = $state("");
  let anthropicBaseUrl = $state("");
  let showApiKey = $state(false);
  let generalSaved = $state(false);
  let modelOpus = $state("");
  let modelSonnet = $state("");
  let modelHaiku = $state("");
  let selectedPlatformId = $state<string | null>(null);
  let platformCredentials = $state<PlatformCredential[]>([]);
  let platformExtraEnv = $state<Array<{ key: string; value: string }>>([]);
  // Track whether user manually edited extra_env (per platform ID).
  // Untouched platforms don't write extra_env, avoiding preset defaults being baked into credentials.
  let extraEnvTouched = $state<Record<string, boolean>>({});

  // CLI Auth state
  let authOverview = $state<import("$lib/types").AuthOverview | null>(null);
  let cliLoginLoading = $state(false);
  let cliLoginError = $state("");

  // Derive merged platform list (static presets + dynamic custom endpoints)
  let platformList = $derived(buildPlatformList(platformCredentials));

  // Group platforms by category for the grid; sort alphabetically within each group,
  // but pin Anthropic first (official default) in the provider group.
  let groupedPlatforms = $derived.by(() => {
    const groups: { id: string; label: string; items: PlatformPreset[] }[] = [];
    for (const cat of PRESET_CATEGORIES) {
      const items = platformList.filter((p) => p.id !== "custom" && p.category === cat.id);
      const sorted = [...items].sort((a, b) => a.name.localeCompare(b.name));
      if (cat.id === "provider") {
        const idx = sorted.findIndex((p) => p.id === "anthropic");
        if (idx > 0) sorted.unshift(sorted.splice(idx, 1)[0]);
      }
      groups.push({ id: cat.id, label: cat.label, items: sorted });
    }
    return groups;
  });

  // Derive selected platform from id (search merged list, not just static presets)
  let selectedPlatform = $derived<PlatformPreset | null>(
    selectedPlatformId ? (platformList.find((p) => p.id === selectedPlatformId) ?? null) : null,
  );

  // Custom endpoint editing state
  // ── Local proxy detection state ──
  let localProxyStatus = $state<import("$lib/types").LocalProxyStatus | null>(null);
  let localProxyChecking = $state(false);
  let localProxyRequestId = $state(0);
  let localAdvancedOpen = $state(false);
  let localProxyStatuses = $state<Record<string, { running: boolean; needsAuth: boolean }>>({});

  // ── API connectivity test state ──
  let apiTestLoading = $state(false);
  let apiTestResult = $state<import("$lib/types").ApiTestResult | null>(null);
  let apiTestRequestId = $state(0);
  // Derive effective auth env var (tracks platformCredentials + selectedPlatformId)
  let effectiveAuthEnvVar = $derived(
    findCredential(platformCredentials, selectedPlatformId ?? "")?.auth_env_var ||
      selectedPlatform?.auth_env_var ||
      "ANTHROPIC_API_KEY",
  );
  // Clear stale test result AND invalidate in-flight requests when any relevant input changes
  $effect(() => {
    void anthropicApiKey;
    void anthropicBaseUrl;
    void modelOpus;
    void modelSonnet;
    void modelHaiku;
    void effectiveAuthEnvVar;
    return () => {
      apiTestResult = null;
      apiTestRequestId++; // invalidate in-flight request
      apiTestLoading = false;
    };
  });

  // ── Web Server state (desktop-only) ──
  let webToken = $state<string | null>(null);
  let webStatus = $state<{
    enabled: boolean;
    running: boolean;
    port: number;
    bind: string;
    warning?: string;
  } | null>(null);
  let showWebToken = $state(false);
  let webTokenCopied = $state(false);
  let webLinkCopied = $state(false);
  let webRestarting = $state(false);
  let webRestartError = $state<string | null>(null);
  let webRestartWarning = $state<string | null>(null);
  let webPortInput = $state("9476");
  let webOriginInput = $state("");
  let webBindValue = $state("127.0.0.1");
  let webOrigins = $state<string[]>([]);
  let webOriginError = $state<string | null>(null);
  let webAdvancedOpen = $state(false);
  let webLanIp = $state<string | null>(null);
  let webTunnelUrl = $state("");
  let webTunnelError = $state<string | null>(null);
  let webTunnelLinkCopied = $state(false);
  let lanIpRequestId = $state(0);

  let debugOn = $state(isDebugMode());
  let logCopied = $state(false);
  let debugFilter = $state(getDebugFilter() || "1");

  // ── UI Zoom state (desktop-only, dynamic import with fallback) ──

  let cachedWebview: any = null;
  async function getWebview() {
    if (!cachedWebview) {
      const { getCurrentWebviewWindow } = await import("@tauri-apps/api/webviewWindow");
      cachedWebview = getCurrentWebviewWindow();
    }
    return cachedWebview;
  }

  let zoomPreview = $state(1.0);

  $effect(() => {
    if (settings) {
      zoomPreview = Math.min(1.5, Math.max(0.75, settings.ui_zoom ?? 1.0));
    }
  });

  function clampZoom(v: number): number | null {
    if (!Number.isFinite(v)) return null;
    return Math.min(1.5, Math.max(0.75, v));
  }

  let pendingZoom: number | null = null;
  let zoomFlying = false;

  async function applyZoomQueued(factor: number) {
    if (zoomFlying) {
      pendingZoom = factor;
      return;
    }

    zoomFlying = true;
    try {
      const wv = await getWebview();
      await wv.setZoom(factor);
      dbg("settings", "applyZoomQueued", { factor });
    } catch (e) {
      dbgWarn("settings", "applyZoomQueued failed", e);
    }
    zoomFlying = false;

    if (pendingZoom !== null) {
      const next = pendingZoom;
      pendingZoom = null;
      void applyZoomQueued(next);
    }
  }

  function previewZoom(raw: number) {
    const factor = clampZoom(raw);
    if (factor === null) return;
    zoomPreview = factor;
  }

  let displaySaved = $state(false);

  async function commitZoom(raw: number) {
    const factor = clampZoom(raw);
    if (factor === null) return;

    // Persist
    try {
      settings = await api.updateUserSettings({ ui_zoom: factor });
      dbg("settings", "commitZoom saved", { factor });
      displaySaved = true;
      setTimeout(() => (displaySaved = false), 1500);
    } catch (e) {
      dbgWarn("settings", "commitZoom save failed", e);
      // Rollback to last persisted value
      const fallback = Math.min(1.5, Math.max(0.75, settings?.ui_zoom ?? 1.0));
      zoomPreview = fallback;
      pendingZoom = null;
      void applyZoomQueued(fallback);
      return;
    }

    // Apply final value via queue (overrides any stale preview)
    pendingZoom = null;
    void applyZoomQueued(factor);
  }
  let logCount = $state(getDebugLogCount());
  let rustCmdCopied = $state(false);
  let currentUsername = $state("");

  // ── Remote host state ──
  let remoteHosts = $state<RemoteHost[]>([]);
  let editingRemote = $state<RemoteHost | null>(null);
  let remoteFormName = $state("");
  let remoteFormHost = $state("");
  let remoteFormUser = $state("");
  let remoteFormPort = $state(22);
  let remoteFormKeyPath = $state("");
  let remoteFormRemoteCwd = $state("");
  let remoteFormClaudePath = $state("");
  let remoteFormForwardKey = $state(false);
  let remoteTesting = $state(false);
  let remoteTestResult = $state<RemoteTestResult | null>(null);
  let remoteSaving = $state(false);
  let remoteSaved = $state(false);

  function resetRemoteForm() {
    editingRemote = null;
    remoteFormName = "";
    remoteFormHost = "";
    remoteFormUser = "";
    remoteFormPort = 22;
    remoteFormKeyPath = "";
    remoteFormRemoteCwd = "";
    remoteFormClaudePath = "";
    remoteFormForwardKey = false;
    remoteTestResult = null;
    remoteFormTouched = false;
  }

  function editRemoteHost(host: RemoteHost) {
    editingRemote = host;
    remoteFormName = host.name;
    remoteFormHost = host.host;
    remoteFormUser = host.user;
    remoteFormPort = host.port;
    remoteFormKeyPath = host.key_path ?? "";
    remoteFormRemoteCwd = host.remote_cwd ?? "";
    remoteFormClaudePath = host.remote_claude_path ?? "";
    remoteFormForwardKey = host.forward_api_key;
    remoteTestResult = null;
  }

  async function saveRemoteHost(keepForm = false) {
    if (!remoteFormName.trim() || !remoteFormHost.trim() || !remoteFormUser.trim()) {
      remoteFormTouched = true;
      return;
    }
    remoteSaving = true;
    try {
      const newHost: RemoteHost = {
        name: remoteFormName.trim(),
        host: remoteFormHost.trim(),
        user: remoteFormUser.trim(),
        port: remoteFormPort || 22,
        key_path: remoteFormKeyPath.trim() || undefined,
        remote_cwd: remoteFormRemoteCwd.trim() || undefined,
        remote_claude_path: remoteFormClaudePath.trim() || undefined,
        forward_api_key: remoteFormForwardKey,
      };

      const updated = editingRemote
        ? remoteHosts.map((h) => (h.name === editingRemote!.name ? newHost : h))
        : [...remoteHosts, newHost];

      await api.updateUserSettings({ remote_hosts: updated } as Partial<UserSettings>);
      remoteHosts = updated;
      if (keepForm) {
        // Switch to edit mode so subsequent saves update instead of duplicate
        editingRemote = newHost;
      } else {
        resetRemoteForm();
      }
      remoteSaved = true;
      setTimeout(() => (remoteSaved = false), 2000);
      dbg("settings", "remote host saved", newHost.name);
    } catch (e) {
      dbgWarn("settings", "save remote host failed", e);
    } finally {
      remoteSaving = false;
    }
  }

  async function deleteRemoteHost(name: string) {
    const updated = remoteHosts.filter((h) => h.name !== name);
    try {
      await api.updateUserSettings({ remote_hosts: updated } as Partial<UserSettings>);
      remoteHosts = updated;
      if (editingRemote?.name === name) resetRemoteForm();
      dbg("settings", "remote host deleted", name);
    } catch (e) {
      dbgWarn("settings", "delete remote host failed", e);
    }
  }

  let remoteFormTouched = $state(false);

  async function testRemoteConnection() {
    if (!remoteFormHost.trim() || !remoteFormUser.trim()) {
      remoteFormTouched = true;
      return;
    }
    remoteTesting = true;
    remoteTestResult = null;
    try {
      remoteTestResult = await api.testRemoteHost(
        remoteFormHost.trim(),
        remoteFormUser.trim(),
        remoteFormPort || undefined,
        remoteFormKeyPath.trim() || undefined,
        remoteFormClaudePath.trim() || undefined,
      );
      dbg("settings", "remote test result", remoteTestResult);
      // Auto-save on successful SSH connection (keep form visible for user to review)
      if (remoteTestResult.ssh_ok && remoteFormName && remoteFormHost && remoteFormUser) {
        await saveRemoteHost(true);
      }
    } catch (e) {
      remoteTestResult = { ssh_ok: false, cli_found: false, error: String(e) };
      dbgWarn("settings", "remote test error", e);
    } finally {
      remoteTesting = false;
    }
  }

  // ── SSH Key wizard state ──
  type SshKeyStep =
    | "idle"
    | "checking"
    | "no_key"
    | "has_key"
    | "pub_missing"
    | "generating"
    | "done"
    | "error";
  let sshKeyStep = $state<SshKeyStep>("idle");
  let sshKeyInfo = $state<SshKeyInfo | null>(null);
  let sshKeyError = $state("");
  let sshCopied = $state(false);
  let sshVerifying = $state(false);
  let wizardKeyPath = $derived(sshKeyInfo?.key_path ?? "");

  function shellQuote(s: string): string {
    return "'" + s.replace(/'/g, "'\\''") + "'";
  }

  function pwshQuote(s: string): string {
    return "'" + s.replace(/'/g, "''") + "'";
  }

  function buildCopyCommand(keyInfo: SshKeyInfo, host: string, user: string, port: number): string {
    if (IS_WINDOWS) {
      const pubPath = pwshQuote(keyInfo.key_path_expanded + ".pub");
      const target = pwshQuote(`${user}@${host}`);
      const remoteScript = pwshQuote(
        "mkdir -p ~/.ssh && chmod 700 ~/.ssh && " +
          "touch ~/.ssh/authorized_keys && chmod 600 ~/.ssh/authorized_keys && " +
          'key=$(cat) && (grep -qxF "$key" ~/.ssh/authorized_keys 2>/dev/null || ' +
          'echo "$key" >> ~/.ssh/authorized_keys)',
      );
      return `Get-Content -LiteralPath ${pubPath} -Raw | ssh -p ${port} ${target} ${remoteScript}`;
    }
    const keyArg = shellQuote(keyInfo.key_path_expanded);
    const pubArg = shellQuote(keyInfo.key_path_expanded + ".pub");
    const target = `${shellQuote(user)}@${shellQuote(host)}`;

    if (keyInfo.ssh_copy_id_available) {
      return `ssh-copy-id -i ${keyArg} -p ${port} ${target}`;
    }
    const remoteScript =
      "mkdir -p ~/.ssh && chmod 700 ~/.ssh && " +
      "touch ~/.ssh/authorized_keys && chmod 600 ~/.ssh/authorized_keys && " +
      'key=$(cat) && (grep -qxF "$key" ~/.ssh/authorized_keys 2>/dev/null || ' +
      'echo "$key" >> ~/.ssh/authorized_keys)';
    return `cat ${pubArg} | ssh -p ${port} ${target} ${shellQuote(remoteScript)}`;
  }

  function buildRebuildPubKeyCommand(keyInfo: SshKeyInfo): string {
    if (IS_WINDOWS) {
      const keyPath = pwshQuote(keyInfo.key_path_expanded);
      const pubPath = pwshQuote(keyInfo.key_path_expanded + ".pub");
      return `ssh-keygen -y -f ${keyPath} | Out-File -Encoding ascii ${pubPath}`;
    }
    const keyArg = shellQuote(keyInfo.key_path_expanded);
    return `ssh-keygen -y -f ${keyArg} > ${shellQuote(keyInfo.key_path_expanded + ".pub")}`;
  }

  async function startSshKeyWizard() {
    sshKeyStep = "checking";
    sshKeyError = "";
    sshCopied = false;
    try {
      const info = await api.checkSshKey();
      sshKeyInfo = info;
      dbg("settings", "ssh key check", info);
      if (info.exists && info.pub_exists) {
        sshKeyStep = "has_key";
      } else if (info.exists && !info.pub_exists) {
        sshKeyStep = "pub_missing";
      } else {
        sshKeyStep = "no_key";
      }
    } catch (e) {
      sshKeyError = String(e);
      sshKeyStep = "error";
      dbgWarn("settings", "ssh key check failed", e);
    }
  }

  async function generateSshKey() {
    sshKeyStep = "generating";
    sshKeyError = "";
    try {
      const info = await api.generateSshKey();
      sshKeyInfo = info;
      sshKeyStep = "has_key";
      dbg("settings", "ssh key generated", info);
    } catch (e) {
      sshKeyError = String(e);
      sshKeyStep = "error";
      dbgWarn("settings", "ssh key generation failed", e);
    }
  }

  async function verifySshConnection() {
    if (!sshKeyInfo || !remoteFormHost || !remoteFormUser) return;
    sshVerifying = true;
    try {
      const result = await api.testRemoteHost(
        remoteFormHost.trim(),
        remoteFormUser.trim(),
        remoteFormPort || undefined,
        wizardKeyPath || undefined,
        remoteFormClaudePath.trim() || undefined,
      );
      dbg("settings", "ssh verify result", result);
      if (result.ssh_ok) {
        remoteFormKeyPath = wizardKeyPath;
        sshKeyStep = "done";
      } else {
        sshKeyError = result.error ?? "";
        sshKeyStep = "has_key"; // stay on has_key so user can retry
      }
      remoteTestResult = result;
    } catch (e) {
      sshKeyError = String(e);
      dbgWarn("settings", "ssh verify failed", e);
    } finally {
      sshVerifying = false;
    }
  }

  function closeSshWizard() {
    sshKeyStep = "idle";
    sshKeyError = "";
    sshCopied = false;
    sshVerifying = false;
  }

  // Keybinding store from layout context
  const keybindingStore = getContext<KeybindingStore>("keybindings");
  let cliSectionOpen = $state(false);
  let codexCliSectionOpen = $state(false);
  let cliSource = $state<"defaults" | "file">("defaults");

  // Keybinding conflict warning for recording editor
  let recordingConflict = $state("");

  // Derived keybinding groups
  let appBindings = $derived(
    keybindingStore.resolved.filter((b) => b.source === "app" && b.editable),
  );
  let fixedBindings = $derived(
    keybindingStore.resolved.filter((b) => b.source === "app" && !b.editable),
  );
  let cliBindings = $derived(keybindingStore.resolved.filter((b) => b.source === "cli"));
  let codexCliBindings = $derived(keybindingStore.resolved.filter((b) => b.source === "codex"));
  let hasOverrides = $derived(keybindingStore.overrides.length > 0);

  function isOverridden(command: string): boolean {
    return keybindingStore.overrides.some((o) => o.command === command);
  }

  function getConflictWarning(key: string, context: string, excludeCmd: string): string {
    const conflict = keybindingStore.findConflict(key, context, excludeCmd);
    return conflict ? t("settings_shortcuts_conflictsWith", { label: conflict.label }) : "";
  }

  // ── Codex status ──
  let codexStatus = $state<CodexAuthResult | null>(null);
  let codexStatusLoading = $state(false);

  async function loadCodexStatus() {
    codexStatusLoading = true;
    try {
      codexStatus = await api.checkCodexAuth();
    } catch {
      codexStatus = null;
    } finally {
      codexStatusLoading = false;
    }
  }

  // ── codex doctor (richer install/config/auth/runtime diagnostics) ──
  let codexDoctor = $state<api.CodexDoctorReport | null>(null);
  let codexDoctorLoading = $state(false);
  let codexDoctorError = $state("");

  async function runCodexDoctor() {
    codexDoctorLoading = true;
    codexDoctorError = "";
    try {
      codexDoctor = await api.runCodexDoctor();
      dbg("settings", "codex doctor", { overall: codexDoctor.overallStatus });
    } catch (e) {
      codexDoctorError = String(e);
      codexDoctor = null;
    } finally {
      codexDoctorLoading = false;
    }
  }

  // Sort doctor checks: problems first (fail → warn → ok), then by id.
  let codexDoctorChecks = $derived.by(() => {
    if (!codexDoctor) return [];
    const rank = (s: string) => (s === "fail" ? 0 : s === "warn" ? 1 : 2);
    return Object.values(codexDoctor.checks).sort(
      (a, b) => rank(a.status) - rank(b.status) || a.id.localeCompare(b.id),
    );
  });

  function doctorStatusClass(status: string): string {
    if (status === "ok") return "bg-emerald-500/10 text-emerald-600 dark:text-emerald-400";
    if (status === "warn") return "bg-amber-500/10 text-amber-600 dark:text-amber-400";
    if (status === "fail") return "bg-red-500/10 text-red-600 dark:text-red-400";
    return "bg-muted text-muted-foreground";
  }

  // ── Codex per-session flags (AgentSettings on codex agent) ──
  let codexAgentSettings = $state<AgentSettings | null>(null);

  async function loadCodexAgentSettings() {
    try {
      codexAgentSettings = await api.getAgentSettings("codex");
      dbg("settings", "codex agent settings loaded", codexAgentSettings);
    } catch (e) {
      dbgWarn("settings", "loadCodexAgentSettings failed", e);
      codexAgentSettings = null;
    }
  }

  async function saveCodexAgentPatch(patch: Partial<AgentSettings>) {
    if (!codexAgentSettings) return;
    try {
      const updated = await api.updateAgentSettings("codex", patch);
      codexAgentSettings = updated;
      dbg("settings", "codex agent settings saved", patch);
    } catch (e) {
      dbgWarn("settings", "saveCodexAgentPatch failed", e);
    }
  }

  // ── Codex third-party provider ──
  // Draft form state for the Codex provider section (committed via saveCodexProvider).
  let codexProviderId = $state<string>(""); // "" = None (plain codex login)
  let codexProviderKey = $state("");
  let codexProviderModel = $state("");
  let codexProviderBaseUrl = $state(""); // editable for "custom"
  // Auth Mode toggle, mirroring Claude: "cli" = codex login (provider None),
  // "app" = app points Codex at a third-party Responses provider. Explicit
  // state (not derived) so "app" can be selected before a preset is picked.
  let codexAuthMode = $state<"cli" | "app">("cli");

  // Initialize the draft from the saved provider once settings load.
  let codexProviderInitDone = false;
  $effect(() => {
    if (!settings || codexProviderInitDone) return;
    codexProviderInitDone = true;
    const cp = settings.codex_provider;
    if (cp) {
      codexProviderId = cp.id;
      codexProviderKey = cp.api_key ?? "";
      codexProviderModel = cp.model ?? "";
      codexProviderBaseUrl = cp.base_url ?? "";
      codexAuthMode = "app";
    }
  });

  function setCodexAuthMode(mode: "cli" | "app") {
    codexAuthMode = mode;
    // CLI Auth clears any app-managed provider (back to codex login default),
    // mirroring Claude's CLI-Auth toggle which clears platform/base_url config.
    if (mode === "cli") selectCodexProvider(null);
  }

  function selectCodexProvider(preset: CodexProviderPreset | null) {
    if (!preset) {
      codexProviderId = "";
      void saveCodexProvider(null);
      return;
    }
    codexProviderId = preset.id;
    codexProviderModel = preset.model;
    codexProviderBaseUrl = preset.base_url;
    if (preset.keyless) codexProviderKey = "";
  }

  async function saveCodexProvider(clear?: null) {
    if (clear === null || !codexProviderId) {
      settings = await api.updateUserSettings({ codex_provider: null } as Partial<UserSettings>);
      return;
    }
    const preset = CODEX_PROVIDER_PRESETS.find((p) => p.id === codexProviderId);
    if (!preset) return;
    const cred: CodexProviderCredential = {
      id: preset.id,
      name: preset.name,
      base_url: codexProviderBaseUrl.trim() || preset.base_url,
      env_key: preset.env_key,
      wire_api: "responses",
      model: codexProviderModel.trim(),
      api_key: preset.keyless ? undefined : codexProviderKey.trim() || undefined,
    };
    settings = await api.updateUserSettings({
      codex_provider: cred,
    } as Partial<UserSettings>);
    dbg("settings", "codex provider saved", { id: cred.id });
  }

  let codexProviderPreset = $derived(
    CODEX_PROVIDER_PRESETS.find((p) => p.id === codexProviderId) ?? null,
  );

  // ── Codex hooks count ──
  let codexHooksCount = $state<number | null>(null);

  async function loadCodexHooksCount() {
    dbg("settings", "loadCodexHooksCount");
    try {
      const { hooks } = await api.getCodexHooks();
      let count = 0;
      for (const groups of Object.values(hooks)) {
        if (Array.isArray(groups)) count += groups.length;
      }
      codexHooksCount = count;
      dbg("settings", "codex hooks loaded", { count: codexHooksCount });
    } catch (e) {
      dbgWarn("settings", "loadCodexHooksCount failed", e);
      codexHooksCount = null;
    }
  }

  async function refreshCodexAll() {
    await loadCodexStatus();
    if (codexStatus?.installed) {
      loadCodexHooksCount();
      loadCodexAgentSettings();
    } else {
      codexHooksCount = null;
      codexAgentSettings = null;
    }
  }

  // ── Codex login ──
  let codexLoginLoading = $state(false);
  let codexLoginError = $state("");

  async function handleCodexLogin() {
    codexLoginLoading = true;
    codexLoginError = "";
    try {
      await api.runCodexLogin();
      await refreshCodexAll();
    } catch (e) {
      codexLoginError = e instanceof Error ? e.message : String(e);
      dbgWarn("settings", "codex login failed", e);
    } finally {
      codexLoginLoading = false;
    }
  }

  // ── CLI Config state ──
  let cliConfig = $state<Record<string, unknown>>({});
  let projectCliConfig = $state<Record<string, unknown>>({});
  let cliConfigLoaded = $state(false);
  let cliConfigLoading = $state(false);
  let cliConfigError = $state("");

  // CLI Config setting definitions
  const CLI_CONFIG_SETTINGS: CliConfigSettingDef[] = [
    // Behavior
    {
      key: "thinkingEnabled",
      label: t("settings_cliConfig_thinkingModeLabel"),
      description: t("settings_cliConfig_thinkingModeDesc"),
      group: "behavior",
      type: "boolean",
      default: true,
    },
    {
      key: "fastMode",
      label: t("settings_cliConfig_fastModeLabel"),
      description: t("settings_cliConfig_fastModeDesc"),
      group: "behavior",
      type: "boolean",
      default: false,
    },
    {
      key: "autoCompactEnabled",
      label: t("settings_cliConfig_autoCompactLabel"),
      description: t("settings_cliConfig_autoCompactDesc"),
      group: "behavior",
      type: "boolean",
      default: true,
    },
    {
      key: "fileCheckpointingEnabled",
      label: t("settings_cliConfig_fileCheckpointsLabel"),
      description: t("settings_cliConfig_fileCheckpointsDesc"),
      group: "behavior",
      type: "boolean",
      default: true,
    },
    {
      key: "respectGitignore",
      label: t("settings_cliConfig_respectGitignoreLabel"),
      description: t("settings_cliConfig_respectGitignoreDesc"),
      group: "behavior",
      type: "boolean",
      default: true,
    },
    {
      key: "verbose",
      label: t("settings_cliConfig_verboseLabel"),
      description: t("settings_cliConfig_verboseDesc"),
      group: "behavior",
      type: "boolean",
      default: false,
    },
    {
      key: "defaultPermissionMode",
      label: t("settings_cliConfig_permissionModeLabel"),
      description: t("settings_cliConfig_permissionModeDesc"),
      group: "behavior",
      type: "enum",
      default: undefined,
      options: [
        { value: "default", label: t("settings_cliConfig_optDefault") },
        { value: "plan", label: t("settings_cliConfig_optPlan") },
        { value: "acceptEdits", label: t("settings_cliConfig_optAutoEdit") },
        { value: "bypassPermissions", label: t("settings_cliConfig_optFullAuto") },
      ],
    },
    {
      key: "teammateMode",
      label: t("settings_cliConfig_teammateModeLabel"),
      description: t("settings_cliConfig_teammateModeDesc"),
      group: "behavior",
      type: "enum",
      default: "auto",
      options: [
        { value: "auto", label: t("settings_cliConfig_optAuto") },
        { value: "always", label: t("settings_cliConfig_optAlways") },
        { value: "never", label: t("settings_cliConfig_optNever") },
      ],
    },
    // Appearance
    {
      key: "theme",
      label: t("settings_cliConfig_cliThemeLabel"),
      description: t("settings_cliConfig_cliThemeDesc"),
      group: "appearance",
      type: "enum",
      default: "dark",
      options: [
        { value: "dark", label: t("settings_cliConfig_optDark") },
        { value: "light", label: t("settings_cliConfig_optLight") },
        { value: "light-high-contrast", label: t("settings_cliConfig_optHighContrast") },
      ],
    },
    {
      key: "prefersReducedMotion",
      label: t("settings_cliConfig_reduceMotionLabel"),
      description: t("settings_cliConfig_reduceMotionDesc"),
      group: "appearance",
      type: "boolean",
      default: false,
    },
    {
      key: "language",
      label: t("settings_cliConfig_responseLangLabel"),
      description: t("settings_cliConfig_responseLangDesc"),
      group: "appearance",
      type: "string",
      default: undefined,
    },
    {
      key: "outputStyle",
      label: t("settings_cliConfig_outputStyleLabel"),
      description: t("settings_cliConfig_outputStyleDesc"),
      group: "appearance",
      type: "string",
      default: undefined,
    },
    // Advanced
    {
      key: "autoConnectIde",
      label: t("settings_cliConfig_autoConnectIdeLabel"),
      description: t("settings_cliConfig_autoConnectIdeDesc"),
      group: "advanced",
      type: "boolean",
      default: false,
    },
    {
      key: "promptSuggestionsEnabled",
      label: t("settings_cliConfig_promptSuggestionsLabel"),
      description: t("settings_cliConfig_promptSuggestionsDesc"),
      group: "advanced",
      type: "boolean",
      default: true,
    },
    {
      key: "spinnerTipsEnabled",
      label: t("settings_cliConfig_spinnerTipsLabel"),
      description: t("settings_cliConfig_spinnerTipsDesc"),
      group: "advanced",
      type: "boolean",
      default: true,
    },
    {
      key: "codeDiffFooterEnabled",
      label: t("settings_cliConfig_codeDiffFooterLabel"),
      description: t("settings_cliConfig_codeDiffFooterDesc"),
      group: "advanced",
      type: "boolean",
      default: true,
    },
    {
      key: "prStatusFooterEnabled",
      label: t("settings_cliConfig_prStatusFooterLabel"),
      description: t("settings_cliConfig_prStatusFooterDesc"),
      group: "advanced",
      type: "boolean",
      default: true,
    },
    {
      key: "autoUpdatesChannel",
      label: t("settings_cliConfig_updateChannelLabel"),
      description: t("settings_cliConfig_updateChannelDesc"),
      group: "advanced",
      type: "enum",
      default: undefined,
      options: [
        { value: "latest", label: t("settings_cliConfig_optLatest") },
        { value: "stable", label: t("settings_cliConfig_optStable") },
      ],
    },
    {
      key: "preferredNotifChannel",
      label: t("settings_cliConfig_notifChannelLabel"),
      description: t("settings_cliConfig_notifChannelDesc"),
      group: "advanced",
      type: "enum",
      default: "auto",
      options: [
        { value: "auto", label: t("settings_cliConfig_optAuto") },
        { value: "iterm2", label: t("settings_cliConfig_optIterm2") },
        { value: "terminal_bell", label: t("settings_cliConfig_optTerminalBell") },
      ],
    },
  ];

  const behaviorSettings = CLI_CONFIG_SETTINGS.filter((s) => s.group === "behavior");
  const appearanceSettings = CLI_CONFIG_SETTINGS.filter((s) => s.group === "appearance");
  const advancedSettings = CLI_CONFIG_SETTINGS.filter((s) => s.group === "advanced");

  // ── Codex Config state ──
  let codexConfig = $state<Record<string, unknown>>({});
  let projectCodexConfig = $state<Record<string, unknown>>({});
  let codexConfigWarning = $state<string | undefined>(undefined);

  // Codex Config setting definitions
  const CODEX_CONFIG_SETTINGS: CliConfigSettingDef[] = [
    {
      key: "model",
      label: t("settings_codexConfig_modelLabel"),
      description: t("settings_codexConfig_modelDesc"),
      group: "behavior",
      type: "string",
      default: undefined,
    },
    {
      key: "model_reasoning_effort",
      label: t("settings_codexConfig_reasoningEffortLabel"),
      description: t("settings_codexConfig_reasoningEffortDesc"),
      group: "behavior",
      type: "enum",
      default: "medium",
      options: [
        { value: "none", label: t("settings_codexConfig_optNone") },
        { value: "minimal", label: t("settings_codexConfig_optMinimal") },
        { value: "low", label: t("settings_codexConfig_optLow") },
        { value: "medium", label: t("settings_codexConfig_optMedium") },
        { value: "high", label: t("settings_codexConfig_optHigh") },
        { value: "xhigh", label: t("settings_codexConfig_optXHigh") },
      ],
    },
    {
      key: "model_reasoning_summary",
      label: t("settings_codexConfig_reasoningSummaryLabel"),
      description: t("settings_codexConfig_reasoningSummaryDesc"),
      group: "behavior",
      type: "enum",
      default: "auto",
      options: [
        { value: "none", label: t("settings_codexConfig_optNone") },
        { value: "auto", label: t("settings_codexConfig_optAuto") },
        { value: "concise", label: t("settings_codexConfig_optConcise") },
        { value: "detailed", label: t("settings_codexConfig_optDetailed") },
      ],
    },
    {
      key: "model_verbosity",
      label: t("settings_codexConfig_verbosityLabel"),
      description: t("settings_codexConfig_verbosityDesc"),
      group: "behavior",
      type: "enum",
      default: "medium",
      options: [
        { value: "low", label: t("settings_codexConfig_optLow") },
        { value: "medium", label: t("settings_codexConfig_optMedium") },
        { value: "high", label: t("settings_codexConfig_optHigh") },
      ],
    },
    {
      key: "personality",
      label: t("settings_codexConfig_personalityLabel"),
      description: t("settings_codexConfig_personalityDesc"),
      group: "behavior",
      type: "string",
      default: undefined,
    },
    {
      key: "approval_policy",
      label: t("settings_codexConfig_approvalPolicyLabel"),
      description: t("settings_codexConfig_approvalPolicyDesc"),
      group: "behavior",
      type: "enum",
      default: "on-request",
      options: [
        { value: "on-request", label: t("settings_codexConfig_optOnRequest") },
        { value: "untrusted", label: t("settings_codexConfig_optUntrusted") },
        { value: "on-failure", label: t("settings_codexConfig_optOnFailure") },
        { value: "never", label: t("settings_codexConfig_optNever") },
      ],
    },
    {
      key: "sandbox_mode",
      label: t("settings_codexConfig_sandboxLabel"),
      description: t("settings_codexConfig_sandboxDesc"),
      group: "behavior",
      type: "enum",
      default: "read-only",
      options: [
        { value: "read-only", label: t("settings_codexConfig_optReadOnly") },
        { value: "workspace-write", label: t("settings_codexConfig_optWorkspaceWrite") },
        { value: "danger-full-access", label: t("settings_codexConfig_optFullAccess") },
      ],
    },
    {
      key: "web_search",
      label: t("settings_codexConfig_webSearchLabel"),
      description: t("settings_codexConfig_webSearchDesc"),
      group: "behavior",
      type: "enum",
      // Durable tri-state web search mode. The `cached` value is currently
      // unreachable via the per-session --search boolean (which maps to live);
      // this config key is the only way to select it.
      default: "disabled",
      options: [
        { value: "disabled", label: t("settings_codexConfig_optDisabled") },
        { value: "cached", label: t("settings_codexConfig_optCached") },
        { value: "live", label: t("settings_codexConfig_optLive") },
      ],
    },
  ];

  function getCodexConfigValue(key: string, def: CliConfigSettingDef): unknown {
    return key in codexConfig ? codexConfig[key] : def.default;
  }

  function isCodexProjectOverride(key: string): boolean {
    return key in projectCodexConfig;
  }

  /** Render display for unknown/unrecognized values */
  function codexValueDisplay(key: string, def: CliConfigSettingDef): string | null {
    const val = codexConfig[key];
    if (val === undefined || val === null) return null;
    // If it's a table/object → custom table value
    if (typeof val === "object" && !Array.isArray(val)) {
      return t("settings_codexConfig_customTable");
    }
    // If it's an enum type and value doesn't match known options
    if (def.type === "enum" && def.options) {
      const known = def.options.some((o) => o.value === val);
      if (!known && typeof val === "string") {
        return t("settings_codexConfig_unrecognized", { value: val });
      }
    }
    return null;
  }

  async function saveCodexConfigPatch(key: string, value: unknown) {
    dbg("settings", "saveCodexConfigPatch", { key, value });
    try {
      codexConfig = await api.updateCodexConfig({ [key]: value ?? null });
    } catch (e) {
      dbgWarn("settings", "saveCodexConfigPatch error", e);
    }
  }

  function getCliConfigValue(key: string, def: CliConfigSettingDef): unknown {
    return key in cliConfig ? cliConfig[key] : def.default;
  }

  function isProjectOverride(key: string): boolean {
    return key in projectCliConfig;
  }

  async function saveCliConfigPatch(key: string, value: unknown) {
    dbg("settings", "saveCliConfigPatch", { key, value });
    try {
      // null value = delete key (restore CLI default)
      cliConfig = await api.updateCliConfig({ [key]: value ?? null });
    } catch (e) {
      dbgWarn("settings", "saveCliConfigPatch error", e);
    }
  }

  async function loadCliConfig() {
    if (cliConfigLoading) return;
    cliConfigLoading = true;
    cliConfigError = "";
    try {
      const cwd = localStorage.getItem("ocv:project-cwd") || "";
      // Load Claude + Codex configs in parallel
      const [claudeConfig, codexResult, projectClaude, projectCodex] = await Promise.all([
        api.getCliConfig(),
        api.getCodexConfig(),
        cwd ? api.getProjectCliConfig(cwd) : Promise.resolve({}),
        cwd ? api.getProjectCodexConfig(cwd) : Promise.resolve({}),
      ]);

      cliConfig = claudeConfig;
      projectCliConfig = projectClaude;

      // Codex config with warning support
      codexConfig = codexResult.config ?? {};
      codexConfigWarning = codexResult.warning;
      projectCodexConfig = projectCodex;

      cliConfigLoaded = true;
      dbg("settings", "cliConfig loaded", {
        keys: Object.keys(cliConfig).length,
        projectKeys: Object.keys(projectCliConfig).length,
        codexKeys: Object.keys(codexConfig).length,
        codexWarning: codexConfigWarning,
      });
    } catch (e) {
      cliConfigError = String(e);
      dbgWarn("settings", "loadCliConfig error", e);
    } finally {
      cliConfigLoading = false;
    }
  }

  // Lazy load CLI config when tab activates
  $effect(() => {
    if (activeTab === "cli-config" && !cliConfigLoaded && !cliConfigLoading) {
      loadCliConfig();
    }
  });

  // Refresh log count periodically when debug is on
  $effect(() => {
    if (!debugOn) return;
    const timer = setInterval(() => {
      logCount = getDebugLogCount();
    }, 2000);
    return () => clearInterval(timer);
  });

  function detectPlatformFromUrl(url: string, activePlatformId?: string): string | null {
    // If we have a stored active_platform_id, prefer it
    if (activePlatformId) return activePlatformId;
    if (!url) return null;
    const match = PLATFORM_PRESETS.find((p) => p.base_url && url === p.base_url);
    return match?.id ?? "custom";
  }

  /** Load display fields (key + URL) from credential store for a given platform. */
  function loadFieldsFromCredential(platformId: string | null) {
    apiTestResult = null;
    if (!platformId) {
      anthropicApiKey = "";
      anthropicBaseUrl = "";
      platformExtraEnv = [];
      return;
    }
    const cred = findCredential(platformCredentials, platformId);
    const preset = PLATFORM_PRESETS.find((p) => p.id === platformId);
    anthropicApiKey = cred?.api_key ?? "";
    // base_url: credential override > preset default > empty
    anthropicBaseUrl = cred?.base_url ?? preset?.base_url ?? "";
    // models: credential override > preset default > expand to 3 tiers
    const models = cred?.models ?? preset?.models;
    const [o, s, h] = expandModelsToTiers(models);
    modelOpus = o;
    modelSonnet = s;
    modelHaiku = h;
    // extra_env: credential explicit value (including {}) takes priority; undefined falls back to preset
    const extraEnv = cred?.extra_env !== undefined ? cred.extra_env : (preset?.extra_env ?? {});
    platformExtraEnv = Object.entries(extraEnv).map(([key, value]) => ({ key, value }));
    // Don't set touched on load — touched is only driven by UI edit actions (onblur/delete row)
    dbg("settings", "loadFieldsFromCredential", {
      platformId,
      hasKey: !!anthropicApiKey,
      url: anthropicBaseUrl,
      models: [modelOpus, modelSonnet, modelHaiku],
      extraEnvKeys: Object.keys(extraEnv),
      extraEnvSource: cred?.extra_env !== undefined ? "credential" : "preset",
    });
  }

  /** Save current editing fields into the credentials array. */
  function saveCurrentToCredential() {
    if (!selectedPlatformId) return;
    const preset = PLATFORM_PRESETS.find((p) => p.id === selectedPlatformId);
    // Compress 3 tier inputs → models array; undefined when all empty (→ backend preset fallback).
    // Do NOT fall back to preset?.models here — undefined means "use provider defaults",
    // and baking preset values into credential would prevent future preset updates from taking effect.
    const modelsToSave = compressModelsFromTiers(modelOpus, modelSonnet, modelHaiku);

    // Convert extra_env array back to Record, filter empty keys, warn on duplicates
    const extraEnvRecord: Record<string, string> = {};
    const seenKeys = new Set<string>();
    for (const { key, value } of platformExtraEnv) {
      const k = key.trim();
      if (!k) continue;
      if (seenKeys.has(k)) {
        dbgWarn("settings", `duplicate extra_env key "${k}" — last value wins`);
      }
      seenKeys.add(k);
      extraEnvRecord[k] = value;
    }

    // Only write extra_env when user has touched it; otherwise preserve credential's original value
    const extraEnvToSave = extraEnvTouched[selectedPlatformId]
      ? extraEnvRecord // always write (even empty {}), distinct from undefined
      : undefined; // don't overwrite — keep credential as-is (may be undefined or old value)

    dbg("settings", "saveCurrentToCredential: extra_env", {
      platform: selectedPlatformId,
      touched: !!extraEnvTouched[selectedPlatformId],
      keys: Object.keys(extraEnvRecord),
    });

    _upsertCredential(selectedPlatformId, {
      api_key: anthropicApiKey || undefined,
      // Always save base_url — backend needs it for ANTHROPIC_BASE_URL injection
      base_url: anthropicBaseUrl || preset?.base_url || undefined,
      auth_env_var: selectedPlatform?.auth_env_var ?? preset?.auth_env_var,
      models: modelsToSave,
      ...(extraEnvToSave !== undefined ? { extra_env: extraEnvToSave } : {}),
    });
  }

  /** Sync global fields from current display state and persist everything. */
  function syncAndSave(platformId: string) {
    const preset = PLATFORM_PRESETS.find((p) => p.id === platformId);
    saveGeneralPatch({
      anthropic_api_key: anthropicApiKey || undefined,
      anthropic_base_url: anthropicBaseUrl || undefined,
      auth_env_var: preset?.auth_env_var,
      active_platform_id: platformId,
      platform_credentials: platformCredentials,
    });
  }

  function markExtraEnvTouched() {
    if (selectedPlatformId) extraEnvTouched[selectedPlatformId] = true;
  }

  /**
   * Parse pasted env text. Supported formats:
   * - KEY=value lines (with optional `export` prefix, # comments, quoted values)
   * - JSON object: { "KEY": "value", ... }
   */
  function parseEnvText(text: string): Array<{ key: string; value: string }> {
    const trimmed = text.trim();
    // Try JSON object first
    if (trimmed.startsWith("{")) {
      try {
        const obj = JSON.parse(trimmed);
        if (obj && typeof obj === "object" && !Array.isArray(obj)) {
          const results: Array<{ key: string; value: string }> = [];
          for (const [key, val] of Object.entries(obj)) {
            if (/^[A-Za-z_][A-Za-z0-9_]*$/.test(key)) {
              results.push({ key, value: String(val) });
            }
          }
          if (results.length > 0) return results;
        }
      } catch {
        // Not valid JSON, fall through to line-based parsing
      }
    }
    // Line-based: KEY=value, export KEY=value, # comments
    const results: Array<{ key: string; value: string }> = [];
    for (const raw of trimmed.split(/\r?\n/)) {
      const line = raw.trim();
      if (!line || line.startsWith("#")) continue;
      const stripped = line.replace(/^export\s+/, "");
      const eqIdx = stripped.indexOf("=");
      if (eqIdx <= 0) continue;
      const key = stripped.slice(0, eqIdx).trim();
      let value = stripped.slice(eqIdx + 1).trim();
      if (
        (value.startsWith('"') && value.endsWith('"')) ||
        (value.startsWith("'") && value.endsWith("'"))
      ) {
        value = value.slice(1, -1);
      }
      if (/^[A-Za-z_][A-Za-z0-9_]*$/.test(key)) {
        results.push({ key, value });
      }
    }
    return results;
  }

  /** Handle paste on env key input: if content looks like KEY=value lines, bulk-add them. */
  function handleEnvKeyPaste(e: ClipboardEvent, index: number) {
    const text = e.clipboardData?.getData("text/plain") ?? "";
    const parsed = parseEnvText(text);
    if (parsed.length === 0) return; // not env format, let normal paste through
    e.preventDefault();
    // Replace current (likely empty) row with first parsed entry, append rest
    const before = platformExtraEnv.slice(0, index);
    const after = platformExtraEnv.slice(index + 1);
    platformExtraEnv = [...before, ...parsed, ...after];
    markExtraEnvTouched();
    persistCurrentPlatform();
    dbg("settings", "env paste parsed", { count: parsed.length, keys: parsed.map((p) => p.key) });
  }

  /** Unified persist: save current platform fields to credential + sync to settings. */
  function persistCurrentPlatform() {
    saveCurrentToCredential();
    if (selectedPlatformId) syncAndSave(selectedPlatformId);
  }

  // ── Local proxy detection ──

  async function checkLocalProxy() {
    if (!selectedPlatform || selectedPlatform.category !== "local" || !selectedPlatformId) return;
    localProxyChecking = true;
    localProxyStatus = null;
    const myRequestId = ++localProxyRequestId;
    const myPlatformId = selectedPlatformId;
    const urlToCheck = anthropicBaseUrl;
    dbg("settings", "checkLocalProxy start", {
      id: myPlatformId,
      url: urlToCheck,
      reqId: myRequestId,
    });
    try {
      const result = await api.detectLocalProxy(myPlatformId, urlToCheck);
      if (myRequestId !== localProxyRequestId) return;
      if (myPlatformId !== selectedPlatformId) return;
      localProxyStatus = result;
      localProxyStatuses = {
        ...localProxyStatuses,
        [myPlatformId]: { running: result.running, needsAuth: result.needsAuth },
      };
      dbg("settings", "checkLocalProxy result", result);
    } catch (e) {
      if (myRequestId !== localProxyRequestId || myPlatformId !== selectedPlatformId) return;
      localProxyStatus = {
        proxyId: myPlatformId,
        running: false,
        needsAuth: false,
        baseUrl: urlToCheck,
        error: String(e),
      };
      localProxyStatuses = {
        ...localProxyStatuses,
        [myPlatformId]: { running: false, needsAuth: false },
      };
      dbgWarn("settings", "checkLocalProxy error", e);
    } finally {
      if (myRequestId === localProxyRequestId) localProxyChecking = false;
    }
  }

  async function checkAllLocalProxies() {
    const localPresets = PLATFORM_PRESETS.filter((p) => p.category === "local");
    const results = await Promise.allSettled(
      localPresets.map((p) => {
        const cred = findCredential(platformCredentials, p.id);
        const url = cred?.base_url || p.base_url;
        return api.detectLocalProxy(p.id, url);
      }),
    );
    const statuses: Record<string, { running: boolean; needsAuth: boolean }> = {};
    results.forEach((r, i) => {
      if (r.status === "fulfilled") {
        statuses[localPresets[i].id] = { running: r.value.running, needsAuth: r.value.needsAuth };
      } else {
        statuses[localPresets[i].id] = { running: false, needsAuth: false };
      }
    });
    localProxyStatuses = statuses;
    dbg("settings", "checkAllLocalProxies", statuses);
  }

  function applyPlatformPreset(preset: PlatformPreset) {
    // 1. Save current platform's data to credentials (if modified)
    saveCurrentToCredential();
    // 2. Switch to new platform
    selectedPlatformId = preset.id;
    localAdvancedOpen = false;
    localProxyStatus = null;
    // 3. Load new platform's data from credentials
    loadFieldsFromCredential(preset.id);
    // 4. Sync global fields + persist
    syncAndSave(preset.id);
    // 5. Auto-detect if local proxy
    if (preset.category === "local") {
      checkLocalProxy();
    }
  }

  /** Upsert a credential in the local platformCredentials array. */
  function _upsertCredential(platformId: string, fields: Partial<PlatformCredential>) {
    const idx = platformCredentials.findIndex((c) => c.platform_id === platformId);
    if (idx >= 0) {
      platformCredentials[idx] = { ...platformCredentials[idx], ...fields };
    } else {
      platformCredentials = [...platformCredentials, { platform_id: platformId, ...fields }];
    }
  }

  /** Add a new custom endpoint — creates with defaults and immediately selects it. */
  function addCustomEndpoint() {
    const id = `custom-${Date.now()}`;
    const cred: PlatformCredential = {
      platform_id: id,
      name: "Custom",
      base_url: "",
      auth_env_var: "ANTHROPIC_AUTH_TOKEN",
    };
    platformCredentials = [...platformCredentials, cred];
    saveGeneralPatch({ platform_credentials: platformCredentials });
    // Select the newly created endpoint — opens full config form below
    const preset = buildPlatformList(platformCredentials).find((p) => p.id === id);
    if (preset) applyPlatformPreset(preset);
  }

  /** Delete a custom endpoint. */
  function deleteCustomEndpoint(platformId: string) {
    // Clear selection first so applyPlatformPreset won't re-save the deleted credential
    const wasActive = selectedPlatformId === platformId;
    if (wasActive) selectedPlatformId = null;
    platformCredentials = platformCredentials.filter((c) => c.platform_id !== platformId);
    saveGeneralPatch({ platform_credentials: platformCredentials });
    // If we deleted the active platform, switch to Anthropic
    if (wasActive) {
      const anthropic = PLATFORM_PRESETS.find((p) => p.id === "anthropic")!;
      applyPlatformPreset(anthropic);
    }
  }

  function openSetupWizard() {
    window.dispatchEvent(new CustomEvent("ocv:show-wizard"));
  }

  onMount(async () => {
    try {
      settings = await api.getUserSettings();
      authMode = settings.auth_mode ?? "cli";
      remoteHosts = settings.remote_hosts ?? [];
      platformCredentials = settings.platform_credentials ?? [];
      // Load display fields from credentials (not global fields)
      if (authMode === "api") {
        selectedPlatformId = detectPlatformFromUrl(
          settings.anthropic_base_url ?? "",
          settings.active_platform_id,
        );
        loadFieldsFromCredential(selectedPlatformId);
      } else {
        anthropicApiKey = settings.anthropic_api_key ?? "";
        anthropicBaseUrl = settings.anthropic_base_url ?? "";
      }
    } catch (e) {
      dbgWarn("settings", "error", e);
    }
    // Load Codex status + hooks + per-session agent settings
    loadCodexStatus().then(() => {
      if (codexStatus?.installed) {
        loadCodexHooksCount();
        loadCodexAgentSettings();
      }
    });
    // Load auth overview
    api
      .getAuthOverview()
      .then((ov) => (authOverview = ov))
      .catch((e) => {
        dbgWarn("settings", "failed to load auth overview", e);
      });
    // Load web server status + token (desktop only)
    if (getTransport().isDesktop()) {
      Promise.all([api.getWebServerStatus(), api.getWebServerToken()])
        .then(async ([status, token]) => {
          webStatus = status;
          webToken = token;
          // Initialize form fields from settings
          webPortInput = String(settings?.web_server_port ?? 9476);
          webBindValue = settings?.web_server_bind ?? "127.0.0.1";
          webOrigins = [...(settings?.web_server_allowed_origins ?? [])];
          webTunnelUrl = settings?.web_server_tunnel_url ?? "";
          dbg("settings", "webServer loaded", {
            enabled: status?.enabled,
            hasToken: !!token,
            tunnel: webTunnelUrl,
          });
          if (status?.running) await refreshLanIp(status.bind);
        })
        .catch((e) => {
          dbgWarn("settings", "webServer load failed", e);
        });
    }
    loadCliInfo();
    // Auto-detect local proxies
    checkAllLocalProxies();
    if (selectedPlatform?.category === "local") {
      checkLocalProxy();
    }
    // Detect current username + CLI keybindings source
    import("@tauri-apps/api/path")
      .then(async (p) => {
        const home = await p.homeDir();
        const parts = splitPath(home.replace(/[/\\]+$/, ""));
        currentUsername = parts[parts.length - 1] || "";
        const absPath = await p.join(home, ".claude", "keybindings.json");
        return api.readTextFile(absPath);
      })
      .then(() => {
        cliSource = "file";
      })
      .catch(() => {
        cliSource = "defaults";
      });
  });

  // Cross-page sync: when in-chat /login or /logout finishes, the chat page
  // dispatches `ocv:codex-auth-changed`. Refresh Codex status here so this
  // Settings tab doesn't show stale auth state. Strictly one-way
  // (chat → settings) to avoid double-refresh loops with handleCodexLogin.
  onMount(() => {
    const handler = () => {
      void refreshCodexAll();
    };
    window.addEventListener("ocv:codex-auth-changed", handler);
    return () => window.removeEventListener("ocv:codex-auth-changed", handler);
  });

  async function saveGeneralPatch(patch: Record<string, unknown>) {
    dbg("settings", "saveGeneralPatch", redactSensitive(patch));
    try {
      settings = await api.updateUserSettings(patch as Partial<UserSettings>);
      generalSaved = true;
      setTimeout(() => (generalSaved = false), 1500);
    } catch (e) {
      dbgWarn("settings", "saveGeneralPatch error", e);
    }
  }

  // ── Web Server helpers ──

  async function applyWebServerSettings() {
    webRestarting = true;
    webRestartError = null;
    webRestartWarning = null;
    webTunnelError = null;
    try {
      const portNum = parseInt(webPortInput, 10);
      if (isNaN(portNum) || portNum < 1024 || portNum > 65535) {
        throw new Error(t("settings_general_webPortInvalid"));
      }
      const result = await api.restartWebServer({
        enabled: true,
        port: portNum,
        bind: webBindValue,
        allowed_origins: webOrigins.length > 0 ? webOrigins : null,
        tunnel_url: webTunnelUrl.trim() || null,
      });
      webStatus = await api.getWebServerStatus();
      settings = await api.getUserSettings();
      if (!result.config_saved) {
        webRestartWarning = t("settings_general_webSaveWarning");
      }
      dbg("settings", "webServer apply", { started: result.started, saved: result.config_saved });
      if (webStatus?.running) await refreshLanIp(webStatus.bind);
    } catch (e: unknown) {
      webRestartError = (e as Error)?.message ?? String(e);
      webStatus = await api.getWebServerStatus();
      dbgWarn("settings", "webServer apply failed", e);
    } finally {
      webRestarting = false;
    }
  }

  function addWebOrigin() {
    const trimmed = webOriginInput.trim().replace(/\/+$/, "");
    if (!trimmed) return;
    try {
      const url = new URL(trimmed);
      if (url.protocol !== "http:" && url.protocol !== "https:") {
        webOriginError = t("settings_general_webOriginInvalid");
        return;
      }
      const origin = url.origin;
      if (!webOrigins.includes(origin)) {
        webOrigins = [...webOrigins, origin];
      }
    } catch {
      webOriginError = t("settings_general_webOriginInvalid");
      return;
    }
    webOriginInput = "";
    webOriginError = null;
  }

  async function refreshLanIp(bind: string): Promise<string | null> {
    const myId = ++lanIpRequestId;
    if (bind !== "0.0.0.0" && bind !== "::" && bind !== "[::]") {
      webLanIp = null;
      return null;
    }
    try {
      const preferV6 = bind === "::" || bind === "[::]";
      const ip = await api.getLocalIp(preferV6);
      if (myId !== lanIpRequestId) return webLanIp;
      webLanIp = ip;
      return ip;
    } catch (e) {
      dbgWarn("settings", "refreshLanIp failed", e);
      if (myId !== lanIpRequestId) return webLanIp;
      webLanIp = null;
      return null;
    }
  }

  function buildLocalAccessUrl(): string | null {
    if (!webStatus?.running || !webToken) return null;
    const bind = webStatus.bind;
    const isAll = bind === "0.0.0.0" || bind === "::" || bind === "[::]";
    const rawHost = isAll ? webLanIp : bind;
    if (!rawHost) return null;
    const host = rawHost.includes(":") ? `[${rawHost}]` : rawHost;
    return `http://${host}:${webStatus.port}/login#token=${webToken}`;
  }

  function buildTunnelAccessUrl(): string | null {
    if (!webStatus?.running || !webToken) return null;
    // Use saved (applied) tunnel URL, not the draft input value
    const tunnel = settings?.web_server_tunnel_url?.trim();
    if (!tunnel) return null;
    try {
      const u = new URL(tunnel);
      // Tunnel links use ?token= (server-side auth) to survive ngrok/cloudflared
      // interstitial pages. Local links keep #token= (fragment, never sent to server).
      return `${u.origin}/login?token=${webToken}`;
    } catch {
      return null;
    }
  }

  function buildAccessUrl(): string | null {
    return buildTunnelAccessUrl() ?? buildLocalAccessUrl();
  }

  async function copyAccessLink() {
    const url = buildAccessUrl();
    if (!url) return;
    await navigator.clipboard.writeText(url);
    webLinkCopied = true;
    dbg("settings", "webLink copied");
    setTimeout(() => (webLinkCopied = false), 1500);
  }

  async function openAccessLink() {
    const url = buildAccessUrl();
    if (!url) return;
    try {
      const { open } = await import("@tauri-apps/plugin-shell");
      await open(url);
      dbg("settings", "webLink opened in browser");
    } catch (e) {
      dbgWarn("settings", "failed to open browser", e);
    }
  }
</script>

{#key currentLocale()}
  <div class="max-w-4xl mx-auto p-6 animate-slide-up">
    <h1 class="text-2xl font-bold mb-5">{t("settings_title")}</h1>

    <!-- Tab bar -->
    <div class="flex gap-1 border-b border-border mb-6">
      {#each tabs as tab (tab.id)}
        <button
          class="flex items-center gap-1.5 px-4 py-2.5 text-sm font-medium transition-colors relative
          {activeTab === tab.id
            ? 'text-foreground'
            : 'text-muted-foreground hover:text-foreground/80'}"
          onclick={() => (activeTab = tab.id)}
        >
          <svg
            class="h-3.5 w-3.5"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            stroke-linejoin="round"
          >
            <path d={tab.icon} />
          </svg>
          {tabLabels[tab.id]()}
          {#if activeTab === tab.id}
            <span class="absolute bottom-0 left-2 right-2 h-0.5 bg-primary rounded-full"></span>
          {/if}
        </button>
      {/each}
    </div>

    <!-- ═══ General tab ═══ -->
    {#if activeTab === "general"}
      <div class="space-y-6">
        <!-- Default Agent Card -->
        <Card class="p-6 space-y-4">
          <h2 class="text-sm font-semibold text-muted-foreground uppercase tracking-wider">
            {t("settings_general_defaultAgent")}
          </h2>
          <div class="flex items-center justify-between gap-4">
            <div>
              <p class="text-sm font-medium">{t("settings_general_defaultAgentLabel")}</p>
              <p class="text-xs text-muted-foreground">
                {t("settings_general_defaultAgentDesc")}
              </p>
            </div>
            <div class="flex gap-1.5">
              {#each ["claude", "codex"] as ag (ag)}
                <button
                  class="rounded-md border px-3 py-1.5 text-xs capitalize transition-all duration-150
                  {(settings?.default_agent ?? 'claude') === ag
                    ? 'bg-primary text-primary-foreground'
                    : 'hover:bg-accent'}"
                  onclick={() => saveGeneralPatch({ default_agent: ag })}
                >
                  {ag}
                </button>
              {/each}
            </div>
          </div>
        </Card>

        <!-- Language Card -->
        <Card class="p-6 space-y-4">
          <h2 class="text-sm font-semibold text-muted-foreground uppercase tracking-wider">
            {t("settings_general_language")}
          </h2>
          <div class="flex items-center justify-between">
            <div>
              <p class="text-sm font-medium">{t("settings_general_displayLanguage")}</p>
              <p class="text-xs text-muted-foreground">
                {t("settings_general_displayLanguageDesc")}
              </p>
            </div>
            <div class="flex gap-1.5">
              {#each LOCALE_REGISTRY as entry}
                <button
                  class="rounded-md border px-3 py-1.5 text-xs transition-all duration-150
                  {currentLocale() === entry.code
                    ? 'bg-primary text-primary-foreground'
                    : (entry.status as string) === 'beta'
                      ? 'border-muted-foreground/30 text-muted-foreground hover:bg-accent'
                      : 'hover:bg-accent'}"
                  onclick={() => switchLocale(entry.code)}
                >
                  {entry.nativeName}{#if (entry.status as string) === "beta"}<span
                      class="ml-1 text-[10px] opacity-60">(Beta)</span
                    >{/if}
                </button>
              {/each}
            </div>
          </div>
        </Card>

        <!-- Display Card -->
        <Card class="p-6 space-y-4">
          <div class="flex items-center justify-between">
            <h2 class="text-sm font-semibold text-muted-foreground uppercase tracking-wider">
              {t("settings_general_display")}
            </h2>
            {#if displaySaved}
              <span class="text-xs text-emerald-500 flex items-center gap-1 animate-fade-in">
                <svg
                  class="h-3 w-3"
                  viewBox="0 0 24 24"
                  fill="none"
                  stroke="currentColor"
                  stroke-width="2"
                  stroke-linecap="round"
                  stroke-linejoin="round"><path d="M20 6 9 17l-5-5" /></svg
                >
                {t("settings_general_saved")}
              </span>
            {/if}
          </div>
          <div class="flex items-center justify-between gap-4">
            <div>
              <p class="text-sm font-medium">{t("settings_general_uiZoom")}</p>
              <p class="text-xs text-muted-foreground">{t("settings_general_uiZoomDesc")}</p>
            </div>
            <div class="flex items-center gap-3">
              <input
                type="range"
                min="0.75"
                max="1.5"
                step="0.05"
                value={zoomPreview}
                class="w-28 accent-primary"
                oninput={(e) => previewZoom(parseFloat((e.target as HTMLInputElement).value))}
                onchange={(e) => commitZoom(parseFloat((e.target as HTMLInputElement).value))}
              />
              <span class="text-xs text-muted-foreground w-10 text-right">
                {Math.round(zoomPreview * 100)}%
              </span>
            </div>
          </div>
        </Card>

        <!-- Web Server Card (desktop only) -->
        {#if getTransport().isDesktop()}
          <Card class="p-6 space-y-4">
            <h2 class="text-sm font-semibold text-muted-foreground uppercase tracking-wider">
              {t("settings_general_webServer")}
            </h2>

            <!-- Enabled toggle -->
            <div class="flex items-center justify-between">
              <div>
                <p class="text-sm font-medium">{t("settings_general_webEnabled")}</p>
                <p class="text-xs text-muted-foreground">{t("settings_general_webEnabledDesc")}</p>
              </div>
              <button
                class="relative inline-flex h-6 w-11 items-center rounded-full transition-colors {webStatus?.enabled
                  ? 'bg-primary'
                  : 'bg-muted'}"
                disabled={webRestarting}
                onclick={async () => {
                  const newEnabled = !webStatus?.enabled;
                  webRestarting = true;
                  webRestartError = null;
                  webRestartWarning = null;
                  try {
                    if (newEnabled) {
                      const portNum = parseInt(webPortInput, 10);
                      if (isNaN(portNum) || portNum < 1024 || portNum > 65535) {
                        throw new Error(t("settings_general_webPortInvalid"));
                      }
                      const result = await api.restartWebServer({
                        enabled: true,
                        port: portNum,
                        bind: webBindValue,
                        allowed_origins: webOrigins.length > 0 ? webOrigins : null,
                        tunnel_url: webTunnelUrl.trim() || null,
                      });
                      if (!result.config_saved) {
                        webRestartWarning = t("settings_general_webSaveWarning");
                      }
                    } else {
                      await api.restartWebServer({
                        enabled: false,
                        port: 0,
                        bind: "",
                        allowed_origins: null,
                        tunnel_url: null,
                      });
                    }
                    webStatus = await api.getWebServerStatus();
                    settings = await api.getUserSettings();
                    dbg("settings", "webServer toggled", { enabled: newEnabled });
                    if (webStatus?.running) await refreshLanIp(webStatus.bind);
                  } catch (e) {
                    webRestartError = (e as Error)?.message ?? String(e);
                    webStatus = await api.getWebServerStatus();
                    dbgWarn("settings", "webServer toggle failed", e);
                  } finally {
                    webRestarting = false;
                  }
                }}
              >
                <span
                  class="inline-block h-4 w-4 transform rounded-full bg-white transition-transform {webStatus?.enabled
                    ? 'translate-x-6'
                    : 'translate-x-1'}"
                ></span>
              </button>
            </div>

            <!-- Config area (show when enabled OR running) -->
            {#if webStatus?.enabled || webStatus?.running}
              <!-- Startup warning banner -->
              {#if webStatus?.warning}
                <div class="rounded-md border border-amber-500/30 bg-amber-500/5 px-3 py-2">
                  <p class="text-xs text-amber-400 whitespace-pre-line">
                    {t("settings_general_webStartupWarning", { warning: webStatus.warning })}
                  </p>
                </div>
              {/if}

              <!-- Access link + token (only when running) -->
              {#if webStatus?.running && webToken}
                {@const isAllInterfaces =
                  webStatus.bind === "0.0.0.0" ||
                  webStatus.bind === "::" ||
                  webStatus.bind === "[::]"}
                {@const rawHost = isAllInterfaces ? webLanIp : webStatus.bind}
                {@const displayHost = rawHost
                  ? rawHost.includes(":")
                    ? `[${rawHost}]`
                    : rawHost
                  : null}
                {@const tunnelUrl = buildTunnelAccessUrl()}
                {@const localUrl = buildLocalAccessUrl()}
                <div class="space-y-2">
                  {#if tunnelUrl}
                    <!-- Tunnel link (primary) -->
                    <div class="flex items-center gap-2">
                      <span class="text-xs text-muted-foreground shrink-0"
                        >{t("settings_general_webTunnelLink")}</span
                      >
                      <code
                        class="flex-1 rounded-md border bg-muted/50 px-3 py-1.5 font-mono text-xs overflow-hidden text-ellipsis whitespace-nowrap"
                        >{tunnelUrl.replace(/[?#]token=.*$/, "?token=...")}</code
                      >
                      <button
                        class="rounded-md border px-3 py-1.5 text-xs text-muted-foreground hover:bg-accent transition-colors shrink-0"
                        onclick={async () => {
                          await navigator.clipboard.writeText(tunnelUrl);
                          webTunnelLinkCopied = true;
                          dbg("settings", "tunnelLink copied");
                          setTimeout(() => (webTunnelLinkCopied = false), 1500);
                        }}
                      >
                        {webTunnelLinkCopied
                          ? t("settings_general_webCopied")
                          : t("settings_general_webCopyLink")}
                      </button>
                      <button
                        class="rounded-md border px-3 py-1.5 text-xs text-muted-foreground hover:bg-accent transition-colors shrink-0"
                        onclick={async () => {
                          try {
                            const { open } = await import("@tauri-apps/plugin-shell");
                            await open(tunnelUrl);
                            dbg("settings", "tunnelLink opened in browser");
                          } catch (e) {
                            dbgWarn("settings", "failed to open browser", e);
                          }
                        }}
                      >
                        {t("settings_general_webOpenBrowser")}
                      </button>
                    </div>
                    <!-- Local link (secondary, muted) -->
                    {#if displayHost && localUrl}
                      <div class="flex items-center gap-2">
                        <span class="text-xs text-muted-foreground shrink-0"
                          >{t("settings_general_webLocalLink")}</span
                        >
                        <code
                          class="flex-1 rounded-md border bg-muted/30 px-3 py-1.5 font-mono text-xs text-muted-foreground overflow-hidden text-ellipsis whitespace-nowrap"
                          >{localUrl.replace(/#token=.*$/, "#token=...")}</code
                        >
                        <button
                          class="rounded-md border px-3 py-1.5 text-xs text-muted-foreground hover:bg-accent transition-colors shrink-0"
                          onclick={async () => {
                            if (localUrl) {
                              await navigator.clipboard.writeText(localUrl);
                              webLinkCopied = true;
                              dbg("settings", "localLink copied");
                              setTimeout(() => (webLinkCopied = false), 1500);
                            }
                          }}
                        >
                          {webLinkCopied
                            ? t("settings_general_webCopied")
                            : t("settings_general_webCopyLink")}
                        </button>
                      </div>
                    {/if}
                  {:else if displayHost}
                    <div class="flex items-center gap-2">
                      <code
                        class="flex-1 rounded-md border bg-muted/50 px-3 py-1.5 font-mono text-xs overflow-hidden text-ellipsis whitespace-nowrap"
                        >{`http://${displayHost}:${webStatus.port}/login#token=...`}</code
                      >
                      <button
                        class="rounded-md border px-3 py-1.5 text-xs text-muted-foreground hover:bg-accent transition-colors shrink-0"
                        onclick={copyAccessLink}
                      >
                        {webLinkCopied
                          ? t("settings_general_webCopied")
                          : t("settings_general_webCopyLink")}
                      </button>
                      <button
                        class="rounded-md border px-3 py-1.5 text-xs text-muted-foreground hover:bg-accent transition-colors shrink-0"
                        onclick={openAccessLink}
                      >
                        {t("settings_general_webOpenBrowser")}
                      </button>
                    </div>
                  {:else if isAllInterfaces}
                    <p class="text-xs text-amber-400">
                      {t("settings_general_webLanIpFailed")}
                    </p>
                  {/if}
                  <!-- Token reveal + regenerate -->
                  <div class="flex items-center gap-3 text-xs text-muted-foreground">
                    {#if showWebToken}
                      <code class="font-mono text-[11px] select-all">{webToken}</code>
                      <button
                        class="hover:text-foreground transition-colors shrink-0"
                        onclick={() => (showWebToken = false)}
                      >
                        {t("settings_general_hide")}
                      </button>
                      <button
                        class="hover:text-foreground transition-colors shrink-0"
                        onclick={async () => {
                          if (webToken) {
                            await navigator.clipboard.writeText(webToken);
                            webTokenCopied = true;
                            dbg("settings", "webToken copied");
                            setTimeout(() => (webTokenCopied = false), 1500);
                          }
                        }}
                      >
                        {webTokenCopied
                          ? t("settings_general_webCopied")
                          : t("settings_general_webCopy")}
                      </button>
                    {:else}
                      <button
                        class="hover:text-foreground transition-colors"
                        onclick={() => (showWebToken = true)}
                      >
                        {t("settings_general_webShowToken")}
                      </button>
                    {/if}
                    <span class="text-border">|</span>
                    <button
                      class="text-amber-400/70 hover:text-amber-400 transition-colors"
                      onclick={async () => {
                        try {
                          const newToken = await api.regenerateWebServerToken();
                          webToken = newToken;
                          showWebToken = false;
                          webTokenCopied = false;
                          webLinkCopied = false;
                          dbg("settings", "webToken regenerated");
                        } catch (e) {
                          dbgWarn("settings", "webToken regenerate failed", e);
                        }
                      }}
                    >
                      {t("settings_general_webRegenerate")}
                    </button>
                    <span class="text-muted-foreground">—</span>
                    <span class="text-muted-foreground"
                      >{t("settings_general_webRegenerateDesc")}</span
                    >
                  </div>
                </div>
              {/if}

              <!-- HTTP Tunnel -->
              <div>
                <p class="text-sm font-medium mb-1.5">{t("settings_general_webTunnel")}</p>
                <input
                  type="text"
                  class="w-full rounded-md border bg-background px-3 py-1.5 text-sm"
                  placeholder={t("settings_general_webTunnelPlaceholder")}
                  bind:value={webTunnelUrl}
                  onblur={() => {
                    const v = webTunnelUrl.trim();
                    if (v) {
                      try {
                        const u = new URL(v);
                        if (u.protocol !== "http:" && u.protocol !== "https:") {
                          webTunnelError = t("settings_general_webTunnelInvalid");
                        } else {
                          webTunnelError = null;
                        }
                      } catch {
                        webTunnelError = t("settings_general_webTunnelInvalid");
                      }
                    } else {
                      webTunnelError = null;
                    }
                  }}
                />
                {#if webTunnelError}
                  <p class="text-xs text-red-400 mt-1">{webTunnelError}</p>
                {:else}
                  <p class="text-xs text-muted-foreground mt-1">
                    {t("settings_general_webTunnelDesc")}
                  </p>
                {/if}
              </div>

              <!-- Access + Port — side by side -->
              <div class="grid grid-cols-[1fr_auto] gap-4 items-start">
                <div>
                  <p class="text-sm font-medium mb-1.5">{t("settings_general_webAccess")}</p>
                  <div class="flex gap-2">
                    <button
                      class="flex-1 rounded-md border px-3 py-2 text-[13px] transition-colors {webBindValue ===
                      '127.0.0.1'
                        ? 'border-primary bg-primary/10 text-primary'
                        : 'text-muted-foreground hover:bg-accent'}"
                      onclick={() => (webBindValue = "127.0.0.1")}
                    >
                      {t("settings_general_webAccessLocal")}
                    </button>
                    <button
                      class="flex-1 rounded-md border px-3 py-2 text-[13px] transition-colors {webBindValue ===
                      '0.0.0.0'
                        ? 'border-primary bg-primary/10 text-primary'
                        : 'text-muted-foreground hover:bg-accent'}"
                      onclick={() => (webBindValue = "0.0.0.0")}
                    >
                      {t("settings_general_webAccessLan")}
                    </button>
                  </div>
                  <p class="text-xs text-muted-foreground mt-1">
                    {t("settings_general_webAccessDesc")}
                  </p>
                </div>
                <div>
                  <p class="text-sm font-medium mb-1.5">{t("settings_general_webPort")}</p>
                  <input
                    type="number"
                    class="w-24 rounded-md border bg-background px-3 py-1.5 text-sm"
                    bind:value={webPortInput}
                    min="1024"
                    max="65535"
                    onblur={() => {
                      const n = parseInt(webPortInput, 10);
                      if (isNaN(n) || n < 1024 || n > 65535) {
                        webRestartError = t("settings_general_webPortInvalid");
                      } else {
                        if (webRestartError === t("settings_general_webPortInvalid")) {
                          webRestartError = null;
                        }
                      }
                    }}
                  />
                </div>
              </div>

              <!-- Advanced (collapsible) -->
              <div>
                <button
                  class="flex items-center gap-1.5 text-xs text-muted-foreground hover:text-foreground transition-colors"
                  onclick={() => (webAdvancedOpen = !webAdvancedOpen)}
                >
                  <svg
                    class="h-3 w-3 transition-transform {webAdvancedOpen ? 'rotate-90' : ''}"
                    viewBox="0 0 24 24"
                    fill="none"
                    stroke="currentColor"
                    stroke-width="2"><path d="m9 18 6-6-6-6" /></svg
                  >
                  {t("settings_general_webAdvanced")}
                </button>

                {#if webAdvancedOpen}
                  <div class="mt-3 space-y-2">
                    <p class="text-sm font-medium">{t("settings_general_webAllowedOrigins")}</p>
                    {#if webOrigins.length > 0}
                      <div class="flex flex-wrap gap-1.5">
                        {#each webOrigins as origin, i}
                          <span
                            class="inline-flex items-center gap-1 rounded-full border bg-muted/50 px-2.5 py-0.5 text-xs"
                          >
                            {origin}
                            <button
                              class="text-muted-foreground hover:text-foreground"
                              onclick={() => {
                                webOrigins = webOrigins.filter((_, idx) => idx !== i);
                              }}
                            >
                              <svg
                                class="h-3 w-3"
                                viewBox="0 0 24 24"
                                fill="none"
                                stroke="currentColor"
                                stroke-width="2"><path d="M18 6L6 18M6 6l12 12" /></svg
                              >
                            </button>
                          </span>
                        {/each}
                      </div>
                    {/if}
                    <div class="flex gap-2">
                      <input
                        type="text"
                        class="flex-1 rounded-md border bg-background px-3 py-1.5 text-sm"
                        placeholder={t("settings_general_webAllowedOriginsPlaceholder")}
                        bind:value={webOriginInput}
                        onkeydown={(e) => {
                          if (e.key === "Enter") {
                            e.preventDefault();
                            addWebOrigin();
                          }
                        }}
                      />
                      <button
                        class="rounded-md border px-3 py-1.5 text-xs text-muted-foreground hover:bg-accent transition-colors shrink-0"
                        onclick={addWebOrigin}
                      >
                        {t("settings_general_webAddOrigin")}
                      </button>
                    </div>
                    {#if webOriginError}
                      <p class="text-xs text-red-400">{webOriginError}</p>
                    {/if}
                    <p class="text-xs text-muted-foreground">
                      {t("settings_general_webAllowedOriginsDesc")}
                    </p>
                  </div>
                {/if}
              </div>

              <!-- Apply + feedback -->
              <div class="space-y-2 pt-2 border-t border-border">
                {#if webRestartError}
                  <p class="text-xs text-red-400">
                    {t("settings_general_webRestartFailed", { error: webRestartError })}
                  </p>
                {/if}
                {#if webRestartWarning}
                  <p class="text-xs text-amber-400">{webRestartWarning}</p>
                {/if}
                <button
                  class="rounded-md border border-primary px-4 py-2 text-sm font-medium text-primary hover:bg-primary/10 transition-colors disabled:opacity-50"
                  disabled={webRestarting}
                  onclick={applyWebServerSettings}
                >
                  {#if webRestarting}
                    <span class="inline-flex items-center gap-2">
                      <span
                        class="h-3.5 w-3.5 animate-spin rounded-full border-2 border-primary border-t-transparent"
                      ></span>
                      {t("settings_general_webApplying")}
                    </span>
                  {:else}
                    {t("settings_general_webApply")}
                  {/if}
                </button>
              </div>
            {:else}
              <p class="text-sm text-muted-foreground">
                {t("settings_general_webDisabled")}
              </p>
            {/if}
          </Card>
        {/if}
      </div>

      <!-- ═══ Connection tab ═══ -->
    {:else if activeTab === "connection"}
      <div class="space-y-6">
        <!-- Claude authentication -->
        <Card class="p-6 space-y-5">
          <div class="flex items-center justify-between">
            <h2 class="text-sm font-semibold text-muted-foreground uppercase tracking-wider">
              {t("settings_connection_claudeTitle")}
            </h2>
            {#if generalSaved}
              <span class="text-xs text-emerald-500 flex items-center gap-1 animate-fade-in">
                <svg
                  class="h-3 w-3"
                  viewBox="0 0 24 24"
                  fill="none"
                  stroke="currentColor"
                  stroke-width="2"
                  stroke-linecap="round"
                  stroke-linejoin="round"><path d="M20 6 9 17l-5-5" /></svg
                >
                {t("settings_general_saved")}
              </span>
            {/if}
          </div>

          <!-- Auth Mode selector: 2-way radio -->
          <div>
            <span class="text-sm font-medium mb-2 block">{t("settings_auth_modeLabel")}</span>
            <div class="mt-1 grid grid-cols-2 gap-3">
              <button
                class="flex flex-col items-center gap-2 rounded-lg border p-4 text-sm transition-all duration-150
                {authMode === 'cli'
                  ? 'border-primary bg-primary/5 ring-1 ring-primary/20'
                  : 'hover:bg-accent hover:border-ring/30'}"
                onclick={() => {
                  authMode = "cli";
                  saveGeneralPatch({
                    auth_mode: "cli",
                    anthropic_base_url: null,
                    active_platform_id: null,
                    auth_env_var: null,
                  });
                  api.removeCliApiKey().catch(() => {});
                  api
                    .getAuthOverview()
                    .then((ov) => (authOverview = ov))
                    .catch(() => {});
                }}
              >
                <div
                  class="flex h-10 w-10 items-center justify-center rounded-full bg-emerald-500/10"
                >
                  <svg
                    class="h-5 w-5 text-emerald-400"
                    viewBox="0 0 24 24"
                    fill="none"
                    stroke="currentColor"
                    stroke-width="2"
                    stroke-linecap="round"
                    stroke-linejoin="round"
                  >
                    <rect x="3" y="11" width="18" height="11" rx="2" ry="2" /><path
                      d="M7 11V7a5 5 0 0 1 10 0v4"
                    />
                  </svg>
                </div>
                <span class="font-medium">{t("auth_cliAuth")}</span>
                <span class="text-[10px] text-muted-foreground text-center"
                  >{t("settings_auth_modeCliDesc")}</span
                >
              </button>
              <button
                class="flex flex-col items-center gap-2 rounded-lg border p-4 text-sm transition-all duration-150
                {authMode === 'api'
                  ? 'border-primary bg-primary/5 ring-1 ring-primary/20'
                  : 'hover:bg-accent hover:border-ring/30'}"
                onclick={() => {
                  authMode = "api";
                  saveGeneralPatch({ auth_mode: "api" });
                  api
                    .getAuthOverview()
                    .then((ov) => (authOverview = ov))
                    .catch(() => {});
                }}
              >
                <div
                  class="flex h-10 w-10 items-center justify-center rounded-full bg-violet-500/10"
                >
                  <svg
                    class="h-5 w-5 text-violet-400"
                    viewBox="0 0 24 24"
                    fill="none"
                    stroke="currentColor"
                    stroke-width="2"
                    stroke-linecap="round"
                    stroke-linejoin="round"
                  >
                    <path
                      d="M21 2l-2 2m-7.61 7.61a5.5 5.5 0 1 1-7.778 7.778 5.5 5.5 0 0 1 7.777-7.777zm0 0L15.5 7.5m0 0l3 3L22 7l-3-3m-3.5 3.5L19 4"
                    />
                  </svg>
                </div>
                <span class="font-medium">{t("auth_appApiKey")}</span>
                <span class="text-[10px] text-muted-foreground text-center"
                  >{t("settings_auth_modeAppDesc")}</span
                >
              </button>
            </div>
          </div>

          <!-- CLI Auth details (expanded when auth_mode = cli) -->
          {#if authMode === "cli"}
            <div class="space-y-4 rounded-lg border border-border/50 p-4">
              <!-- CLI Login status -->
              <div>
                <h3 class="text-sm font-medium mb-1">{t("settings_auth_cliLoginTitle")}</h3>
                <p class="text-xs text-muted-foreground mb-2">{t("settings_auth_cliLoginDesc")}</p>
                {#if authOverview?.cli_login_available}
                  <div class="flex items-center gap-2">
                    <span class="h-2 w-2 rounded-full bg-emerald-500"></span>
                    <span class="text-xs text-emerald-500">
                      {t("auth_loggedIn")}{authOverview.cli_login_account
                        ? `: ${authOverview.cli_login_account}`
                        : ""}
                    </span>
                  </div>
                {:else}
                  <div class="flex flex-col gap-2">
                    <div class="flex items-center gap-2">
                      <span class="h-2 w-2 rounded-full bg-muted-foreground/40"></span>
                      <span class="text-xs text-muted-foreground">{t("auth_notLoggedIn")}</span>
                      <Button
                        size="sm"
                        variant="outline"
                        disabled={cliLoginLoading}
                        onclick={() => {
                          cliLoginLoading = true;
                          cliLoginError = "";
                          api
                            .runClaudeLogin()
                            .then((success) => {
                              if (success) {
                                api
                                  .getAuthOverview()
                                  .then((ov) => (authOverview = ov))
                                  .catch(() => {});
                              } else {
                                cliLoginError = t("setup_loginFailed");
                              }
                            })
                            .catch((e) => {
                              cliLoginError = String(e);
                            })
                            .finally(() => {
                              cliLoginLoading = false;
                            });
                        }}
                      >
                        {#if cliLoginLoading}
                          <span class="flex items-center gap-1.5">
                            <span
                              class="h-3 w-3 border border-foreground/30 border-t-foreground rounded-full animate-spin"
                            ></span>
                            {t("settings_auth_cliLoginBtn")}
                          </span>
                        {:else}
                          {t("settings_auth_cliLoginBtn")}
                        {/if}
                      </Button>
                    </div>
                    {#if cliLoginError}
                      <div class="rounded border border-red-500/30 bg-red-500/5 px-2 py-1">
                        <p class="text-xs text-red-500">{cliLoginError}</p>
                      </div>
                    {/if}
                  </div>
                {/if}
              </div>

              <!-- CLI API Key (read-only) -->
              <div>
                <h3 class="text-sm font-medium mb-1">{t("settings_auth_cliApiKeyTitle")}</h3>
                {#if authOverview?.cli_has_api_key}
                  <div class="flex items-center gap-2">
                    <span class="h-2 w-2 rounded-full bg-emerald-500"></span>
                    <span class="text-xs text-emerald-500"
                      >{t("auth_cliKeyHint", { hint: authOverview.cli_api_key_hint ?? "" })}</span
                    >
                  </div>
                  <p class="mt-1 text-[10px] text-muted-foreground/70 italic">
                    {#if authOverview.cli_api_key_source === "settings"}
                      {t("settings_auth_cliApiKeySourceSettings")}
                    {:else if authOverview.cli_api_key_source === "env"}
                      {t("settings_auth_cliApiKeySourceEnv")}
                    {:else if authOverview.cli_api_key_source?.startsWith("shell_config:")}
                      {t("settings_auth_cliApiKeySourceShell", {
                        path: authOverview.cli_api_key_source.slice(13),
                      })}
                    {/if}
                  </p>
                {:else}
                  <div class="flex items-center gap-2">
                    <span class="h-2 w-2 rounded-full bg-muted-foreground/40"></span>
                    <span class="text-xs text-muted-foreground"
                      >{t("settings_auth_cliApiKeyNotSet")}</span
                    >
                  </div>
                  <p class="mt-1 text-[10px] text-muted-foreground/70 italic">
                    {t("settings_auth_cliApiKeyEditHint")}
                  </p>
                {/if}
              </div>

              <!-- Priority hint -->
              {#if authOverview?.cli_login_available && authOverview?.cli_has_api_key}
                <p class="text-[10px] text-muted-foreground/70 italic">
                  {t("auth_cliPriorityHint")}
                </p>
              {/if}
            </div>
          {/if}

          {#if authMode === "api"}
            <div class="space-y-4 rounded-lg border border-border/50 p-4">
              <div>
                <h3 class="text-sm font-medium mb-1">{t("settings_auth_appApiKeyTitle")}</h3>
                <p class="text-xs text-muted-foreground mb-3">{t("settings_auth_appApiKeyDesc")}</p>
              </div>
              <!-- Platform selector -->
              <div>
                <span class="text-sm font-medium mb-1.5 block"
                  >{t("settings_general_platform")}</span
                >
                <!-- Platform grid (always visible) -->
                <div class="grid grid-cols-4 gap-1.5">
                  {#each groupedPlatforms as group}
                    <div
                      class="col-span-4 px-1 pt-2 pb-0.5 text-[10px] font-semibold uppercase tracking-wider text-muted-foreground/60"
                    >
                      {group.label}
                    </div>
                    {#each group.items as preset}
                      <button
                        class="flex items-center gap-2 rounded-md p-2 text-left transition-colors relative group
                      {selectedPlatformId === preset.id
                          ? 'bg-primary/10 ring-1 ring-primary'
                          : 'bg-muted/40 hover:bg-muted/70'}"
                        onclick={() => applyPlatformPreset(preset)}
                      >
                        <ProviderIcon id={preset.id} name={preset.name} />
                        <span class="flex flex-col min-w-0 flex-1">
                          <span class="text-xs font-medium truncate">{preset.name}</span>
                          <span class="text-[10px] text-muted-foreground truncate"
                            >{preset.description}</span
                          >
                        </span>
                        {#if isCustomPlatform(preset.id)}
                          <span
                            role="button"
                            tabindex="0"
                            class="absolute top-1 right-1 opacity-0 group-hover:opacity-100 text-muted-foreground hover:text-destructive transition-all p-0.5 cursor-pointer"
                            onclick={(e: MouseEvent) => {
                              e.stopPropagation();
                              deleteCustomEndpoint(preset.id);
                            }}
                            onkeydown={(e: KeyboardEvent) => {
                              if (e.key === "Enter" || e.key === " ") {
                                e.stopPropagation();
                                deleteCustomEndpoint(preset.id);
                              }
                            }}
                            title={t("settings_general_deleteCustom")}
                          >
                            <svg
                              class="h-3 w-3"
                              viewBox="0 0 24 24"
                              fill="none"
                              stroke="currentColor"
                              stroke-width="2"><path d="M18 6 6 18" /><path d="m6 6 12 12" /></svg
                            >
                          </span>
                        {/if}
                        {#if preset.category === "local"}
                          {@const ps = localProxyStatuses[preset.id]}
                          <span
                            class="absolute bottom-1 right-1 h-1.5 w-1.5 rounded-full {ps?.running &&
                            !ps.needsAuth
                              ? 'bg-green-500'
                              : ps?.running && ps.needsAuth
                                ? 'bg-amber-500'
                                : 'bg-muted-foreground/30'}"
                            title={ps?.running && !ps.needsAuth
                              ? t("settings_local_running")
                              : ps?.running && ps.needsAuth
                                ? t("settings_local_needsAuth")
                                : t("settings_local_notDetected")}
                          ></span>
                        {:else if findCredential(platformCredentials, preset.id)?.api_key}
                          <span
                            class="absolute bottom-1 right-1 h-1.5 w-1.5 rounded-full bg-green-500"
                            title="Key saved"
                          ></span>
                        {/if}
                      </button>
                    {/each}
                    {#if group.id === "custom"}
                      <!-- Add Custom -->
                      <button
                        class="flex flex-col items-center justify-center gap-1 rounded-md border border-dashed border-muted-foreground/30 p-2 text-muted-foreground hover:border-primary/50 hover:text-foreground hover:bg-muted/40 transition-colors"
                        onclick={() => addCustomEndpoint()}
                      >
                        <svg
                          class="h-4 w-4"
                          viewBox="0 0 24 24"
                          fill="none"
                          stroke="currentColor"
                          stroke-width="2"><path d="M12 5v14" /><path d="M5 12h14" /></svg
                        >
                        <span class="text-[10px]">{t("settings_general_addCustom")}</span>
                      </button>
                    {/if}
                  {/each}
                </div>
              </div>

              {#if selectedPlatform?.category === "local"}
                <!-- Local proxy status card -->
                <div class="rounded-lg border p-4 space-y-3">
                  <div class="flex items-center gap-2">
                    {#if localProxyChecking}
                      <span class="h-2 w-2 rounded-full bg-amber-400 animate-pulse"></span>
                      <span class="text-sm">{t("settings_local_checking")}</span>
                    {:else if localProxyStatus?.running && !localProxyStatus.needsAuth}
                      <span class="h-2 w-2 rounded-full bg-green-500"></span>
                      <span class="text-sm font-medium">{t("settings_local_running")}</span>
                    {:else if localProxyStatus?.running && localProxyStatus.needsAuth}
                      <span class="h-2 w-2 rounded-full bg-amber-500"></span>
                      <span class="text-sm font-medium">{t("settings_local_needsAuth")}</span>
                    {:else}
                      <span class="h-2 w-2 rounded-full bg-muted-foreground/30"></span>
                      <span class="text-sm">{t("settings_local_notDetected")}</span>
                    {/if}
                    <button
                      class="ml-auto rounded-md border px-2.5 py-1 text-xs text-muted-foreground hover:text-foreground hover:bg-accent transition-colors"
                      onclick={checkLocalProxy}>{t("settings_local_refresh")}</button
                    >
                  </div>
                  <p class="text-xs text-muted-foreground font-mono">{anthropicBaseUrl}</p>
                  {#if localProxyStatus && !localProxyStatus.running}
                    <p class="text-xs text-amber-500">
                      {selectedPlatform.setup_hint
                        ? t(selectedPlatform.setup_hint as Parameters<typeof t>[0])
                        : t("settings_local_startHint", { name: selectedPlatform.name })}
                    </p>
                  {/if}
                  {#if selectedPlatform.docs_url}
                    <a
                      href={selectedPlatform.docs_url}
                      target="_blank"
                      class="text-xs text-primary hover:underline"
                    >
                      {t("settings_local_viewDocs")} →
                    </a>
                  {/if}
                </div>

                <!-- Advanced settings toggle -->
                <button
                  class="text-xs text-muted-foreground hover:text-foreground transition-colors"
                  onclick={() => (localAdvancedOpen = !localAdvancedOpen)}
                >
                  {localAdvancedOpen ? "▾" : "▸"}
                  {t("settings_local_advanced")}
                </button>
              {/if}

              {#if selectedPlatform?.category !== "local" || localAdvancedOpen}
                <!-- Custom endpoint: Name + Auth Type -->
                {#if isCustomPlatform(selectedPlatformId ?? "")}
                  <div class="flex gap-3">
                    <div class="flex-1">
                      <label class="text-sm font-medium mb-1.5 block"
                        >{t("settings_general_customNameLabel")}</label
                      >
                      <Input
                        value={findCredential(platformCredentials, selectedPlatformId ?? "")
                          ?.name ?? ""}
                        placeholder={t("settings_general_customNamePlaceholder")}
                        class="mt-1 text-xs"
                        onblur={(e) => {
                          const target = e.currentTarget as HTMLInputElement | null;
                          if (!target) return;
                          const val = target.value.trim();
                          if (selectedPlatformId) {
                            _upsertCredential(selectedPlatformId, { name: val || "Custom" });
                            saveGeneralPatch({ platform_credentials: platformCredentials });
                          }
                        }}
                      />
                    </div>
                    <div>
                      <label class="text-sm font-medium mb-1.5 block"
                        >{t("settings_general_authType")}</label
                      >
                      <div class="mt-1 flex rounded-md border border-input overflow-hidden">
                        <button
                          class="px-3 py-1.5 text-xs font-medium transition-colors {(findCredential(
                            platformCredentials,
                            selectedPlatformId ?? '',
                          )?.auth_env_var ?? 'ANTHROPIC_AUTH_TOKEN') === 'ANTHROPIC_AUTH_TOKEN'
                            ? 'bg-primary text-primary-foreground'
                            : 'text-muted-foreground hover:text-foreground hover:bg-accent'}"
                          onclick={() => {
                            if (selectedPlatformId) {
                              _upsertCredential(selectedPlatformId, {
                                auth_env_var: "ANTHROPIC_AUTH_TOKEN",
                              });
                              saveGeneralPatch({ platform_credentials: platformCredentials });
                            }
                          }}>Bearer</button
                        >
                        <button
                          class="px-3 py-1.5 text-xs font-medium transition-colors border-l border-input {(findCredential(
                            platformCredentials,
                            selectedPlatformId ?? '',
                          )?.auth_env_var ?? 'ANTHROPIC_AUTH_TOKEN') === 'ANTHROPIC_API_KEY'
                            ? 'bg-primary text-primary-foreground'
                            : 'text-muted-foreground hover:text-foreground hover:bg-accent'}"
                          onclick={() => {
                            if (selectedPlatformId) {
                              _upsertCredential(selectedPlatformId, {
                                auth_env_var: "ANTHROPIC_API_KEY",
                              });
                              saveGeneralPatch({ platform_credentials: platformCredentials });
                            }
                          }}>x-api-key</button
                        >
                      </div>
                    </div>
                  </div>
                {/if}

                <!-- API Key input -->
                <div>
                  <label class="text-sm font-medium mb-1.5 block" for="api-key"
                    >{t("settings_general_apiKey")}</label
                  >
                  <div class="mt-1 flex gap-2">
                    <div class="flex-1 relative">
                      <Input
                        bind:value={anthropicApiKey}
                        placeholder={selectedPlatform?.key_placeholder ?? "<your-api-key>"}
                        type={showApiKey ? "text" : "password"}
                        class="font-mono text-xs"
                        onblur={() => persistCurrentPlatform()}
                      />
                    </div>
                    <button
                      class="rounded-md border px-3 py-2 text-xs text-muted-foreground hover:bg-accent transition-colors"
                      onclick={() => (showApiKey = !showApiKey)}
                    >
                      {showApiKey ? t("settings_general_hide") : t("settings_general_show")}
                    </button>
                    {#if selectedPlatform?.category !== "local"}
                      {@const cred = findCredential(platformCredentials, selectedPlatformId ?? "")}
                      {@const authEnvVar =
                        cred?.auth_env_var || selectedPlatform?.auth_env_var || "ANTHROPIC_API_KEY"}
                      {@const [presetOpusTest, presetSonnetTest] = expandModelsToTiers(
                        selectedPlatform?.models,
                      )}
                      {@const testModel =
                        modelSonnet.trim() ||
                        modelOpus.trim() ||
                        presetSonnetTest ||
                        presetOpusTest ||
                        ""}
                      {@const isCustom = isCustomPlatform(selectedPlatformId ?? "")}
                      {@const noKey = !anthropicApiKey}
                      {@const noUrl = isCustom && !anthropicBaseUrl.trim()}
                      {@const disableReason = noKey
                        ? t("settings_apiTest_noKey")
                        : noUrl
                          ? t("settings_apiTest_noUrl")
                          : ""}
                      <button
                        class="rounded-md border px-3 py-2 text-xs transition-colors {disableReason ||
                        apiTestLoading
                          ? 'text-muted-foreground/50 cursor-not-allowed'
                          : 'text-muted-foreground hover:bg-accent hover:text-foreground'}"
                        disabled={!!disableReason || apiTestLoading}
                        title={disableReason || ""}
                        onclick={async () => {
                          const myRequestId = ++apiTestRequestId;
                          const myPlatformId = selectedPlatformId;
                          apiTestLoading = true;
                          apiTestResult = null;
                          dbg("settings", "testApi start", {
                            platform: myPlatformId,
                            model: testModel,
                            authEnvVar,
                            reqId: myRequestId,
                          });
                          try {
                            const result = await api.testApiConnectivity(
                              anthropicApiKey,
                              anthropicBaseUrl,
                              authEnvVar,
                              testModel,
                            );
                            if (myRequestId !== apiTestRequestId) return;
                            if (myPlatformId !== selectedPlatformId) return;
                            apiTestResult = result;
                            if (result.success) {
                              dbg("settings", "testApi success", { latencyMs: result.latencyMs });
                            } else {
                              dbgWarn("settings", "testApi error", result.error);
                            }
                          } catch (e) {
                            if (
                              myRequestId !== apiTestRequestId ||
                              myPlatformId !== selectedPlatformId
                            )
                              return;
                            apiTestResult = {
                              success: false,
                              latencyMs: 0,
                              error: String(e),
                              partial: false,
                            };
                            dbgWarn("settings", "testApi error", e);
                          } finally {
                            if (myRequestId === apiTestRequestId) apiTestLoading = false;
                          }
                        }}
                      >
                        {t("settings_apiTest")}
                      </button>
                    {/if}
                  </div>
                  <!-- API test result -->
                  {#if apiTestLoading}
                    <div class="mt-1.5 flex items-center gap-1.5">
                      <span class="h-2 w-2 rounded-full bg-amber-400 animate-pulse"></span>
                      <span class="text-xs text-muted-foreground"
                        >{t("settings_apiTest_testing")}</span
                      >
                    </div>
                  {:else if apiTestResult?.success && apiTestResult.partial}
                    <div class="mt-1.5 flex items-center gap-1.5">
                      <span class="h-2 w-2 rounded-full bg-green-500"></span>
                      <span class="text-xs text-green-600 dark:text-green-400"
                        >{t("settings_apiTest_partial", {
                          latency: String(apiTestResult.latencyMs),
                        })}</span
                      >
                    </div>
                  {:else if apiTestResult?.success}
                    <div class="mt-1.5 flex items-center gap-1.5">
                      <span class="h-2 w-2 rounded-full bg-green-500"></span>
                      <span class="text-xs text-green-600 dark:text-green-400"
                        >{t("settings_apiTest_success", {
                          latency: String(apiTestResult.latencyMs),
                        })}</span
                      >
                    </div>
                  {:else if apiTestResult && !apiTestResult.success}
                    <div class="mt-1.5 flex items-center gap-1.5">
                      <span class="h-2 w-2 rounded-full bg-red-500"></span>
                      <span class="text-xs text-red-600 dark:text-red-400"
                        >{apiTestResult.error ?? t("settings_apiTest_failed")}</span
                      >
                    </div>
                  {:else if selectedPlatform?.id === "ollama"}
                    <p class="mt-1 text-xs text-muted-foreground">{t("setup_noKeyNeeded")}</p>
                  {:else}
                    <p class="mt-1 text-xs text-muted-foreground">
                      {t("settings_general_apiKeyStored")}
                    </p>
                  {/if}
                </div>

                <!-- Base URL (only show for custom or direct editing) -->
                <div>
                  <label class="text-sm font-medium mb-1.5 block" for="base-url"
                    >{t("settings_general_baseUrl")}</label
                  >
                  <Input
                    bind:value={anthropicBaseUrl}
                    placeholder="https://api.anthropic.com"
                    class="mt-1 font-mono text-xs"
                    disabled={selectedPlatformId !== null &&
                      selectedPlatformId !== "anthropic" &&
                      selectedPlatform?.category !== "local" &&
                      !isCustomPlatform(selectedPlatformId ?? "")}
                    onblur={() => persistCurrentPlatform()}
                  />
                  <p class="mt-1 text-xs text-muted-foreground">
                    {#if selectedPlatform && selectedPlatform.auth_env_var === "ANTHROPIC_AUTH_TOKEN"}
                      {t("setup_authTypeBearer")}
                    {:else if selectedPlatform && selectedPlatform.auth_env_var === "ANTHROPIC_API_KEY"}
                      {t("setup_authTypeApiKey")}
                    {:else}
                      {t("settings_general_baseUrlHelp")}
                    {/if}
                  </p>
                </div>

                <!-- Models (3-tier: Opus / Sonnet / Haiku) -->
                {@const [presetOpus, presetSonnet, presetHaiku] = expandModelsToTiers(
                  selectedPlatform?.models,
                )}
                {@const phOpus = presetOpus || t("settings_general_modelsPlaceholder")}
                {@const phSonnet = presetSonnet || t("settings_general_modelsPlaceholder")}
                {@const phHaiku = presetHaiku || t("settings_general_modelsPlaceholder")}
                <div>
                  <label class="text-sm font-medium mb-1.5 block"
                    >{t("settings_general_models")}</label
                  >
                  <div class="mt-1 space-y-1.5">
                    <div class="flex items-center gap-2">
                      <span class="text-xs text-muted-foreground w-24 shrink-0 text-right"
                        >{t("settings_general_modelOpus")}</span
                      >
                      <Input
                        bind:value={modelOpus}
                        placeholder={phOpus}
                        class="flex-1 font-mono text-xs"
                        onblur={() => persistCurrentPlatform()}
                      />
                    </div>
                    <div class="flex items-center gap-2">
                      <span
                        class="text-xs text-muted-foreground w-24 shrink-0 text-right font-medium"
                        >{t("settings_general_modelSonnet")}</span
                      >
                      <Input
                        bind:value={modelSonnet}
                        placeholder={phSonnet}
                        class="flex-1 font-mono text-xs"
                        onblur={() => persistCurrentPlatform()}
                      />
                    </div>
                    <div class="flex items-center gap-2">
                      <span class="text-xs text-muted-foreground w-24 shrink-0 text-right"
                        >{t("settings_general_modelHaiku")}</span
                      >
                      <Input
                        bind:value={modelHaiku}
                        placeholder={phHaiku}
                        class="flex-1 font-mono text-xs"
                        onblur={() => persistCurrentPlatform()}
                      />
                    </div>
                  </div>
                  <p class="mt-1 text-xs text-muted-foreground">
                    {t("settings_general_modelsHelp")}
                  </p>
                </div>

                <!-- Extra Environment Variables -->
                <div>
                  <label class="text-sm font-medium mb-1.5 block" for="extra-env-section">
                    {t("settings_general_extraEnv")}
                  </label>
                  {#each platformExtraEnv as envVar, i}
                    <div class="flex gap-1.5 mt-1.5">
                      <Input
                        bind:value={envVar.key}
                        placeholder={t("settings_general_envKeyPlaceholder")}
                        class="flex-1 font-mono text-xs"
                        oninput={() => markExtraEnvTouched()}
                        onblur={() => persistCurrentPlatform()}
                        onpaste={(e: ClipboardEvent) => handleEnvKeyPaste(e, i)}
                      />
                      <Input
                        bind:value={envVar.value}
                        placeholder={t("settings_general_envValuePlaceholder")}
                        class="flex-1 font-mono text-xs"
                        oninput={() => markExtraEnvTouched()}
                        onblur={() => persistCurrentPlatform()}
                      />
                      <button
                        class="shrink-0 rounded-md p-1.5 text-muted-foreground hover:text-destructive hover:bg-destructive/10 transition-colors"
                        aria-label={t("settings_remote_delete")}
                        onclick={() => {
                          platformExtraEnv = platformExtraEnv.filter((_, idx) => idx !== i);
                          markExtraEnvTouched();
                          persistCurrentPlatform();
                        }}
                      >
                        <svg
                          class="h-3.5 w-3.5"
                          viewBox="0 0 24 24"
                          fill="none"
                          stroke="currentColor"
                          stroke-width="2"
                          stroke-linecap="round"
                          stroke-linejoin="round"
                        >
                          <path d="M18 6 6 18" /><path d="m6 6 12 12" />
                        </svg>
                      </button>
                    </div>
                  {/each}
                  <button
                    class="mt-1.5 flex items-center gap-1 text-xs text-muted-foreground hover:text-foreground transition-colors"
                    onclick={() => {
                      platformExtraEnv = [...platformExtraEnv, { key: "", value: "" }];
                    }}
                  >
                    <svg
                      class="h-3 w-3"
                      viewBox="0 0 24 24"
                      fill="none"
                      stroke="currentColor"
                      stroke-width="2"
                      stroke-linecap="round"
                      stroke-linejoin="round"
                    >
                      <path d="M12 5v14" /><path d="M5 12h14" />
                    </svg>
                    {t("settings_general_addEnvVar")}
                  </button>
                  <p class="mt-1 text-xs text-muted-foreground">
                    {t("settings_general_extraEnvHelp")}
                  </p>
                </div>
              {/if}
            </div>
          {/if}
        </Card>

        <!-- Codex Status -->
        <Card class="p-6 space-y-4">
          <div class="flex items-center justify-between">
            <h2 class="text-sm font-semibold text-muted-foreground uppercase tracking-wider">
              {t("settings_codex_title")}
            </h2>
            <button
              class="text-xs text-muted-foreground hover:text-foreground transition-colors"
              onclick={refreshCodexAll}
            >
              {t("settings_codex_refresh")}
            </button>
          </div>

          {#if codexStatusLoading}
            <div class="flex items-center gap-2">
              <div
                class="h-4 w-4 animate-spin rounded-full border-2 border-primary border-t-transparent"
              ></div>
              <span class="text-sm text-muted-foreground">{t("settings_codex_checking")}</span>
            </div>
          {:else if !codexStatus}
            <p class="text-sm text-muted-foreground">{t("settings_codex_checkFailed")}</p>
          {:else if !codexStatus.installed}
            <div class="flex items-center gap-3">
              <div class="flex h-8 w-8 items-center justify-center rounded-full bg-red-500/10">
                <svg
                  class="h-4 w-4 text-red-400"
                  viewBox="0 0 24 24"
                  fill="none"
                  stroke="currentColor"
                  stroke-width="2"
                  stroke-linecap="round"
                  stroke-linejoin="round"
                >
                  <circle cx="12" cy="12" r="10" /><line x1="15" y1="9" x2="9" y2="15" /><line
                    x1="9"
                    y1="9"
                    x2="15"
                    y2="15"
                  />
                </svg>
              </div>
              <div>
                <p class="text-sm font-medium">{t("settings_codex_notInstalled")}</p>
                <p class="text-xs text-muted-foreground">
                  {t("settings_codex_installHint", { command: "npm i -g @openai/codex" })}
                </p>
              </div>
            </div>
          {:else}
            <div class="space-y-3">
              <!-- Version -->
              <div class="flex items-center gap-3">
                <div
                  class="flex h-8 w-8 items-center justify-center rounded-full bg-emerald-500/10"
                >
                  <svg
                    class="h-4 w-4 text-emerald-400"
                    viewBox="0 0 24 24"
                    fill="none"
                    stroke="currentColor"
                    stroke-width="2"
                    stroke-linecap="round"
                    stroke-linejoin="round"
                  >
                    <path d="M20 6 9 17l-5-5" />
                  </svg>
                </div>
                <div>
                  <p class="text-sm font-medium">{t("settings_codex_installed")}</p>
                  {#if codexStatus.version}
                    <p class="text-xs text-muted-foreground">v{codexStatus.version}</p>
                  {/if}
                </div>
              </div>

              <!-- codex doctor: richer install/config/auth/runtime/app-server diagnostics -->
              <div class="rounded-lg border border-border/60 bg-card/40 p-3">
                <div class="flex items-center justify-between gap-2">
                  <div class="min-w-0">
                    <p class="text-sm font-medium">{t("settings_codex_doctorTitle")}</p>
                    <p class="text-xs text-muted-foreground">{t("settings_codex_doctorDesc")}</p>
                  </div>
                  <button
                    class="shrink-0 rounded-md border border-border/60 px-2.5 py-1 text-xs font-medium hover:bg-accent disabled:opacity-50 transition-colors"
                    disabled={codexDoctorLoading}
                    onclick={runCodexDoctor}
                  >
                    {codexDoctorLoading ? t("common_loading") : t("settings_codex_doctorRun")}
                  </button>
                </div>
                {#if codexDoctorError}
                  <p class="mt-2 text-xs text-red-600 dark:text-red-400">{codexDoctorError}</p>
                {/if}
                {#if codexDoctor}
                  <div class="mt-3 flex items-center gap-2 text-xs">
                    <span
                      class="rounded px-1.5 py-0.5 font-medium {doctorStatusClass(
                        codexDoctor.overallStatus,
                      )}">{codexDoctor.overallStatus}</span
                    >
                    <span class="text-muted-foreground">codex v{codexDoctor.codexVersion}</span>
                  </div>
                  <ul class="mt-2 space-y-1.5">
                    {#each codexDoctorChecks as check (check.id)}
                      <li class="flex items-start gap-2 text-xs">
                        <span
                          class="shrink-0 rounded px-1 py-0.5 text-[10px] font-medium {doctorStatusClass(
                            check.status,
                          )}">{check.status}</span
                        >
                        <div class="min-w-0">
                          <span class="font-medium">{check.id}</span>
                          <span class="text-muted-foreground"> — {check.summary}</span>
                          {#if check.remediation}
                            <p class="text-amber-600 dark:text-amber-400">{check.remediation}</p>
                          {/if}
                        </div>
                      </li>
                    {/each}
                  </ul>
                {/if}
              </div>

              <!-- Auth Mode selector: 2-way radio, mirroring Claude -->
              <div>
                <span class="text-sm font-medium mb-2 block">{t("settings_auth_modeLabel")}</span>
                <div class="mt-1 grid grid-cols-2 gap-3">
                  <button
                    class="flex flex-col items-center gap-2 rounded-lg border p-4 text-sm transition-all duration-150
                    {codexAuthMode === 'cli'
                      ? 'border-primary bg-primary/5 ring-1 ring-primary/20'
                      : 'hover:bg-accent hover:border-ring/30'}"
                    onclick={() => setCodexAuthMode("cli")}
                  >
                    <div
                      class="flex h-10 w-10 items-center justify-center rounded-full bg-emerald-500/10"
                    >
                      <svg
                        class="h-5 w-5 text-emerald-400"
                        viewBox="0 0 24 24"
                        fill="none"
                        stroke="currentColor"
                        stroke-width="2"
                        stroke-linecap="round"
                        stroke-linejoin="round"
                      >
                        <rect x="3" y="11" width="18" height="11" rx="2" ry="2" /><path
                          d="M7 11V7a5 5 0 0 1 10 0v4"
                        />
                      </svg>
                    </div>
                    <span class="font-medium">{t("auth_cliAuth")}</span>
                    <span class="text-[10px] text-muted-foreground text-center"
                      >{t("settings_codex_authModeCliDesc")}</span
                    >
                  </button>
                  <button
                    class="flex flex-col items-center gap-2 rounded-lg border p-4 text-sm transition-all duration-150
                    {codexAuthMode === 'app'
                      ? 'border-primary bg-primary/5 ring-1 ring-primary/20'
                      : 'hover:bg-accent hover:border-ring/30'}"
                    onclick={() => setCodexAuthMode("app")}
                  >
                    <div
                      class="flex h-10 w-10 items-center justify-center rounded-full bg-violet-500/10"
                    >
                      <svg
                        class="h-5 w-5 text-violet-400"
                        viewBox="0 0 24 24"
                        fill="none"
                        stroke="currentColor"
                        stroke-width="2"
                        stroke-linecap="round"
                        stroke-linejoin="round"
                      >
                        <rect x="2" y="2" width="20" height="8" rx="2" ry="2" /><rect
                          x="2"
                          y="14"
                          width="20"
                          height="8"
                          rx="2"
                          ry="2"
                        /><line x1="6" y1="6" x2="6.01" y2="6" /><line
                          x1="6"
                          y1="18"
                          x2="6.01"
                          y2="18"
                        />
                      </svg>
                    </div>
                    <span class="font-medium">{t("settings_codex_authModeProviderLabel")}</span>
                    <span class="text-[10px] text-muted-foreground text-center"
                      >{t("settings_codex_authModeProviderDesc")}</span
                    >
                  </button>
                </div>
              </div>

              <!-- CLI Auth details (codex login owns auth in this mode) -->
              {#if codexAuthMode === "cli"}
                <div class="space-y-3 rounded-lg border border-border/50 p-4">
                  <!-- Auth status -->
                  <div class="flex items-center gap-3">
                    {#if codexStatus.logged_in}
                      <div
                        class="flex h-8 w-8 items-center justify-center rounded-full bg-emerald-500/10"
                      >
                        <svg
                          class="h-4 w-4 text-emerald-400"
                          viewBox="0 0 24 24"
                          fill="none"
                          stroke="currentColor"
                          stroke-width="2"
                          stroke-linecap="round"
                          stroke-linejoin="round"
                        >
                          <rect x="3" y="11" width="18" height="11" rx="2" ry="2" /><path
                            d="M7 11V7a5 5 0 0 1 10 0v4"
                          />
                        </svg>
                      </div>
                      <div>
                        <p class="text-sm font-medium">{t("settings_codex_loggedIn")}</p>
                        <p class="text-xs text-muted-foreground">
                          {codexStatus.auth_method === "chatgpt"
                            ? t("settings_codex_authChatGPT")
                            : codexStatus.auth_method === "api_key"
                              ? t("settings_codex_authApiKey")
                              : t("settings_codex_authGeneric")}
                        </p>
                      </div>
                    {:else}
                      <div
                        class="flex h-8 w-8 items-center justify-center rounded-full bg-amber-500/10"
                      >
                        <svg
                          class="h-4 w-4 text-amber-400"
                          viewBox="0 0 24 24"
                          fill="none"
                          stroke="currentColor"
                          stroke-width="2"
                          stroke-linecap="round"
                          stroke-linejoin="round"
                        >
                          <path
                            d="M10.29 3.86L1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z"
                          /><line x1="12" y1="9" x2="12" y2="13" /><line
                            x1="12"
                            y1="17"
                            x2="12.01"
                            y2="17"
                          />
                        </svg>
                      </div>
                      <div class="flex-1">
                        <p class="text-sm font-medium">{t("settings_codex_notLoggedIn")}</p>
                        {#if codexLoginError}
                          <p class="text-xs text-red-400 mt-1">{codexLoginError}</p>
                        {/if}
                      </div>
                      <button
                        class="shrink-0 rounded-md bg-primary px-3 py-1.5 text-xs font-medium text-primary-foreground hover:bg-primary/90 disabled:opacity-50 transition-colors"
                        disabled={codexLoginLoading}
                        onclick={handleCodexLogin}
                      >
                        {codexLoginLoading
                          ? t("settings_codex_loggingIn")
                          : t("settings_codex_loginBtn")}
                      </button>
                    {/if}
                  </div>
                  <!-- codex login owns auth in this mode -->
                  <p class="text-xs text-muted-foreground/80 pl-11">
                    {t("settings_codex_authNote")}
                  </p>
                </div>
              {/if}

              <!-- Codex transport: app-server unlocks interactive tools -->
              {#if settings}
                <label
                  class="flex items-start gap-3 rounded-lg border border-border/50 p-4 cursor-pointer hover:bg-accent/40"
                >
                  <input
                    type="checkbox"
                    class="mt-0.5 rounded"
                    checked={settings.codex_transport !== "exec"}
                    onchange={async (e) => {
                      const on = (e.currentTarget as HTMLInputElement).checked;
                      settings = await api.updateUserSettings({
                        codex_transport: on ? "app_server" : "exec",
                      } as Partial<UserSettings>);
                    }}
                  />
                  <span>
                    <span class="font-medium text-sm">{t("settings_codexTransport_label")}</span>
                    <span class="block text-xs text-muted-foreground mt-0.5"
                      >{t("settings_codexTransport_desc")}</span
                    >
                  </span>
                </label>
              {/if}

              <!-- App Provider details: point Codex at a third-party Responses provider -->
              {#if codexAuthMode === "app"}
                <div class="space-y-3 rounded-lg border border-border/50 p-4">
                  <p class="text-xs text-muted-foreground">
                    {t("settings_codexProvider_desc")}
                  </p>
                  <!-- Provider grid. No "None" tile — CLI Auth above already is None. -->
                  <div class="grid grid-cols-2 gap-2 sm:grid-cols-3">
                    {#each CODEX_PROVIDER_PRESETS as preset (preset.id)}
                      <button
                        class="rounded-md border p-3 text-left transition-colors
                          {codexProviderId === preset.id
                          ? 'border-primary bg-primary/5'
                          : 'hover:bg-accent hover:border-ring/30'}"
                        onclick={() => selectCodexProvider(preset)}
                      >
                        <p class="text-sm font-medium truncate">{preset.name}</p>
                        <p class="text-xs text-muted-foreground mt-0.5 truncate">
                          {preset.description}
                        </p>
                      </button>
                    {/each}
                  </div>

                  {#if codexProviderPreset}
                    <div class="space-y-3 rounded-md border border-border/60 p-4">
                      <!-- Base URL (editable for custom, shown read-only otherwise) -->
                      <div>
                        <span class="text-xs font-medium text-muted-foreground">Base URL</span>
                        {#if codexProviderPreset.custom}
                          <input
                            class="mt-1 w-full rounded-md border px-3 py-2 text-sm bg-background font-mono"
                            placeholder="https://host/v1"
                            bind:value={codexProviderBaseUrl}
                          />
                        {:else}
                          <p class="text-xs font-mono text-muted-foreground mt-1">
                            {codexProviderBaseUrl || codexProviderPreset.base_url}
                          </p>
                        {/if}
                      </div>

                      <!-- Model -->
                      <div>
                        <span class="text-xs font-medium text-muted-foreground"
                          >{t("settings_codexProvider_model")}</span
                        >
                        <input
                          class="mt-1 w-full rounded-md border px-3 py-2 text-sm bg-background font-mono"
                          placeholder="gpt-5.1"
                          bind:value={codexProviderModel}
                        />
                      </div>

                      <!-- API key (skipped for keyless local providers) -->
                      {#if !codexProviderPreset.keyless}
                        <div>
                          <span class="text-xs font-medium text-muted-foreground"
                            >{t("settings_codexProvider_apiKey")}</span
                          >
                          <input
                            type="password"
                            class="mt-1 w-full rounded-md border px-3 py-2 text-sm bg-background font-mono"
                            placeholder={codexProviderPreset.key_placeholder}
                            bind:value={codexProviderKey}
                          />
                          <p class="text-[11px] text-muted-foreground/70 mt-1">
                            {t("settings_codexProvider_keyEnvNote", {
                              env: codexProviderPreset.env_key,
                            })}
                          </p>
                        </div>
                      {/if}

                      <div class="flex justify-end">
                        <button
                          class="rounded-md bg-primary px-3 py-1.5 text-xs font-medium text-primary-foreground hover:bg-primary/90"
                          onclick={() => saveCodexProvider()}
                        >
                          {t("settings_codexProvider_save")}
                        </button>
                      </div>
                    </div>
                  {/if}
                </div>
              {/if}

              <!-- Pointer: Codex model/sandbox config lives in the CLI Config tab, not here -->
              <p class="text-xs text-muted-foreground/80">
                {t("settings_codex_modelHint")}
              </p>
              <!-- Codex Hooks shortcut -->
              {#if codexHooksCount !== null}
                <div class="flex items-center gap-3 pt-3 border-t border-border/50">
                  <div class="flex h-8 w-8 items-center justify-center rounded-full bg-muted">
                    <svg
                      class="h-4 w-4 text-muted-foreground"
                      viewBox="0 0 24 24"
                      fill="none"
                      stroke="currentColor"
                      stroke-width="2"
                      stroke-linecap="round"
                      stroke-linejoin="round"
                    >
                      <path d="M18 8h1a4 4 0 0 1 0 8h-1" /><path
                        d="M2 8h16v9a4 4 0 0 1-4 4H6a4 4 0 0 1-4-4V8z"
                      /><line x1="6" y1="1" x2="6" y2="4" /><line
                        x1="10"
                        y1="1"
                        x2="10"
                        y2="4"
                      /><line x1="14" y1="1" x2="14" y2="4" />
                    </svg>
                  </div>
                  <div class="flex-1">
                    <p class="text-sm font-medium">{t("settings_codexHooks_label")}</p>
                    <p class="text-xs text-muted-foreground">
                      {t("settings_codexHooks_count", { count: String(codexHooksCount) })}
                    </p>
                  </div>
                  <button
                    class="text-xs text-primary hover:underline"
                    onclick={() => goto("/plugins?section=hooks")}
                  >
                    {t("settings_codexHooks_manage")}
                  </button>
                </div>
              {/if}
            </div>
          {/if}
        </Card>

        <!-- Setup Wizard button -->
        <div class="flex items-center justify-between rounded-lg border border-border p-4">
          <div>
            <p class="text-sm font-medium">{t("settings_general_setupWizard")}</p>
            <p class="text-xs text-muted-foreground">{t("settings_general_setupWizardDesc")}</p>
          </div>
          <button
            class="rounded-md border px-3 py-1.5 text-xs text-muted-foreground hover:text-foreground hover:bg-accent transition-colors"
            onclick={openSetupWizard}>{t("settings_general_runWizard")}</button
          >
        </div>
      </div>

      <!-- ═══ CLI Config tab ═══ -->
    {:else if activeTab === "cli-config"}
      {#if cliConfigLoading && !cliConfigLoaded}
        <div class="flex items-center justify-center py-12">
          <div
            class="h-5 w-5 animate-spin rounded-full border-2 border-primary border-t-transparent"
          ></div>
          <span class="ml-3 text-sm text-muted-foreground">{t("settings_cliConfig_loading")}</span>
        </div>
      {:else if cliConfigError}
        <Card class="p-6">
          <p class="text-sm text-red-400">
            {t("settings_cliConfig_loadFailed", { error: cliConfigError })}
          </p>
          <button
            class="mt-3 rounded-md border px-3 py-1.5 text-xs hover:bg-accent transition-colors"
            onclick={() => {
              cliConfigLoaded = false;
              loadCliConfig();
            }}
          >
            {t("settings_cliConfig_retry")}
          </button>
        </Card>
      {:else}
        <div class="space-y-6">
          <!-- Claude group -->
          <h2
            class="text-xs font-semibold text-muted-foreground/60 uppercase tracking-widest pt-1 border-b border-border/40 pb-2"
          >
            {t("settings_cliConfig_claudeGroup")}
          </h2>
          <!-- Behavior -->
          <Card class="p-6 space-y-4">
            <h2 class="text-sm font-semibold text-muted-foreground uppercase tracking-wider">
              {t("settings_cliConfig_behavior")}
            </h2>
            {#each behaviorSettings as def (def.key)}
              <div class="flex items-center justify-between gap-4 py-1">
                <div class="flex-1 min-w-0">
                  <div class="flex items-center gap-2">
                    <p class="text-sm font-medium">{def.label}</p>
                    {#if isProjectOverride(def.key)}
                      <span
                        class="inline-flex items-center rounded px-1.5 py-0.5 text-[10px] font-medium bg-amber-500/15 text-amber-400 border border-amber-500/20"
                      >
                        {t("settings_cliConfig_projectOverride")}
                      </span>
                    {/if}
                  </div>
                  <p class="text-xs text-muted-foreground mt-0.5">{def.description}</p>
                </div>
                {#if def.type === "boolean"}
                  <button
                    aria-label={def.label}
                    class="relative inline-flex h-6 w-11 shrink-0 items-center rounded-full transition-colors duration-200 {getCliConfigValue(
                      def.key,
                      def,
                    ) === true
                      ? 'bg-primary'
                      : 'bg-neutral-700'}"
                    onclick={() => {
                      const current = getCliConfigValue(def.key, def);
                      const next = current === true ? false : true;
                      saveCliConfigPatch(def.key, next);
                      cliConfig = { ...cliConfig, [def.key]: next };
                    }}
                  >
                    <span
                      class="inline-block h-4 w-4 transform rounded-full bg-white transition-transform duration-200 {getCliConfigValue(
                        def.key,
                        def,
                      ) === true
                        ? 'translate-x-6'
                        : 'translate-x-1'}"
                    ></span>
                  </button>
                {:else if def.type === "enum" && def.options}
                  <div class="flex gap-1.5 shrink-0">
                    {#each def.options as opt (opt.value)}
                      <button
                        class="rounded-md border px-3 py-1.5 text-xs transition-all duration-150
                        {getCliConfigValue(def.key, def) === opt.value
                          ? 'bg-primary text-primary-foreground'
                          : 'hover:bg-accent hover:border-ring/30'}"
                        onclick={() => {
                          saveCliConfigPatch(def.key, opt.value);
                          cliConfig = { ...cliConfig, [def.key]: opt.value };
                        }}
                      >
                        {opt.label}
                      </button>
                    {/each}
                  </div>
                {/if}
              </div>
            {/each}
          </Card>

          <!-- Appearance -->
          <Card class="p-6 space-y-4">
            <h2 class="text-sm font-semibold text-muted-foreground uppercase tracking-wider">
              {t("settings_cliConfig_appearance")}
            </h2>
            {#each appearanceSettings as def (def.key)}
              <div class="flex items-center justify-between gap-4 py-1">
                <div class="flex-1 min-w-0">
                  <div class="flex items-center gap-2">
                    <p class="text-sm font-medium">{def.label}</p>
                    {#if isProjectOverride(def.key)}
                      <span
                        class="inline-flex items-center rounded px-1.5 py-0.5 text-[10px] font-medium bg-amber-500/15 text-amber-400 border border-amber-500/20"
                      >
                        {t("settings_cliConfig_projectOverride")}
                      </span>
                    {/if}
                  </div>
                  <p class="text-xs text-muted-foreground mt-0.5">{def.description}</p>
                </div>
                {#if def.type === "boolean"}
                  <button
                    aria-label={def.label}
                    class="relative inline-flex h-6 w-11 shrink-0 items-center rounded-full transition-colors duration-200 {getCliConfigValue(
                      def.key,
                      def,
                    ) === true
                      ? 'bg-primary'
                      : 'bg-neutral-700'}"
                    onclick={() => {
                      const current = getCliConfigValue(def.key, def);
                      const next = current === true ? false : true;
                      saveCliConfigPatch(def.key, next);
                      cliConfig = { ...cliConfig, [def.key]: next };
                    }}
                  >
                    <span
                      class="inline-block h-4 w-4 transform rounded-full bg-white transition-transform duration-200 {getCliConfigValue(
                        def.key,
                        def,
                      ) === true
                        ? 'translate-x-6'
                        : 'translate-x-1'}"
                    ></span>
                  </button>
                {:else if def.type === "enum" && def.options}
                  <div class="flex gap-1.5 shrink-0">
                    {#each def.options as opt (opt.value)}
                      <button
                        class="rounded-md border px-3 py-1.5 text-xs transition-all duration-150
                        {getCliConfigValue(def.key, def) === opt.value
                          ? 'bg-primary text-primary-foreground'
                          : 'hover:bg-accent hover:border-ring/30'}"
                        onclick={() => {
                          saveCliConfigPatch(def.key, opt.value);
                          cliConfig = { ...cliConfig, [def.key]: opt.value };
                        }}
                      >
                        {opt.label}
                      </button>
                    {/each}
                  </div>
                {:else if def.type === "string"}
                  <input
                    class="w-40 shrink-0 rounded-md border bg-transparent px-3 py-1.5 text-sm placeholder:text-muted-foreground focus:border-ring focus:outline-none"
                    value={getCliConfigValue(def.key, def) ?? ""}
                    placeholder={def.label}
                    onblur={(e) => {
                      const val = (e.target as HTMLInputElement).value.trim();
                      if (val) {
                        saveCliConfigPatch(def.key, val);
                        cliConfig = { ...cliConfig, [def.key]: val };
                      } else {
                        // Empty string → delete key (restore default)
                        saveCliConfigPatch(def.key, null);
                        const next = { ...cliConfig };
                        delete next[def.key];
                        cliConfig = next;
                      }
                    }}
                  />
                {/if}
              </div>
            {/each}
          </Card>

          <!-- Advanced -->
          <Card class="p-6 space-y-4">
            <h2 class="text-sm font-semibold text-muted-foreground uppercase tracking-wider">
              {t("settings_cliConfig_advanced")}
            </h2>
            {#each advancedSettings as def (def.key)}
              <div class="flex items-center justify-between gap-4 py-1">
                <div class="flex-1 min-w-0">
                  <div class="flex items-center gap-2">
                    <p class="text-sm font-medium">{def.label}</p>
                    {#if isProjectOverride(def.key)}
                      <span
                        class="inline-flex items-center rounded px-1.5 py-0.5 text-[10px] font-medium bg-amber-500/15 text-amber-400 border border-amber-500/20"
                      >
                        {t("settings_cliConfig_projectOverride")}
                      </span>
                    {/if}
                  </div>
                  <p class="text-xs text-muted-foreground mt-0.5">{def.description}</p>
                </div>
                {#if def.type === "boolean"}
                  <button
                    aria-label={def.label}
                    class="relative inline-flex h-6 w-11 shrink-0 items-center rounded-full transition-colors duration-200 {getCliConfigValue(
                      def.key,
                      def,
                    ) === true
                      ? 'bg-primary'
                      : 'bg-neutral-700'}"
                    onclick={() => {
                      const current = getCliConfigValue(def.key, def);
                      const next = current === true ? false : true;
                      saveCliConfigPatch(def.key, next);
                      cliConfig = { ...cliConfig, [def.key]: next };
                    }}
                  >
                    <span
                      class="inline-block h-4 w-4 transform rounded-full bg-white transition-transform duration-200 {getCliConfigValue(
                        def.key,
                        def,
                      ) === true
                        ? 'translate-x-6'
                        : 'translate-x-1'}"
                    ></span>
                  </button>
                {:else if def.type === "enum" && def.options}
                  <div class="flex gap-1.5 shrink-0">
                    {#each def.options as opt (opt.value)}
                      <button
                        class="rounded-md border px-3 py-1.5 text-xs transition-all duration-150
                        {getCliConfigValue(def.key, def) === opt.value
                          ? 'bg-primary text-primary-foreground'
                          : 'hover:bg-accent hover:border-ring/30'}"
                        onclick={() => {
                          saveCliConfigPatch(def.key, opt.value);
                          cliConfig = { ...cliConfig, [def.key]: opt.value };
                        }}
                      >
                        {opt.label}
                      </button>
                    {/each}
                  </div>
                {/if}
              </div>
            {/each}
          </Card>

          <!-- Footer note -->
          <p class="text-[10px] text-muted-foreground px-1">
            {t("settings_cliConfig_footer")}
          </p>

          <!-- Codex group -->
          <h2
            class="text-xs font-semibold text-muted-foreground/60 uppercase tracking-widest pt-3 border-b border-border/40 pb-2"
          >
            {t("settings_cliConfig_codexGroup")}
          </h2>

          <!-- ── Codex Config ── -->
          <Card class="p-6 space-y-4">
            <h2 class="text-sm font-semibold text-muted-foreground uppercase tracking-wider">
              {t("settings_codexConfig_title")}
            </h2>

            {#if codexConfigWarning}
              <div
                class="flex items-start gap-2 rounded-md border border-amber-500/30 bg-amber-500/10 px-3 py-2"
              >
                <svg
                  class="h-4 w-4 text-amber-400 shrink-0 mt-0.5"
                  fill="none"
                  viewBox="0 0 24 24"
                  stroke="currentColor"
                  stroke-width="2"
                >
                  <path
                    stroke-linecap="round"
                    stroke-linejoin="round"
                    d="M12 9v2m0 4h.01m-6.938 4h13.856c1.54 0 2.502-1.667 1.732-3L13.732 4c-.77-1.333-2.694-1.333-3.464 0L3.34 16c-.77 1.333.192 3 1.732 3z"
                  />
                </svg>
                <div class="text-xs text-amber-300">
                  <p>{t("settings_codexConfig_warning", { warning: codexConfigWarning })}</p>
                  <p class="mt-0.5 text-amber-400/70">
                    {t("settings_codexConfig_warningDisabled")}
                  </p>
                </div>
              </div>
            {/if}

            {#each CODEX_CONFIG_SETTINGS as def (def.key)}
              {@const unknownDisplay = codexValueDisplay(def.key, def)}
              <div class="flex items-center justify-between gap-4 py-1">
                <div class="flex-1 min-w-0">
                  <div class="flex items-center gap-2">
                    <p class="text-sm font-medium">{def.label}</p>
                    {#if isCodexProjectOverride(def.key)}
                      <span
                        class="inline-flex items-center rounded px-1.5 py-0.5 text-[10px] font-medium bg-amber-500/15 text-amber-400 border border-amber-500/20"
                      >
                        {t("settings_codexConfig_projectOverrideApprox")}
                      </span>
                    {/if}
                  </div>
                  <p class="text-xs text-muted-foreground mt-0.5">{def.description}</p>
                </div>
                {#if def.type === "enum" && def.options}
                  <div class="flex items-center gap-1.5 shrink-0">
                    {#each def.options as opt (opt.value)}
                      <button
                        class="rounded-md border px-3 py-1.5 text-xs transition-all duration-150
                        {getCodexConfigValue(def.key, def) === opt.value
                          ? 'bg-primary text-primary-foreground'
                          : 'hover:bg-accent hover:border-ring/30'}
                        {codexConfigWarning ? 'opacity-50 cursor-not-allowed' : ''}"
                        disabled={!!codexConfigWarning}
                        onclick={() => {
                          saveCodexConfigPatch(def.key, opt.value);
                          codexConfig = { ...codexConfig, [def.key]: opt.value };
                        }}
                      >
                        {opt.label}
                      </button>
                    {/each}
                    {#if unknownDisplay}
                      <span class="text-[10px] text-amber-400 ml-1">{unknownDisplay}</span>
                    {/if}
                  </div>
                {:else if def.type === "string"}
                  <input
                    class="w-40 shrink-0 rounded-md border bg-transparent px-3 py-1.5 text-sm placeholder:text-muted-foreground focus:border-ring focus:outline-none
                    {codexConfigWarning ? 'opacity-50 cursor-not-allowed' : ''}"
                    disabled={!!codexConfigWarning}
                    value={getCodexConfigValue(def.key, def) ?? ""}
                    placeholder={t("settings_codexConfig_modelPlaceholder")}
                    onblur={(e) => {
                      const val = (e.target as HTMLInputElement).value.trim();
                      if (val) {
                        saveCodexConfigPatch(def.key, val);
                        codexConfig = { ...codexConfig, [def.key]: val };
                      } else {
                        saveCodexConfigPatch(def.key, null);
                        const next = { ...codexConfig };
                        delete next[def.key];
                        codexConfig = next;
                      }
                    }}
                  />
                {/if}
              </div>
            {/each}
          </Card>

          <!-- Codex footer note -->
          <p class="text-[10px] text-muted-foreground px-1">
            {t("settings_codexConfig_footer")}
          </p>

          <!-- Codex per-session flags (OpenCovibe layer, overrides config.toml) -->
          {#if codexAgentSettings}
            <Card class="p-5 space-y-4">
              <div class="flex items-center justify-between gap-2">
                <div>
                  <h3 class="text-sm font-semibold">
                    {t("settings_codexFlags_title")}
                  </h3>
                  <p class="text-xs text-muted-foreground mt-0.5">
                    {t("settings_codexFlags_subtitle")}
                  </p>
                </div>
              </div>

              <!-- Reasoning effort (per-session override) -->
              <div class="flex items-center justify-between gap-4 py-1">
                <div class="flex-1 min-w-0">
                  <p class="text-sm font-medium">{t("settings_codexFlags_effortLabel")}</p>
                  <p class="text-xs text-muted-foreground mt-0.5">
                    {t("settings_codexFlags_effortDesc")}
                  </p>
                </div>
                <div class="flex items-center gap-1.5 shrink-0 flex-wrap">
                  {#each [{ val: "", labelKey: "settings_codexConfig_optInherit" }, { val: "none", labelKey: "settings_codexConfig_optNone" }, { val: "minimal", labelKey: "settings_codexConfig_optMinimal" }, { val: "low", labelKey: "settings_codexConfig_optLow" }, { val: "medium", labelKey: "settings_codexConfig_optMedium" }, { val: "high", labelKey: "settings_codexConfig_optHigh" }, { val: "xhigh", labelKey: "settings_codexConfig_optXHigh" }] as opt (opt.val)}
                    <button
                      class="rounded-md border px-3 py-1.5 text-xs transition-all duration-150
                        {(codexAgentSettings.effort ?? '') === opt.val
                        ? 'bg-primary text-primary-foreground'
                        : 'hover:bg-accent hover:border-ring/30'}"
                      onclick={() =>
                        saveCodexAgentPatch({
                          effort: opt.val === "" ? null : opt.val,
                        } as Partial<AgentSettings>)}
                    >
                      {t(opt.labelKey as Parameters<typeof t>[0])}
                    </button>
                  {/each}
                </div>
              </div>

              <!-- Profile -->
              <div class="flex items-center justify-between gap-4 py-1">
                <div class="flex-1 min-w-0">
                  <p class="text-sm font-medium">{t("settings_codexFlags_profileLabel")}</p>
                  <p class="text-xs text-muted-foreground mt-0.5">
                    {t("settings_codexFlags_profileDesc")}
                  </p>
                </div>
                <input
                  class="w-40 shrink-0 rounded-md border bg-transparent px-3 py-1.5 text-sm placeholder:text-muted-foreground focus:border-ring focus:outline-none"
                  value={codexAgentSettings.profile ?? ""}
                  placeholder={t("settings_codexFlags_profilePlaceholder")}
                  onblur={(e) => {
                    const val = (e.target as HTMLInputElement).value.trim();
                    saveCodexAgentPatch({ profile: val });
                  }}
                />
              </div>

              <!-- Ephemeral toggle -->
              <div class="flex items-center justify-between gap-4 py-1">
                <div class="flex-1 min-w-0">
                  <p class="text-sm font-medium">{t("settings_codexFlags_ephemeralLabel")}</p>
                  <p class="text-xs text-muted-foreground mt-0.5">
                    {t("settings_codexFlags_ephemeralDesc")}
                  </p>
                </div>
                <button
                  class="rounded-md border px-3 py-1.5 text-xs shrink-0 transition-all
                    {codexAgentSettings.ephemeral
                    ? 'bg-primary text-primary-foreground'
                    : 'hover:bg-accent hover:border-ring/30'}"
                  onclick={() => saveCodexAgentPatch({ ephemeral: !codexAgentSettings?.ephemeral })}
                >
                  {codexAgentSettings.ephemeral
                    ? t("settings_codexFlags_on")
                    : t("settings_codexFlags_off")}
                </button>
              </div>

              <!-- Ignore user config -->
              <div class="flex items-center justify-between gap-4 py-1">
                <div class="flex-1 min-w-0">
                  <p class="text-sm font-medium">
                    {t("settings_codexFlags_ignoreUserConfigLabel")}
                  </p>
                  <p class="text-xs text-amber-400/80 mt-0.5">
                    {t("settings_codexFlags_ignoreUserConfigDesc")}
                  </p>
                </div>
                <button
                  class="rounded-md border px-3 py-1.5 text-xs shrink-0 transition-all
                    {codexAgentSettings.ignore_user_config
                    ? 'bg-primary text-primary-foreground'
                    : 'hover:bg-accent hover:border-ring/30'}"
                  onclick={() =>
                    saveCodexAgentPatch({
                      ignore_user_config: !codexAgentSettings?.ignore_user_config,
                    })}
                >
                  {codexAgentSettings.ignore_user_config
                    ? t("settings_codexFlags_on")
                    : t("settings_codexFlags_off")}
                </button>
              </div>

              <!-- Ignore rules -->
              <div class="flex items-center justify-between gap-4 py-1">
                <div class="flex-1 min-w-0">
                  <p class="text-sm font-medium">
                    {t("settings_codexFlags_ignoreRulesLabel")}
                  </p>
                  <p class="text-xs text-amber-400/80 mt-0.5">
                    {t("settings_codexFlags_ignoreRulesDesc")}
                  </p>
                </div>
                <button
                  class="rounded-md border px-3 py-1.5 text-xs shrink-0 transition-all
                    {codexAgentSettings.ignore_rules
                    ? 'bg-primary text-primary-foreground'
                    : 'hover:bg-accent hover:border-ring/30'}"
                  onclick={() =>
                    saveCodexAgentPatch({ ignore_rules: !codexAgentSettings?.ignore_rules })}
                >
                  {codexAgentSettings.ignore_rules
                    ? t("settings_codexFlags_on")
                    : t("settings_codexFlags_off")}
                </button>
              </div>

              <!-- Web search -->
              <div class="flex items-center justify-between gap-4 py-1">
                <div class="flex-1 min-w-0">
                  <p class="text-sm font-medium">{t("settings_codexFlags_webSearchLabel")}</p>
                  <p class="text-xs text-muted-foreground mt-0.5">
                    {t("settings_codexFlags_webSearchDesc")}
                  </p>
                </div>
                <button
                  class="rounded-md border px-3 py-1.5 text-xs shrink-0 transition-all
                    {codexAgentSettings.web_search
                    ? 'bg-primary text-primary-foreground'
                    : 'hover:bg-accent hover:border-ring/30'}"
                  onclick={() =>
                    saveCodexAgentPatch({ web_search: !codexAgentSettings?.web_search })}
                >
                  {codexAgentSettings.web_search
                    ? t("settings_codexFlags_on")
                    : t("settings_codexFlags_off")}
                </button>
              </div>
            </Card>
          {/if}
        </div>
      {/if}

      <!-- ═══ Shortcuts tab ═══ -->
    {:else if activeTab === "shortcuts"}
      <div class="space-y-6">
        <!-- App shortcuts (editable) -->
        <Card class="p-6 space-y-5">
          <h2 class="text-sm font-semibold text-muted-foreground uppercase tracking-wider">
            {t("settings_shortcuts_appShortcuts")}
          </h2>
          <div class="divide-y divide-border/50">
            {#each appBindings as binding (binding.command)}
              <KeybindingEditor
                {binding}
                isOverridden={isOverridden(binding.command)}
                conflictWarning={recordingConflict}
                onSave={(key) => {
                  const conflict = getConflictWarning(key, binding.context, binding.command);
                  if (conflict) {
                    recordingConflict = conflict;
                  }
                  keybindingStore.setOverride(binding.command, key);
                  recordingConflict = "";
                }}
                onReset={isOverridden(binding.command)
                  ? () => keybindingStore.resetBinding(binding.command)
                  : undefined}
              />
            {/each}
          </div>
        </Card>

        <!-- Fixed shortcuts -->
        <Card class="p-6 space-y-5">
          <h2 class="text-sm font-semibold text-muted-foreground uppercase tracking-wider">
            {t("settings_shortcuts_inputFixed")}
          </h2>
          <div class="divide-y divide-border/50">
            {#each fixedBindings as binding (binding.command)}
              <div class="flex items-center gap-3 py-1.5">
                <span class="text-sm text-foreground/60 min-w-[140px]">{binding.label}</span>
                <span
                  class="inline-flex items-center rounded-md border bg-muted/30 px-2.5 py-1 text-xs font-mono text-muted-foreground min-w-[60px] justify-center"
                >
                  {formatKeyDisplay(binding.key)}
                </span>
              </div>
            {/each}
          </div>
        </Card>

        <!-- CLI shortcuts (collapsible) -->
        <Card class="p-6 space-y-4">
          <button
            class="flex items-center gap-2 text-sm font-semibold text-muted-foreground uppercase tracking-wider hover:text-foreground transition-colors w-full"
            onclick={() => (cliSectionOpen = !cliSectionOpen)}
          >
            <svg
              class="h-3 w-3 transition-transform {cliSectionOpen ? 'rotate-90' : ''}"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              stroke-width="2"
              stroke-linecap="round"
              stroke-linejoin="round"><path d="m9 18 6-6-6-6" /></svg
            >
            {t("settings_shortcuts_cliShortcuts")}
            <span class="text-[10px] font-normal normal-case tracking-normal text-muted-foreground"
              >{t("settings_shortcuts_readOnly")}</span
            >
          </button>
          {#if cliSectionOpen}
            <div class="divide-y divide-border/50">
              {#each cliBindings as binding (binding.command)}
                <div class="flex items-center gap-3 py-1.5">
                  <span class="text-sm text-foreground/60 min-w-[140px]">{binding.label}</span>
                  <span
                    class="inline-flex items-center rounded-md border bg-muted/30 px-2.5 py-1 text-xs font-mono text-muted-foreground min-w-[60px] justify-center"
                  >
                    {formatKeyDisplay(binding.key)}
                  </span>
                </div>
              {/each}
            </div>
            <p class="text-[10px] text-muted-foreground">
              {t("settings_shortcuts_source", {
                source:
                  cliSource === "file"
                    ? IS_WINDOWS
                      ? "%USERPROFILE%\\.claude\\keybindings.json"
                      : "~/.claude/keybindings.json"
                    : t("settings_shortcuts_cliDefaults"),
              })}
            </p>
          {/if}
        </Card>

        <!-- Codex CLI shortcuts (collapsible, read-only) -->
        <Card class="p-6 space-y-4">
          <button
            class="flex items-center gap-2 text-sm font-semibold text-muted-foreground uppercase tracking-wider hover:text-foreground transition-colors w-full"
            onclick={() => (codexCliSectionOpen = !codexCliSectionOpen)}
          >
            <svg
              class="h-3 w-3 transition-transform {codexCliSectionOpen ? 'rotate-90' : ''}"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              stroke-width="2"
              stroke-linecap="round"
              stroke-linejoin="round"><path d="m9 18 6-6-6-6" /></svg
            >
            {t("settings_shortcuts_codexCliShortcuts")}
            <span class="text-[10px] font-normal normal-case tracking-normal text-muted-foreground"
              >{t("settings_shortcuts_readOnly")}</span
            >
          </button>
          {#if codexCliSectionOpen}
            <div class="divide-y divide-border/50">
              {#each codexCliBindings as binding (binding.command)}
                <div class="flex items-center gap-3 py-1.5">
                  <span class="text-sm text-foreground/60 min-w-[140px]">{binding.label}</span>
                  <span
                    class="inline-flex items-center rounded-md border bg-muted/30 px-2.5 py-1 text-xs font-mono text-muted-foreground min-w-[60px] justify-center"
                  >
                    {formatKeyDisplay(binding.key)}
                  </span>
                </div>
              {/each}
            </div>
          {/if}
        </Card>

        <!-- Reset all -->
        {#if hasOverrides}
          <div class="flex justify-end">
            <button
              class="rounded-md border px-3 py-1.5 text-xs text-muted-foreground hover:text-foreground hover:bg-accent transition-colors"
              onclick={() => keybindingStore.resetAll()}
            >
              {t("settings_shortcuts_resetAll")}
            </button>
          </div>
        {/if}
      </div>

      <!-- ═══ Remote tab ═══ -->
    {:else if activeTab === "remote"}
      <Card class="p-6 space-y-5">
        <div class="flex items-start justify-between">
          <div>
            <p class="text-sm font-medium">{t("settings_remote_title")}</p>
            <p class="text-xs text-muted-foreground mt-0.5">
              {t("settings_remote_desc")}
            </p>
          </div>
          {#if remoteSaved}
            <span class="text-xs text-emerald-500 flex items-center gap-1 animate-fade-in">
              <svg
                class="h-3 w-3"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                stroke-width="2"
                stroke-linecap="round"
                stroke-linejoin="round"><path d="M20 6 9 17l-5-5" /></svg
              >
              {t("settings_general_saved")}
            </span>
          {/if}
        </div>

        <!-- Claude-only notice: Codex has no remote-SSH support -->
        <div
          class="flex items-start gap-2 rounded-md border border-amber-500/30 bg-amber-500/10 px-3 py-2"
        >
          <svg
            class="h-4 w-4 shrink-0 text-amber-400 mt-0.5"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            stroke-linejoin="round"
          >
            <circle cx="12" cy="12" r="10" /><line x1="12" y1="8" x2="12" y2="12" /><line
              x1="12"
              y1="16"
              x2="12.01"
              y2="16"
            />
          </svg>
          <p class="text-xs text-amber-200/90">{t("settings_remote_claudeOnly")}</p>
        </div>

        <!-- Existing hosts list -->
        {#if remoteHosts.length > 0}
          <div class="space-y-2">
            {#each remoteHosts as host (host.name)}
              <div
                class="flex items-center justify-between p-3 bg-muted/50 rounded-lg border border-border"
              >
                <div>
                  <p class="text-sm font-medium">{host.name}</p>
                  <p class="text-xs text-muted-foreground">
                    {host.user}@{host.host}{host.port !== 22 ? `:${host.port}` : ""}
                  </p>
                  {#if host.remote_cwd}
                    <p class="text-xs text-muted-foreground">cwd: {host.remote_cwd}</p>
                  {/if}
                </div>
                <div class="flex gap-2">
                  <button
                    class="text-xs px-2 py-1 rounded hover:bg-accent text-muted-foreground"
                    onclick={() => editRemoteHost(host)}>{t("settings_remote_edit")}</button
                  >
                  <button
                    class="text-xs px-2 py-1 rounded hover:bg-destructive/10 text-destructive"
                    onclick={() => deleteRemoteHost(host.name)}
                    >{t("settings_remote_delete")}</button
                  >
                </div>
              </div>
            {/each}
          </div>
        {:else}
          <p class="text-xs text-muted-foreground italic">{t("settings_remote_noHosts")}</p>
        {/if}

        <!-- Add / Edit form -->
        <div class="border border-border rounded-lg p-4 space-y-3">
          <p class="text-sm font-medium">
            {editingRemote
              ? t("settings_remote_editHost", { name: editingRemote.name })
              : t("settings_remote_addHost")}
          </p>

          <div class="grid grid-cols-2 gap-3">
            <label class="block">
              <span class="text-xs text-muted-foreground block mb-1"
                >{t("settings_remote_name")} *</span
              >
              <input
                type="text"
                bind:value={remoteFormName}
                placeholder="mac-mini"
                class="w-full text-sm px-2 py-1.5 rounded border bg-background {remoteFormTouched &&
                !remoteFormName.trim()
                  ? 'border-red-500'
                  : 'border-input'}"
              />
            </label>
            <label class="block">
              <span class="text-xs text-muted-foreground block mb-1"
                >{t("settings_remote_host")} *</span
              >
              <input
                type="text"
                bind:value={remoteFormHost}
                placeholder="macmini.local"
                class="w-full text-sm px-2 py-1.5 rounded border bg-background {remoteFormTouched &&
                !remoteFormHost.trim()
                  ? 'border-red-500'
                  : 'border-input'}"
              />
            </label>
            <label class="block">
              <span class="text-xs text-muted-foreground block mb-1"
                >{t("settings_remote_user")} *</span
              >
              <input
                type="text"
                bind:value={remoteFormUser}
                placeholder={currentUsername || "username"}
                class="w-full text-sm px-2 py-1.5 rounded border bg-background {remoteFormTouched &&
                !remoteFormUser.trim()
                  ? 'border-red-500'
                  : 'border-input'}"
              />
            </label>
            <label class="block">
              <span class="text-xs text-muted-foreground block mb-1"
                >{t("settings_remote_port")}</span
              >
              <input
                type="number"
                bind:value={remoteFormPort}
                placeholder="22"
                class="w-full text-sm px-2 py-1.5 rounded border border-input bg-background"
              />
            </label>
            <div class="col-span-2">
              <span class="text-xs text-muted-foreground block mb-1"
                >{t("settings_remote_keyPath")}</span
              >
              <div class="flex gap-2">
                <input
                  type="text"
                  aria-label={t("settings_remote_keyPath")}
                  bind:value={remoteFormKeyPath}
                  placeholder="~/.ssh/id_ed25519"
                  class="flex-1 text-sm px-2 py-1.5 rounded border border-input bg-background"
                />
                {#if sshKeyStep === "idle"}
                  <button
                    class="shrink-0 text-xs px-2 py-1.5 rounded border border-input hover:bg-accent transition-colors text-muted-foreground"
                    onclick={startSshKeyWizard}
                  >
                    {t("settings_remote_setupSshKey")}
                  </button>
                {/if}
              </div>

              <!-- SSH Key Wizard inline panel -->
              {#if sshKeyStep !== "idle"}
                <div class="mt-2 rounded-lg border border-border p-3 space-y-2 text-xs bg-muted/30">
                  {#if sshKeyStep === "checking"}
                    <div class="flex items-center gap-2 text-muted-foreground">
                      <div
                        class="h-3.5 w-3.5 animate-spin rounded-full border-2 border-primary border-t-transparent"
                      ></div>
                      {t("settings_remote_sshKeyChecking")}
                    </div>
                  {:else if sshKeyStep === "no_key"}
                    <p class="text-muted-foreground">{t("settings_remote_sshKeyNotFound")}</p>
                    <button
                      class="rounded border px-3 py-1.5 text-xs hover:bg-accent transition-colors"
                      onclick={generateSshKey}
                    >
                      {t("settings_remote_sshKeyGenerate")}
                    </button>
                  {:else if sshKeyStep === "generating"}
                    <div class="flex items-center gap-2 text-muted-foreground">
                      <div
                        class="h-3.5 w-3.5 animate-spin rounded-full border-2 border-primary border-t-transparent"
                      ></div>
                      {t("settings_remote_sshKeyGenerating")}
                    </div>
                  {:else if sshKeyStep === "pub_missing" && sshKeyInfo}
                    <p class="text-amber-400">
                      {t(
                        IS_WINDOWS
                          ? "settings_remote_sshKeyPubMissing_win"
                          : "settings_remote_sshKeyPubMissing",
                      )}
                    </p>
                    <div class="flex items-center gap-2">
                      <code
                        class="flex-1 rounded bg-muted px-2 py-1.5 font-mono text-[11px] break-all select-all"
                      >
                        {buildRebuildPubKeyCommand(sshKeyInfo)}
                      </code>
                      <button
                        class="shrink-0 rounded border px-2 py-1 text-[10px] hover:bg-accent transition-colors"
                        onclick={async () => {
                          await navigator.clipboard.writeText(
                            buildRebuildPubKeyCommand(sshKeyInfo!),
                          );
                          sshCopied = true;
                          setTimeout(() => (sshCopied = false), 2000);
                        }}
                      >
                        {sshCopied ? t("settings_remote_sshKeyCopied") : t("common_copy")}
                      </button>
                    </div>
                    <p class="text-muted-foreground text-[10px]">
                      After running the command, click "Setup SSH Key" again.
                    </p>
                    <button
                      class="text-[10px] text-muted-foreground hover:underline"
                      onclick={closeSshWizard}
                    >
                      {t("settings_remote_sshKeyClose")}
                    </button>
                  {:else if sshKeyStep === "has_key" && sshKeyInfo}
                    <p class="text-emerald-500">
                      {t("settings_remote_sshKeyFound", { keyType: sshKeyInfo.key_type })}
                      <span class="text-muted-foreground ml-1 font-mono">{sshKeyInfo.key_path}</span
                      >
                    </p>

                    {#if remoteFormHost && remoteFormUser}
                      <p class="text-muted-foreground">
                        {t(
                          IS_WINDOWS
                            ? "settings_remote_sshKeyCopyCmd_win"
                            : "settings_remote_sshKeyCopyCmd",
                        )}
                      </p>
                      <div class="flex items-center gap-2">
                        <code
                          class="flex-1 rounded bg-muted px-2 py-1.5 font-mono text-[11px] break-all select-all"
                        >
                          {buildCopyCommand(
                            sshKeyInfo,
                            remoteFormHost.trim(),
                            remoteFormUser.trim(),
                            remoteFormPort || 22,
                          )}
                        </code>
                        <button
                          class="shrink-0 rounded border px-2 py-1 text-[10px] hover:bg-accent transition-colors"
                          onclick={async () => {
                            await navigator.clipboard.writeText(
                              buildCopyCommand(
                                sshKeyInfo!,
                                remoteFormHost.trim(),
                                remoteFormUser.trim(),
                                remoteFormPort || 22,
                              ),
                            );
                            sshCopied = true;
                            setTimeout(() => (sshCopied = false), 2000);
                          }}
                        >
                          {sshCopied ? t("settings_remote_sshKeyCopied") : t("common_copy")}
                        </button>
                      </div>

                      <div class="flex items-center gap-2 mt-1">
                        <button
                          class="rounded border px-3 py-1.5 text-xs hover:bg-accent transition-colors"
                          disabled={sshVerifying}
                          onclick={verifySshConnection}
                        >
                          {sshVerifying
                            ? t("settings_remote_sshKeyVerifying")
                            : t("settings_remote_sshKeyVerify")}
                        </button>
                        <button
                          class="text-[10px] text-muted-foreground hover:underline"
                          onclick={closeSshWizard}
                        >
                          {t("settings_remote_sshKeyClose")}
                        </button>
                      </div>

                      {#if sshKeyError && sshKeyStep === "has_key"}
                        <p class="text-red-400 text-[11px]">
                          {t(
                            IS_WINDOWS
                              ? "settings_remote_sshKeyFailed_win"
                              : "settings_remote_sshKeyFailed",
                          )}
                        </p>
                      {/if}
                    {:else}
                      <p class="text-muted-foreground text-[10px]">
                        Fill in Host and User above, then come back to copy the install command.
                      </p>
                      <button
                        class="text-[10px] text-muted-foreground hover:underline"
                        onclick={closeSshWizard}
                      >
                        {t("settings_remote_sshKeyClose")}
                      </button>
                    {/if}
                  {:else if sshKeyStep === "done"}
                    <p class="text-emerald-500">{t("settings_remote_sshKeySuccess")}</p>
                    <button
                      class="text-[10px] text-muted-foreground hover:underline"
                      onclick={closeSshWizard}
                    >
                      {t("settings_remote_sshKeyClose")}
                    </button>
                  {:else if sshKeyStep === "error"}
                    <p class="text-red-400">
                      {t("settings_remote_sshKeyGenError", { error: sshKeyError })}
                    </p>
                    <button
                      class="text-[10px] text-muted-foreground hover:underline"
                      onclick={closeSshWizard}
                    >
                      {t("settings_remote_sshKeyClose")}
                    </button>
                  {/if}
                </div>
              {/if}
            </div>
            <label class="block">
              <span class="text-xs text-muted-foreground block mb-1"
                >{t("settings_remote_remoteCwd")}</span
              >
              <input
                type="text"
                bind:value={remoteFormRemoteCwd}
                placeholder={currentUsername ? "~/projects" : "~/projects"}
                class="w-full text-sm px-2 py-1.5 rounded border border-input bg-background"
              />
            </label>
            <label class="block">
              <span class="text-xs text-muted-foreground block mb-1"
                >{t("settings_remote_claudePath")}</span
              >
              <input
                type="text"
                bind:value={remoteFormClaudePath}
                placeholder="claude (default)"
                class="w-full text-sm px-2 py-1.5 rounded border border-input bg-background"
              />
            </label>
            <div class="flex items-end">
              <label class="flex items-center gap-2 text-sm cursor-pointer">
                <input type="checkbox" bind:checked={remoteFormForwardKey} class="rounded" />
                {t("settings_remote_forwardKey")}
              </label>
            </div>
          </div>

          {#if remoteFormForwardKey}
            <div
              class="flex items-start gap-2 p-2 rounded bg-yellow-500/10 border border-yellow-500/20 text-xs text-yellow-600 dark:text-yellow-400"
            >
              <span class="shrink-0 mt-0.5">&#9888;</span>
              <span>{t("settings_remote_forwardKeyWarning")}</span>
            </div>
          {/if}

          <!-- Test + Save buttons -->
          <div class="flex gap-2 items-center">
            <Button
              variant="secondary"
              size="sm"
              disabled={remoteTesting}
              onclick={testRemoteConnection}
            >
              {remoteTesting ? t("settings_remote_testing") : t("settings_remote_testConnection")}
            </Button>
            <Button size="sm" disabled={remoteSaving} onclick={() => saveRemoteHost()}>
              {remoteSaving
                ? t("settings_remote_saving")
                : editingRemote
                  ? t("settings_remote_update")
                  : t("settings_remote_add")}
            </Button>
            {#if editingRemote}
              <button
                class="text-xs text-muted-foreground hover:underline"
                onclick={resetRemoteForm}>{t("settings_remote_cancel")}</button
              >
            {/if}
          </div>

          <!-- Test result -->
          {#if remoteTestResult}
            <div
              class="text-xs space-y-1 p-2 rounded border {remoteTestResult.ssh_ok
                ? 'border-green-500/30 bg-green-500/5'
                : 'border-red-500/30 bg-red-500/5'}"
            >
              <p>
                {t("settings_remote_sshLabel")}
                {remoteTestResult.ssh_ok
                  ? t("settings_remote_connected")
                  : t("settings_remote_failed")}
              </p>
              {#if remoteTestResult.ssh_ok}
                <p>
                  {t("settings_remote_cliLabel")}
                  {remoteTestResult.cli_found
                    ? t("settings_remote_found")
                    : t("settings_remote_notFound")}
                </p>
                {#if remoteTestResult.cli_version}
                  <p>{t("settings_remote_version", { version: remoteTestResult.cli_version })}</p>
                {/if}
                {#if remoteTestResult.cli_path}
                  <p>{t("settings_remote_path", { path: remoteTestResult.cli_path })}</p>
                {/if}
                {#if remoteTestResult.ssh_ok && !remoteTestResult.cli_found}
                  <div
                    class="mt-1.5 p-2 rounded bg-amber-500/10 border border-amber-500/20 space-y-1"
                  >
                    <p class="text-amber-400">{t("settings_remote_cliNotFoundHint")}</p>
                    <code class="block rounded bg-muted px-2 py-1 font-mono text-[11px] select-all"
                      >which claude</code
                    >
                    <p class="text-muted-foreground">{t("settings_remote_cliNotFoundHint2")}</p>
                  </div>
                {/if}
              {/if}
              {#if remoteTestResult.error}
                <p class="text-red-500">{remoteTestResult.error}</p>
              {/if}
            </div>
          {/if}
        </div>
      </Card>

      <!-- ═══ Debug tab ═══ -->
    {:else if activeTab === "debug"}
      <Card class="p-6 space-y-5">
        <div class="flex items-center justify-between">
          <div>
            <p class="text-sm font-medium">{t("settings_debug_title")}</p>
            <p class="text-xs text-muted-foreground mt-0.5">
              {t("settings_debug_desc")}
              {t("settings_debug_rustHint")}
              <code class="text-xs">RUST_LOG=debug cargo tauri dev</code>
            </p>
          </div>
          <button
            aria-label="Debug mode"
            class="relative inline-flex h-6 w-11 items-center rounded-full transition-colors duration-200 {debugOn
              ? 'bg-primary'
              : 'bg-neutral-700'}"
            onclick={() => {
              debugOn = !debugOn;
              setDebugMode(debugOn);
            }}
          >
            <span
              class="inline-block h-4 w-4 transform rounded-full bg-white transition-transform duration-200 {debugOn
                ? 'translate-x-6'
                : 'translate-x-1'}"
            ></span>
          </button>
        </div>

        {#if debugOn}
          <!-- Tag filter -->
          <div>
            <label class="text-sm font-medium mb-1 block" for="debug-filter"
              >{t("settings_debug_tagFilter")}</label
            >
            <input
              id="debug-filter"
              class="w-full rounded-md border bg-transparent px-3 py-1.5 text-sm font-mono placeholder:text-muted-foreground focus:border-ring focus:outline-none"
              value={debugFilter}
              placeholder="1 = all, api,bus = only those, -replay = exclude"
              oninput={(e) => {
                const val = (e.target as HTMLInputElement).value.trim();
                debugFilter = val;
                setDebugMode(val || "1");
              }}
            />
            <p class="mt-1 text-[10px] text-muted-foreground">
              <code class="text-xs">1</code> = {t("settings_debug_filterHelp_all")} &nbsp;|&nbsp;
              <code class="text-xs">api,bus</code> = {t("settings_debug_filterHelp_only")} &nbsp;|&nbsp;
              <code class="text-xs">-replay</code> = {t("settings_debug_filterHelp_exclude")}
            </p>
          </div>

          <!-- Log actions -->
          <div class="flex items-center gap-3">
            <button
              class="rounded-md border px-3 py-1.5 text-xs transition-colors hover:bg-accent"
              onclick={async () => {
                logCopied = await copyDebugLogs();
                if (logCopied) setTimeout(() => (logCopied = false), 2000);
              }}
            >
              {logCopied
                ? t("settings_debug_copied")
                : t("settings_debug_copyLogs", { count: String(logCount) })}
            </button>
            <button
              class="rounded-md border px-3 py-1.5 text-xs transition-colors hover:bg-accent text-muted-foreground"
              onclick={() => {
                clearDebugLogs();
                logCount = 0;
              }}
            >
              {t("settings_debug_clear")}
            </button>
            <span class="text-[10px] text-muted-foreground ml-auto"
              >{t("settings_debug_entriesBuffered", { count: String(logCount) })}</span
            >
          </div>

          <!-- Rust log hint -->
          <div class="rounded-md bg-muted/50 p-3">
            <p class="text-xs text-muted-foreground mb-1.5">
              {t("settings_debug_rustBackendLogs")}
            </p>
            <div class="flex items-center gap-2">
              <code class="flex-1 text-xs font-mono break-all">RUST_LOG=debug cargo tauri dev</code>
              <button
                class="shrink-0 rounded border px-2 py-1 text-[10px] transition-colors hover:bg-accent"
                onclick={async () => {
                  await navigator.clipboard.writeText("RUST_LOG=debug cargo tauri dev");
                  rustCmdCopied = true;
                  setTimeout(() => (rustCmdCopied = false), 2000);
                }}
              >
                {rustCmdCopied ? t("settings_debug_copied") : t("settings_debug_copy")}
              </button>
            </div>
          </div>

          <p class="text-[10px] text-muted-foreground">
            {t("settings_debug_maxEntries")}
          </p>
        {/if}
      </Card>
    {/if}
  </div>
{/key}
