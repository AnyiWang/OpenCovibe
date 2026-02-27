<script lang="ts">
  import { onMount, getContext } from "svelte";
  import { page } from "$app/stores";
  import * as api from "$lib/api";
  import { loadCliInfo, KeybindingStore } from "$lib/stores";
  import type {
    UserSettings,
    CliConfigSettingDef,
    RemoteHost,
    RemoteTestResult,
    SshKeyInfo,
  } from "$lib/types";
  import Card from "$lib/components/Card.svelte";
  import Button from "$lib/components/Button.svelte";
  import Input from "$lib/components/Input.svelte";
  import KeybindingEditor from "$lib/components/KeybindingEditor.svelte";
  import { formatKeyDisplay } from "$lib/stores/keybindings.svelte";
  import {
    PLATFORM_PRESETS,
    PRESET_CATEGORIES,
    buildPlatformList,
    isCustomPlatform,
    findCredential,
  } from "$lib/utils/platform-presets";
  import type { PlatformPreset, PlatformCredential } from "$lib/types";
  import {
    isDebugMode,
    setDebugMode,
    copyDebugLogs,
    getDebugLogCount,
    clearDebugLogs,
    getDebugFilter,
  } from "$lib/utils/debug";
  import { dbg, dbgWarn, redactSensitive } from "$lib/utils/debug";
  import { t, LOCALE_REGISTRY, currentLocale, switchLocale } from "$lib/i18n/index.svelte";

  // ── Tab state ──
  type SettingsTab = "general" | "cli-config" | "shortcuts" | "remote" | "debug";
  const VALID_TABS: SettingsTab[] = ["general", "cli-config", "shortcuts", "remote", "debug"];
  const urlTab = $page.url.searchParams.get("tab");
  const initialTab: SettingsTab = VALID_TABS.includes(urlTab as SettingsTab)
    ? (urlTab as SettingsTab)
    : "general";
  let activeTab = $state<SettingsTab>(initialTab);

  const tabLabels: Record<SettingsTab, () => string> = {
    general: () => t("settings_tab_general"),
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
  let platformModels = $state("");
  let selectedPlatformId = $state<string | null>(null);
  let showPlatformPicker = $state(false);
  let platformCredentials = $state<PlatformCredential[]>([]);

  // CLI Auth state
  let authOverview = $state<import("$lib/types").AuthOverview | null>(null);

  // Derive merged platform list (static presets + dynamic custom endpoints)
  let platformList = $derived(buildPlatformList(platformCredentials));

  // Derive selected platform from id (search merged list, not just static presets)
  let selectedPlatform = $derived<PlatformPreset | null>(
    selectedPlatformId ? (platformList.find((p) => p.id === selectedPlatformId) ?? null) : null,
  );

  // Custom endpoint editing state
  let editingCustomId = $state<string | null>(null);
  let customFormName = $state("");
  let customFormBaseUrl = $state("");
  let customFormAuthEnvVar = $state<"ANTHROPIC_API_KEY" | "ANTHROPIC_AUTH_TOKEN">(
    "ANTHROPIC_AUTH_TOKEN",
  );
  let debugOn = $state(isDebugMode());
  let logCopied = $state(false);
  let debugFilter = $state(getDebugFilter() || "1");
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

  function buildCopyCommand(keyInfo: SshKeyInfo, host: string, user: string, port: number): string {
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
  let hasOverrides = $derived(keybindingStore.overrides.length > 0);

  function isOverridden(command: string): boolean {
    return keybindingStore.overrides.some((o) => o.command === command);
  }

  function getConflictWarning(key: string, context: string, excludeCmd: string): string {
    const conflict = keybindingStore.findConflict(key, context, excludeCmd);
    return conflict ? t("settings_shortcuts_conflictsWith", { label: conflict.label }) : "";
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
      cliConfig = await api.getCliConfig();
      // Load project config for override indicators
      const cwd = localStorage.getItem("ocv:project-cwd") || "";
      if (cwd) {
        projectCliConfig = await api.getProjectCliConfig(cwd);
      }
      cliConfigLoaded = true;
      dbg("settings", "cliConfig loaded", {
        keys: Object.keys(cliConfig).length,
        projectKeys: Object.keys(projectCliConfig).length,
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
    if (!platformId) {
      anthropicApiKey = "";
      anthropicBaseUrl = "";
      return;
    }
    const cred = findCredential(platformCredentials, platformId);
    const preset = PLATFORM_PRESETS.find((p) => p.id === platformId);
    anthropicApiKey = cred?.api_key ?? "";
    // base_url: credential override > preset default > empty
    anthropicBaseUrl = cred?.base_url ?? preset?.base_url ?? "";
    // models: credential override > preset default > empty
    const models = cred?.models ?? preset?.models;
    platformModels = models?.join(", ") ?? "";
    dbg("settings", "loadFieldsFromCredential", {
      platformId,
      hasKey: !!anthropicApiKey,
      url: anthropicBaseUrl,
      models: platformModels,
    });
  }

  /** Save current editing fields into the credentials array. */
  function saveCurrentToCredential() {
    if (!selectedPlatformId) return;
    const preset = PLATFORM_PRESETS.find((p) => p.id === selectedPlatformId);
    // Parse user-edited models (comma-separated) — falls back to preset if empty
    const parsedModels = platformModels
      .split(",")
      .map((s) => s.trim())
      .filter(Boolean);
    const modelsToSave = parsedModels.length > 0 ? parsedModels : preset?.models;
    _upsertCredential(selectedPlatformId, {
      api_key: anthropicApiKey || undefined,
      // Always save base_url — backend needs it for ANTHROPIC_BASE_URL injection
      base_url: anthropicBaseUrl || preset?.base_url || undefined,
      auth_env_var: selectedPlatform?.auth_env_var ?? preset?.auth_env_var,
      models: modelsToSave,
      extra_env: preset?.extra_env,
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

  function applyPlatformPreset(preset: PlatformPreset) {
    // 1. Save current platform's data to credentials (if modified)
    saveCurrentToCredential();
    // 2. Switch to new platform
    selectedPlatformId = preset.id;
    showPlatformPicker = false;
    // 3. Load new platform's data from credentials
    loadFieldsFromCredential(preset.id);
    // 4. Sync global fields + persist
    syncAndSave(preset.id);
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

  /** Add a new custom endpoint. */
  function addCustomEndpoint() {
    const id = `custom-${Date.now()}`;
    const cred: PlatformCredential = {
      platform_id: id,
      name: customFormName || "Custom",
      base_url: customFormBaseUrl,
      auth_env_var: customFormAuthEnvVar,
    };
    platformCredentials = [...platformCredentials, cred];
    saveGeneralPatch({ platform_credentials: platformCredentials });
    // Select the newly created endpoint
    const preset = buildPlatformList(platformCredentials).find((p) => p.id === id);
    if (preset) applyPlatformPreset(preset);
    // Reset form
    customFormName = "";
    customFormBaseUrl = "";
    editingCustomId = null;
  }

  /** Delete a custom endpoint. */
  function deleteCustomEndpoint(platformId: string) {
    platformCredentials = platformCredentials.filter((c) => c.platform_id !== platformId);
    saveGeneralPatch({ platform_credentials: platformCredentials });
    // If we deleted the active platform, switch to Anthropic
    if (selectedPlatformId === platformId) {
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
    // Load auth overview
    api
      .getAuthOverview()
      .then((ov) => (authOverview = ov))
      .catch((e) => {
        dbgWarn("settings", "failed to load auth overview", e);
      });
    loadCliInfo();
    // Detect current username + CLI keybindings source
    import("@tauri-apps/api/path")
      .then((p) => p.homeDir())
      .then((home) => {
        // Extract username from home dir path (e.g. /Users/alice/ → alice)
        const parts = home.replace(/\/+$/, "").split("/");
        currentUsername = parts[parts.length - 1] || "";
        return api.readTextFile(`${home}.claude/keybindings.json`);
      })
      .then(() => {
        cliSource = "file";
      })
      .catch(() => {
        cliSource = "defaults";
      });
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
</script>

{#key currentLocale()}
  <div class="max-w-2xl mx-auto p-6 animate-slide-up">
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
                    : entry.status === 'beta'
                      ? 'border-muted-foreground/30 text-muted-foreground hover:bg-accent'
                      : 'hover:bg-accent'}"
                  onclick={() => switchLocale(entry.code)}
                >
                  {entry.nativeName}{#if entry.status === "beta"}<span
                      class="ml-1 text-[10px] opacity-60">(Beta)</span
                    >{/if}
                </button>
              {/each}
            </div>
          </div>
        </Card>

        <!-- Authentication -->
        <Card class="p-6 space-y-5">
          <div class="flex items-center justify-between">
            <h2 class="text-sm font-semibold text-muted-foreground uppercase tracking-wider">
              {t("settings_general_connection")}
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
            <label class="text-sm font-medium mb-2 block">{t("settings_auth_modeLabel")}</label>
            <div class="mt-1 grid grid-cols-2 gap-3">
              <button
                class="flex flex-col items-center gap-2 rounded-lg border p-4 text-sm transition-all duration-150
                {authMode === 'cli'
                  ? 'border-primary bg-primary/5 ring-1 ring-primary/20'
                  : 'hover:bg-accent hover:border-ring/30'}"
                onclick={() => {
                  authMode = "cli";
                  saveGeneralPatch({ auth_mode: "cli" });
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
                  <div class="flex items-center gap-2">
                    <span class="h-2 w-2 rounded-full bg-muted-foreground/40"></span>
                    <span class="text-xs text-muted-foreground">{t("auth_notLoggedIn")}</span>
                    <Button
                      size="sm"
                      variant="outline"
                      onclick={() =>
                        api.runClaudeLogin().then(() => {
                          api
                            .getAuthOverview()
                            .then((ov) => (authOverview = ov))
                            .catch(() => {});
                        })}>{t("settings_auth_cliLoginBtn")}</Button
                    >
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
                <label class="text-sm font-medium mb-1.5 block"
                  >{t("settings_general_platform")}</label
                >
                {#if selectedPlatform && !showPlatformPicker}
                  <!-- Selected platform badge -->
                  <div
                    class="flex items-center gap-2 rounded-lg border border-primary/30 bg-primary/5 px-3 py-2"
                  >
                    <span class="text-sm font-medium">{selectedPlatform.name}</span>
                    <span class="text-xs text-muted-foreground">{selectedPlatform.description}</span
                    >
                    <button
                      class="ml-auto text-xs text-muted-foreground hover:text-foreground transition-colors"
                      onclick={() => (showPlatformPicker = true)}
                      >{t("settings_general_changePlatform")}</button
                    >
                  </div>
                {:else}
                  <!-- Platform grid picker -->
                  <div
                    class="flex flex-col gap-3 max-h-[40vh] overflow-y-auto rounded-lg border border-border p-3"
                  >
                    {#each PRESET_CATEGORIES as category}
                      {@const presets =
                        category.id === "custom"
                          ? platformList.filter((p) => p.category === "custom")
                          : platformList.filter((p) => p.category === category.id)}
                      {#if presets.length > 0 || category.id === "custom"}
                        <div>
                          <p
                            class="text-[10px] font-medium text-muted-foreground uppercase tracking-wider mb-1.5"
                          >
                            {category.label}
                          </p>
                          <div class="grid grid-cols-3 gap-1.5">
                            {#each presets as preset}
                              <button
                                class="flex flex-col gap-0 rounded-md border p-2 text-left transition-colors relative group
                                {selectedPlatformId === preset.id
                                  ? 'border-primary bg-primary/5'
                                  : 'border-border hover:border-primary/40 hover:bg-accent/50'}"
                                onclick={() => applyPlatformPreset(preset)}
                              >
                                <span class="text-xs font-medium truncate">{preset.name}</span>
                                <span class="text-[10px] text-muted-foreground truncate"
                                  >{preset.description}</span
                                >
                                {#if isCustomPlatform(preset.id)}
                                  <button
                                    class="absolute top-1 right-1 opacity-0 group-hover:opacity-100 text-muted-foreground hover:text-destructive transition-all p-0.5"
                                    onclick={(e: MouseEvent) => {
                                      e.stopPropagation();
                                      deleteCustomEndpoint(preset.id);
                                    }}
                                    title={t("settings_general_deleteCustom")}
                                  >
                                    <svg
                                      class="h-3 w-3"
                                      viewBox="0 0 24 24"
                                      fill="none"
                                      stroke="currentColor"
                                      stroke-width="2"
                                      ><path d="M18 6 6 18" /><path d="m6 6 12 12" /></svg
                                    >
                                  </button>
                                {/if}
                                {#if findCredential(platformCredentials, preset.id)?.api_key}
                                  <span
                                    class="absolute bottom-1 right-1 h-1.5 w-1.5 rounded-full bg-green-500"
                                    title="Key saved"
                                  ></span>
                                {/if}
                              </button>
                            {/each}
                          </div>
                          {#if category.id === "custom"}
                            <!-- Add custom endpoint inline form -->
                            {#if editingCustomId === "new"}
                              <div class="mt-2 rounded-md border border-border p-2 space-y-2">
                                <Input
                                  bind:value={customFormName}
                                  placeholder={t("settings_general_customNamePlaceholder")}
                                  class="text-xs"
                                />
                                <Input
                                  bind:value={customFormBaseUrl}
                                  placeholder="https://api.example.com"
                                  class="text-xs font-mono"
                                />
                                <div class="flex gap-2">
                                  <select
                                    bind:value={customFormAuthEnvVar}
                                    class="flex-1 rounded-md border border-input bg-background px-2 py-1.5 text-xs"
                                  >
                                    <option value="ANTHROPIC_AUTH_TOKEN">Bearer token</option>
                                    <option value="ANTHROPIC_API_KEY">x-api-key</option>
                                  </select>
                                  <Button
                                    size="sm"
                                    onclick={() => addCustomEndpoint()}
                                    disabled={!customFormBaseUrl.trim()}>{t("common_save")}</Button
                                  >
                                  <Button
                                    size="sm"
                                    variant="ghost"
                                    onclick={() => (editingCustomId = null)}
                                    >{t("common_cancel")}</Button
                                  >
                                </div>
                              </div>
                            {:else}
                              <button
                                class="mt-2 flex items-center gap-1 text-xs text-muted-foreground hover:text-foreground transition-colors"
                                onclick={() => (editingCustomId = "new")}
                              >
                                <svg
                                  class="h-3 w-3"
                                  viewBox="0 0 24 24"
                                  fill="none"
                                  stroke="currentColor"
                                  stroke-width="2"><path d="M12 5v14" /><path d="M5 12h14" /></svg
                                >
                                {t("settings_general_addCustom")}
                              </button>
                            {/if}
                          {/if}
                        </div>
                      {/if}
                    {/each}
                  </div>
                  {#if selectedPlatform}
                    <button
                      class="mt-1.5 text-xs text-muted-foreground hover:text-foreground transition-colors"
                      onclick={() => (showPlatformPicker = false)}
                      >{t("settings_general_cancelChange")}</button
                    >
                  {/if}
                {/if}
              </div>

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
                      onblur={() => {
                        saveCurrentToCredential();
                        if (selectedPlatformId) syncAndSave(selectedPlatformId);
                      }}
                    />
                  </div>
                  <button
                    class="rounded-md border px-3 py-2 text-xs text-muted-foreground hover:bg-accent transition-colors"
                    onclick={() => (showApiKey = !showApiKey)}
                  >
                    {showApiKey ? t("settings_general_hide") : t("settings_general_show")}
                  </button>
                </div>
                {#if selectedPlatform?.id === "ollama"}
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
                    !isCustomPlatform(selectedPlatformId ?? "")}
                  onblur={() => {
                    saveCurrentToCredential();
                    if (selectedPlatformId) syncAndSave(selectedPlatformId);
                  }}
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

              <!-- Models -->
              <div>
                <label class="text-sm font-medium mb-1.5 block" for="platform-models"
                  >{t("settings_general_models")}</label
                >
                <Input
                  bind:value={platformModels}
                  placeholder={selectedPlatform?.models?.join(", ") ||
                    t("settings_general_modelsPlaceholder")}
                  class="mt-1 font-mono text-xs"
                  onblur={() => {
                    saveCurrentToCredential();
                    if (selectedPlatformId) syncAndSave(selectedPlatformId);
                  }}
                />
                <p class="mt-1 text-xs text-muted-foreground">
                  {t("settings_general_modelsHelp")}
                </p>
              </div>
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
            <span
              class="text-[10px] font-normal normal-case tracking-normal text-muted-foreground/50"
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
                    ? "~/.claude/keybindings.json"
                    : t("settings_shortcuts_cliDefaults"),
              })}
            </p>
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
            <div>
              <label class="text-xs text-muted-foreground block mb-1"
                >{t("settings_remote_name")} *</label
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
            </div>
            <div>
              <label class="text-xs text-muted-foreground block mb-1"
                >{t("settings_remote_host")} *</label
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
            </div>
            <div>
              <label class="text-xs text-muted-foreground block mb-1"
                >{t("settings_remote_user")} *</label
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
            </div>
            <div>
              <label class="text-xs text-muted-foreground block mb-1"
                >{t("settings_remote_port")}</label
              >
              <input
                type="number"
                bind:value={remoteFormPort}
                placeholder="22"
                class="w-full text-sm px-2 py-1.5 rounded border border-input bg-background"
              />
            </div>
            <div class="col-span-2">
              <label class="text-xs text-muted-foreground block mb-1"
                >{t("settings_remote_keyPath")}</label
              >
              <div class="flex gap-2">
                <input
                  type="text"
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
                    <p class="text-amber-400">{t("settings_remote_sshKeyPubMissing")}</p>
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
                      <p class="text-muted-foreground">{t("settings_remote_sshKeyCopyCmd")}</p>
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
                        <p class="text-red-400 text-[11px]">{t("settings_remote_sshKeyFailed")}</p>
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
            <div>
              <label class="text-xs text-muted-foreground block mb-1"
                >{t("settings_remote_remoteCwd")}</label
              >
              <input
                type="text"
                bind:value={remoteFormRemoteCwd}
                placeholder={currentUsername
                  ? `/Users/${currentUsername}/projects`
                  : "/home/user/projects"}
                class="w-full text-sm px-2 py-1.5 rounded border border-input bg-background"
              />
            </div>
            <div>
              <label class="text-xs text-muted-foreground block mb-1"
                >{t("settings_remote_claudePath")}</label
              >
              <input
                type="text"
                bind:value={remoteFormClaudePath}
                placeholder="claude (default)"
                class="w-full text-sm px-2 py-1.5 rounded border border-input bg-background"
              />
            </div>
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
