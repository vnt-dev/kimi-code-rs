import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  AlertTriangle,
  ArrowLeft,
  ExternalLink,
  FolderOpen,
  Package,
  RefreshCw,
  Search,
  Trash2,
} from "lucide-react";

import { t, type TranslationKey } from "./i18n";
import {
  isThirdPartyEntry,
  marketplaceUpdateAvailable,
  pluginInstallPercent,
  pluginTabNeedsNetwork,
  type CapabilityId,
  type CapabilityReadiness,
  type CapabilityStatus,
  type PluginInfo,
  type PluginInstallProgressEvent,
  type PluginMarketplace,
  type PluginMarketplaceEntry,
  type PluginSummary,
  type PluginTab,
  type PluginUpdateStatus,
} from "./plugins";
import { invoke, listen, openExternalUrl, pickNativeDirectory } from "./transport";
import { formatBytes } from "./utils/format";

interface ConfirmState {
  kind: "install" | "remove";
  label: string;
  id?: string;
  source?: string;
}

function messageOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function initials(label: string): string {
  return label
    .split(/[\s_-]+/)
    .filter(Boolean)
    .slice(0, 2)
    .map((part) => part[0]?.toUpperCase())
    .join("") || "P";
}

function capabilityText(plugin: PluginSummary): string {
  return [
    plugin.skillCount ? t("plugins.skillsCount", { count: plugin.skillCount }) : undefined,
    plugin.mcpServerCount
      ? t("plugins.mcpCount", {
          enabled: plugin.enabledMcpServerCount,
          count: plugin.mcpServerCount,
        })
      : undefined,
    plugin.hookCount ? t("plugins.hooksCount", { count: plugin.hookCount }) : undefined,
    plugin.commandCount ? t("plugins.commandsCount", { count: plugin.commandCount }) : undefined,
  ]
    .filter(Boolean)
    .join(" · ") || t("plugins.noCapabilities");
}

function createInstallOperationId(): string {
  return globalThis.crypto?.randomUUID?.() ??
    `plugin-install-${Date.now()}-${Math.random().toString(16).slice(2)}`;
}

const CAPABILITY_STATE_KEYS: Record<CapabilityReadiness, TranslationKey> = {
  not_installed: "plugins.capability.state.not_installed",
  partial: "plugins.capability.state.partial",
  ready: "plugins.capability.state.ready",
  unsupported: "plugins.capability.state.unsupported",
};

// `install.step` is a machine key reported by the engine; clients localize it.
const CAPABILITY_STEP_KEYS: Record<string, TranslationKey> = {
  plugin: "plugins.capability.step.plugin",
  "mcp-config": "plugins.capability.step.mcp-config",
  download: "plugins.capability.step.download",
  app: "plugins.capability.step.app",
  service: "plugins.capability.step.service",
  permissions: "plugins.capability.step.permissions",
  runtime: "plugins.capability.step.runtime",
  daemon: "plugins.capability.step.daemon",
  skill: "plugins.capability.step.skill",
  "standalone-skill-migration": "plugins.capability.step.standalone-skill-migration",
};

function capabilityStepLabel(step: string): string {
  const key = CAPABILITY_STEP_KEYS[step];
  return key ? t(key) : step;
}

const CAPABILITY_POLL_INTERVAL_MS = 700;
const CAPABILITY_POLL_ATTEMPTS = 260; // ~3 minutes of runtime setup budget

export default function PluginSettings({ onChanged }: { onChanged: () => void }) {
  const [tab, setTab] = useState<PluginTab>("installed");
  const [query, setQuery] = useState("");
  const [plugins, setPlugins] = useState<PluginSummary[]>([]);
  const [marketplace, setMarketplace] = useState<PluginMarketplace>();
  const [updates, setUpdates] = useState<PluginUpdateStatus[]>([]);
  const [installedLoading, setInstalledLoading] = useState(true);
  const [marketLoading, setMarketLoading] = useState(false);
  const [marketError, setMarketError] = useState<string>();
  const [installedError, setInstalledError] = useState<string>();
  const [busy, setBusy] = useState<string>();
  const [notice, setNotice] = useState<string>();
  const [customSource, setCustomSource] = useState("");
  const [confirm, setConfirm] = useState<ConfirmState>();
  const [detail, setDetail] = useState<PluginInfo>();
  const [detailBusy, setDetailBusy] = useState(false);
  const [installProgress, setInstallProgress] = useState<PluginInstallProgressEvent>();
  const installOperationRef = useRef<string | undefined>(undefined);
  const progressClearTimerRef = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const marketAttemptedRef = useRef(false);
  const [capabilities, setCapabilities] = useState<CapabilityStatus[]>([]);
  const [capabilityBusy, setCapabilityBusy] = useState<CapabilityId>();
  const capabilityAttemptedRef = useRef(false);
  const capabilityPollTimerRef = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const capabilityPollAttemptsRef = useRef(0);

  const loadInstalled = useCallback(async (reload = false): Promise<void> => {
    setInstalledLoading(true);
    setInstalledError(undefined);
    if (reload) {
      try {
        await invoke("reload_plugins");
      } catch (error) {
        setInstalledError(messageOf(error));
      }
    }
    try {
      setPlugins(await invoke<PluginSummary[]>("list_plugins"));
    } catch (error) {
      setInstalledError(messageOf(error));
    } finally {
      setInstalledLoading(false);
    }
  }, []);

  const loadMarketData = useCallback(async (): Promise<void> => {
    marketAttemptedRef.current = true;
    setMarketLoading(true);
    setMarketError(undefined);
    const [marketResult, updateResult] = await Promise.allSettled([
      invoke<PluginMarketplace>("get_plugin_marketplace"),
      invoke<PluginUpdateStatus[]>("check_plugin_updates"),
    ]);
    if (marketResult.status === "fulfilled") {
      setMarketplace(marketResult.value);
    } else {
      setMarketError(messageOf(marketResult.reason));
    }
    if (updateResult.status === "fulfilled") setUpdates(updateResult.value);
    setMarketLoading(false);
  }, []);

  useEffect(() => {
    void loadInstalled();
  }, [loadInstalled]);

  useEffect(() => {
    if (!pluginTabNeedsNetwork(tab) || marketAttemptedRef.current) return;
    void loadMarketData();
  }, [loadMarketData, tab]);

  const loadCapabilities = useCallback(async (): Promise<CapabilityStatus[]> => {
    try {
      const next = await invoke<CapabilityStatus[]>("list_capabilities");
      setCapabilities(next);
      return next;
    } catch {
      // Detection is best-effort — the marketplace rows still render without it.
      return [];
    }
  }, []);

  const stopCapabilityPolling = useCallback((): void => {
    if (capabilityPollTimerRef.current !== undefined) {
      clearTimeout(capabilityPollTimerRef.current);
      capabilityPollTimerRef.current = undefined;
    }
  }, []);

  const finishCapabilityInstall = useCallback(
    async (nextNotice?: string): Promise<void> => {
      stopCapabilityPolling();
      setCapabilityBusy(undefined);
      if (nextNotice) setNotice(nextNotice);
      await loadCapabilities();
      // The install rewires a plugin layer, so the installed list changes too.
      await loadInstalled();
      onChanged();
    },
    [loadCapabilities, loadInstalled, onChanged, stopCapabilityPolling],
  );

  const pollCapabilityInstall = useCallback(
    async (id: CapabilityId, name: string): Promise<void> => {
      try {
        const status = await invoke<CapabilityStatus>("get_capability", { id });
        setCapabilities((current) =>
          current.map((item) => (item.id === id ? status : item)),
        );
        if (!status.install.running) {
          await finishCapabilityInstall(
            status.install.error
              ? t("plugins.operationFailed", { error: status.install.error })
              : t("plugins.capability.installedNotice", { name }),
          );
          return;
        }
      } catch (error) {
        await finishCapabilityInstall(t("plugins.operationFailed", { error: messageOf(error) }));
        return;
      }
      capabilityPollAttemptsRef.current += 1;
      if (capabilityPollAttemptsRef.current >= CAPABILITY_POLL_ATTEMPTS) {
        await finishCapabilityInstall();
        return;
      }
      capabilityPollTimerRef.current = setTimeout(
        () => void pollCapabilityInstall(id, name),
        CAPABILITY_POLL_INTERVAL_MS,
      );
    },
    [finishCapabilityInstall],
  );

  // Follow (never restart) a running background install until it settles.
  const followCapabilityInstall = useCallback(
    (id: CapabilityId, name: string): void => {
      stopCapabilityPolling();
      capabilityPollAttemptsRef.current = 0;
      setCapabilityBusy(id);
      capabilityPollTimerRef.current = setTimeout(
        () => void pollCapabilityInstall(id, name),
        CAPABILITY_POLL_INTERVAL_MS,
      );
    },
    [pollCapabilityInstall, stopCapabilityPolling],
  );

  useEffect(() => {
    if (tab !== "official" || capabilityAttemptedRef.current) return;
    capabilityAttemptedRef.current = true;
    void loadCapabilities().then((next) => {
      const running = next.find((capability) => capability.install.running);
      if (running) followCapabilityInstall(running.id, running.displayName);
    });
  }, [followCapabilityInstall, loadCapabilities, tab]);

  useEffect(() => () => stopCapabilityPolling(), [stopCapabilityPolling]);

  const installCapability = async (capability: CapabilityStatus): Promise<void> => {
    if (capabilityBusy || busy) return;
    setNotice(undefined);
    try {
      // An install already running (started from another panel or client) is
      // followed, not restarted — the service rejects duplicate starts even
      // though the original is healthy.
      const started = capability.install.running
        ? capability
        : await invoke<CapabilityStatus>("install_capability", { id: capability.id });
      setCapabilities((current) =>
        current.map((item) => (item.id === started.id ? started : item)),
      );
      if (started.install.running) {
        followCapabilityInstall(capability.id, capability.displayName);
      } else {
        await finishCapabilityInstall(
          started.install.error
            ? t("plugins.operationFailed", { error: started.install.error })
            : t("plugins.capability.installedNotice", { name: capability.displayName }),
        );
      }
    } catch (error) {
      await finishCapabilityInstall(t("plugins.operationFailed", { error: messageOf(error) }));
    }
  };

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void listen<PluginInstallProgressEvent>("plugin-install-progress", (event) => {
      if (event.payload.operationId === installOperationRef.current) {
        setInstallProgress(event.payload);
      }
    }).then((nextUnlisten) => {
      if (disposed) nextUnlisten();
      else unlisten = nextUnlisten;
    });
    return () => {
      disposed = true;
      unlisten?.();
      if (progressClearTimerRef.current) clearTimeout(progressClearTimerRef.current);
    };
  }, []);

  const installedById = useMemo(
    () => new Map(plugins.map((plugin) => [plugin.id, plugin])),
    [plugins],
  );
  const marketById = useMemo(
    () => new Map((marketplace?.plugins ?? []).map((entry) => [entry.id, entry])),
    [marketplace],
  );
  const githubUpdates = useMemo(
    () => new Map(updates.map((update) => [update.id, update])),
    [updates],
  );

  const hasUpdate = useCallback(
    (plugin: PluginSummary, entry?: PluginMarketplaceEntry): boolean =>
      (!!entry && marketplaceUpdateAvailable(plugin, entry)) ||
      githubUpdates.get(plugin.id)?.updateAvailable === true,
    [githubUpdates],
  );

  const refreshAfterMutation = async (message: string): Promise<void> => {
    await invoke("reload_plugins");
    const nextPlugins = await invoke<PluginSummary[]>("list_plugins");
    setPlugins(nextPlugins);
    setUpdates([]);
    setNotice(message);
    onChanged();
  };

  const refresh = async (): Promise<void> => {
    await loadInstalled(true);
    if (pluginTabNeedsNetwork(tab)) await loadMarketData();
  };

  const install = async (source: string, label: string): Promise<void> => {
    const operationId = createInstallOperationId();
    installOperationRef.current = operationId;
    if (progressClearTimerRef.current) clearTimeout(progressClearTimerRef.current);
    setInstallProgress({
      operationId,
      phase: "resolving",
      downloadedBytes: 0,
    });
    setBusy(`install:${label}`);
    setNotice(undefined);
    try {
      await invoke("install_plugin", { source, operationId });
      await refreshAfterMutation(t("plugins.installedNotice", { name: label }));
      setTab("installed");
      setCustomSource("");
      setInstallProgress((current) =>
        current?.operationId === operationId
          ? { ...current, phase: "complete" }
          : current,
      );
      progressClearTimerRef.current = setTimeout(() => {
        if (installOperationRef.current === operationId) {
          installOperationRef.current = undefined;
          setInstallProgress(undefined);
        }
      }, 900);
    } catch (error) {
      if (installOperationRef.current === operationId) {
        installOperationRef.current = undefined;
        setInstallProgress(undefined);
      }
      setNotice(t("plugins.operationFailed", { error: messageOf(error) }));
    } finally {
      setBusy(undefined);
    }
  };

  const installPercent = installProgress
    ? pluginInstallPercent(installProgress)
    : undefined;
  const installBytes = installProgress?.phase === "downloading"
    ? installProgress.totalBytes
      ? `${formatBytes(installProgress.downloadedBytes)} / ${formatBytes(installProgress.totalBytes)}`
      : formatBytes(installProgress.downloadedBytes)
    : undefined;

  const requestInstall = (entry: PluginMarketplaceEntry): void => {
    if (isThirdPartyEntry(entry)) {
      setConfirm({
        kind: "install",
        label: entry.displayName,
        source: entry.source,
      });
    } else {
      void install(entry.source, entry.displayName);
    }
  };

  const requestCustomInstall = (): void => {
    const source = customSource.trim();
    if (!source) return;
    setConfirm({ kind: "install", label: source, source });
  };

  const togglePlugin = async (plugin: PluginSummary): Promise<void> => {
    setBusy(`toggle:${plugin.id}`);
    try {
      await invoke("set_plugin_enabled", { id: plugin.id, enabled: !plugin.enabled });
      await refreshAfterMutation(
        t(plugin.enabled ? "plugins.disabledNotice" : "plugins.enabledNotice", {
          name: plugin.displayName,
        }),
      );
      if (detail?.id === plugin.id) {
        setDetail(await invoke<PluginInfo>("get_plugin_info", { id: plugin.id }));
      }
    } catch (error) {
      setNotice(t("plugins.operationFailed", { error: messageOf(error) }));
    } finally {
      setBusy(undefined);
    }
  };

  const removePlugin = async (id: string, label: string): Promise<void> => {
    setBusy(`remove:${id}`);
    try {
      await invoke("remove_plugin", { id });
      await refreshAfterMutation(t("plugins.removedNotice", { name: label }));
      setDetail(undefined);
    } catch (error) {
      setNotice(t("plugins.operationFailed", { error: messageOf(error) }));
    } finally {
      setBusy(undefined);
    }
  };

  const openDetail = async (id: string): Promise<void> => {
    setDetailBusy(true);
    setNotice(undefined);
    try {
      setDetail(await invoke<PluginInfo>("get_plugin_info", { id }));
    } catch (error) {
      setNotice(t("plugins.operationFailed", { error: messageOf(error) }));
    } finally {
      setDetailBusy(false);
    }
  };

  const toggleMcp = async (server: string, enabled: boolean): Promise<void> => {
    if (!detail) return;
    setBusy(`mcp:${detail.id}:${server}`);
    try {
      await invoke("set_plugin_mcp_server_enabled", {
        id: detail.id,
        server,
        enabled,
      });
      await refreshAfterMutation(t("plugins.mcpChangedNotice"));
      setDetail(await invoke<PluginInfo>("get_plugin_info", { id: detail.id }));
    } catch (error) {
      setNotice(t("plugins.operationFailed", { error: messageOf(error) }));
    } finally {
      setBusy(undefined);
    }
  };

  const normalizedQuery = query.trim().toLowerCase();
  const matches = (name: string, text = "", keywords: string[] = []): boolean =>
    !normalizedQuery ||
    `${name} ${text} ${keywords.join(" ")}`.toLowerCase().includes(normalizedQuery);

  const visiblePlugins = plugins.filter((plugin) =>
    matches(plugin.displayName, plugin.id),
  );
  const visibleMarket = (marketplace?.plugins ?? []).filter((entry) => {
    const tierMatches = tab === "official" ? entry.tier === "official" : entry.tier !== "official";
    return tierMatches && matches(entry.displayName, entry.description, entry.keywords);
  });
  // Built-in capabilities merge into the official tab, ahead of the catalog.
  const visibleCapabilities = capabilities.filter((capability) =>
    matches(capability.displayName, capability.description),
  );
  const loading = tab === "installed"
    ? installedLoading
    : pluginTabNeedsNetwork(tab)
      ? marketLoading
      : false;
  const showSkeleton = tab === "installed"
    ? installedLoading && plugins.length === 0
    : pluginTabNeedsNetwork(tab) && marketLoading;

  if (detail) {
    const homepage = detail.manifest?.interface?.websiteURL ?? detail.manifest?.homepage;
    return (
      <section className="plugin-settings plugin-detail" aria-label={t("plugins.details") }>
        <button className="plugin-back" type="button" onClick={() => setDetail(undefined)}>
          <ArrowLeft size={15} /> {t("plugins.back")}
        </button>
        <div className="plugin-detail-hero">
          <span className="plugin-avatar">{initials(detail.displayName)}</span>
          <div>
            <h3>{detail.displayName}</h3>
            <p>{detail.id}{detail.version ? ` · v${detail.version}` : ""}</p>
          </div>
          <button
            className={`settings-toggle ${detail.enabled ? "active" : ""}`}
            type="button"
            role="switch"
            aria-checked={detail.enabled}
            disabled={busy === `toggle:${detail.id}`}
            onClick={() => void togglePlugin(detail)}
          ><span /></button>
        </div>
        <p className="plugin-detail-description">
          {detail.manifest?.interface?.longDescription ??
            detail.manifest?.interface?.shortDescription ??
            detail.manifest?.description ??
            t("plugins.noDescription")}
        </p>
        <dl className="plugin-meta">
          <div><dt>{t("plugins.source")}</dt><dd>{detail.originalSource ?? detail.root}</dd></div>
          <div><dt>{t("plugins.installedAt")}</dt><dd>{new Date(detail.installedAt).toLocaleString()}</dd></div>
          <div><dt>{t("plugins.capabilities")}</dt><dd>{capabilityText(detail)}</dd></div>
        </dl>
        {homepage && (
          <button className="plugin-link" type="button" onClick={() => void openExternalUrl(homepage)}>
            <ExternalLink size={14} /> {t("plugins.website")}
          </button>
        )}
        <h4>{t("plugins.mcpServers")}</h4>
        {detail.mcpServers.length ? detail.mcpServers.map((server) => (
          <div className="plugin-mcp-row" key={server.name}>
            <div><strong>{server.name}</strong><small>{server.transport} · {server.runtimeName}</small></div>
            <button
              className={`settings-toggle ${server.enabled ? "active" : ""}`}
              type="button"
              role="switch"
              aria-checked={server.enabled}
              disabled={busy === `mcp:${detail.id}:${server.name}`}
              onClick={() => void toggleMcp(server.name, !server.enabled)}
            ><span /></button>
          </div>
        )) : <p className="plugin-empty-copy">{t("plugins.noMcp")}</p>}
        {detail.diagnostics.length > 0 && (
          <><h4>{t("plugins.diagnostics")}</h4><div className="plugin-diagnostics">
            {detail.diagnostics.map((item, index) => (
              <p className={item.severity} key={`${item.message}-${index}`}>
                <AlertTriangle size={13} /> {item.message}
              </p>
            ))}
          </div></>
        )}
        <div className="plugin-detail-actions">
          <button className="plugin-danger" type="button" onClick={() => setConfirm({ kind: "remove", id: detail.id, label: detail.displayName })}>
            <Trash2 size={14} /> {t("plugins.remove")}
          </button>
        </div>
        {notice && <div className="plugin-notice" role="status">{notice}</div>}
      </section>
    );
  }

  return (
    <section className="plugin-settings" aria-labelledby="plugins-heading">
      <div className="plugin-settings-heading">
        <div><h3 id="plugins-heading">{t("plugins.title")}</h3><p>{t("plugins.description")}</p></div>
        <button className="plugin-icon-button" type="button" aria-label={t("plugins.refresh")} disabled={loading} onClick={() => void refresh()}>
          <RefreshCw size={15} className={loading ? "spinning" : undefined} />
        </button>
      </div>
      <div className="plugin-tabs" role="tablist">
        {(["installed", "official", "third-party", "custom"] as const).map((item) => (
          <button key={item} type="button" role="tab" aria-selected={tab === item} className={tab === item ? "active" : ""} onClick={() => setTab(item)}>
            {t(`plugins.tab.${item}`)}
          </button>
        ))}
      </div>
      {tab !== "custom" && (
        <label className="plugin-search"><Search size={14} /><input value={query} onChange={(event) => setQuery(event.target.value)} placeholder={t("plugins.search")} /></label>
      )}
      {installedError && <div className="plugin-error" role="alert">{installedError}</div>}
      {tab !== "installed" && tab !== "custom" && marketError && (
        <div className="plugin-error" role="alert">{t("plugins.marketError", { error: marketError })}</div>
      )}
      {installProgress && (
        <div
          className="plugin-install-progress"
          role="progressbar"
          aria-label={t(`plugins.progress.${installProgress.phase}`)}
          aria-valuemin={0}
          aria-valuemax={100}
          {...(installPercent === undefined ? {} : { "aria-valuenow": installPercent })}
        >
          <div className="plugin-install-progress-label">
            <strong>{t(`plugins.progress.${installProgress.phase}`)}</strong>
            <span>{installBytes ?? (installPercent === undefined ? "" : `${installPercent}%`)}</span>
          </div>
          <div className={`plugin-install-progress-track ${installPercent === undefined ? "indeterminate" : ""}`}>
            <span style={installPercent === undefined ? undefined : { width: `${installPercent}%` }} />
          </div>
        </div>
      )}
      {showSkeleton ? (
        <div className="plugin-skeletons"><span /><span /><span /></div>
      ) : tab === "custom" ? (
        <div className="plugin-custom">
          <Package size={28} />
          <h4>{t("plugins.customTitle")}</h4>
          <p>{t("plugins.customDescription")}</p>
          <div className="plugin-source-input">
            <input value={customSource} onChange={(event) => setCustomSource(event.target.value)} placeholder={t("plugins.customPlaceholder")} />
            <button type="button" disabled={!customSource.trim() || !!busy} onClick={requestCustomInstall}>{t("plugins.install")}</button>
          </div>
          <button className="plugin-folder-button" type="button" onClick={() => void pickNativeDirectory().then((path) => path && setCustomSource(path))}>
            <FolderOpen size={15} /> {t("plugins.chooseFolder")}
          </button>
        </div>
      ) : (
        <div className="plugin-list">
          {tab === "installed" ? visiblePlugins.map((plugin) => {
            const entry = marketById.get(plugin.id);
            const update = hasUpdate(plugin, entry);
            const updateSource = entry?.source ?? plugin.originalSource;
            return (
              <article className="plugin-card" key={plugin.id}>
                <span className="plugin-avatar">{initials(plugin.displayName)}</span>
                <div className="plugin-card-body">
                  <div className="plugin-card-title"><strong>{plugin.displayName}</strong>{plugin.version && <span>v{plugin.version}</span>}{entry && <span className={entry.tier === "official" ? "official" : "curated"}>{entry.tier === "official" ? t("plugins.official") : t("plugins.curated")}</span>}{plugin.hasErrors && <span className="error">{t("plugins.errorBadge")}</span>}</div>
                  <p>{capabilityText(plugin)}</p>
                  <div className="plugin-card-actions">
                    <button type="button" disabled={detailBusy} onClick={() => void openDetail(plugin.id)}>{t("plugins.details")}</button>
                    {update && updateSource && <button className="primary" type="button" disabled={!!busy} onClick={() => entry ? requestInstall(entry) : setConfirm({ kind: "install", label: plugin.displayName, source: updateSource })}>{t("plugins.update")}</button>}
                    <button className="danger-text" type="button" onClick={() => setConfirm({ kind: "remove", id: plugin.id, label: plugin.displayName })}>{t("plugins.remove")}</button>
                  </div>
                </div>
                <button className={`settings-toggle ${plugin.enabled ? "active" : ""}`} type="button" role="switch" aria-label={t("plugins.toggle", { name: plugin.displayName })} aria-checked={plugin.enabled} disabled={busy === `toggle:${plugin.id}`} onClick={() => void togglePlugin(plugin)}><span /></button>
              </article>
            );
          }) : (<>
          {tab === "official" && visibleCapabilities.map((capability) => {
            const running = capability.install.running;
            const percent = capability.install.percent;
            return (
              <article className="plugin-card market" key={capability.id}>
                <span className="plugin-avatar">{initials(capability.displayName)}</span>
                <div className="plugin-card-body">
                  <div className="plugin-card-title">
                    <strong>{capability.displayName}</strong>
                    {capability.version && <span>v{capability.version}</span>}
                    <span className="official">{t("plugins.official")}</span>
                    <span className={`capability-state ${capability.state}`}>
                      {t(CAPABILITY_STATE_KEYS[capability.state])}
                    </span>
                  </div>
                  <p>{capability.description}</p>
                  {running && (
                    <div
                      className="plugin-install-progress capability-progress"
                      role="progressbar"
                      aria-label={t("plugins.capability.installing")}
                      aria-valuemin={0}
                      aria-valuemax={100}
                      {...(percent === undefined ? {} : { "aria-valuenow": percent })}
                    >
                      <div className="plugin-install-progress-label">
                        <strong>
                          {capability.install.step
                            ? capabilityStepLabel(capability.install.step)
                            : t("plugins.capability.installing")}
                        </strong>
                        <span>{percent === undefined ? "" : `${percent}%`}</span>
                      </div>
                      <div className={`plugin-install-progress-track ${percent === undefined ? "indeterminate" : ""}`}>
                        <span style={percent === undefined ? undefined : { width: `${percent}%` }} />
                      </div>
                    </div>
                  )}
                  {!running && capability.install.error && (
                    <p className="plugin-capability-error">{capability.install.error}</p>
                  )}
                </div>
                <div className="plugin-market-actions">
                  <button
                    className="primary"
                    type="button"
                    disabled={!capability.supported || running || !!capabilityBusy || !!busy}
                    onClick={() => void installCapability(capability)}
                  >
                    {!capability.supported
                      ? t("plugins.capability.unsupportedAction")
                      : running
                        ? t("plugins.capability.installing")
                        : capability.state === "ready"
                          ? t("plugins.capability.reinstall")
                          : t("plugins.install")}
                  </button>
                </div>
              </article>
            );
          })}
          {visibleMarket.map((entry) => {
            const installed = installedById.get(entry.id);
            const update = marketplaceUpdateAvailable(installed, entry) || githubUpdates.get(entry.id)?.updateAvailable;
            return (
              <article className="plugin-card market" key={entry.id}>
                <span className="plugin-avatar">{initials(entry.displayName)}</span>
                <div className="plugin-card-body">
                  <div className="plugin-card-title"><strong>{entry.displayName}</strong>{entry.version && <span>v{entry.version}</span>}<span className={entry.tier === "official" ? "official" : "curated"}>{entry.tier === "official" ? t("plugins.official") : t("plugins.curated")}</span></div>
                  <p>{entry.description ?? t("plugins.noDescription")}</p>
                  {entry.keywords && <div className="plugin-keywords">{entry.keywords.slice(0, 5).map((keyword) => <span key={keyword}>{keyword}</span>)}</div>}
                </div>
                <div className="plugin-market-actions">
                  {entry.homepage && <button className="plugin-icon-button" type="button" aria-label={t("plugins.website")} onClick={() => void openExternalUrl(entry.homepage!)}><ExternalLink size={14} /></button>}
                  <button className={installed && !update ? "installed" : "primary"} type="button" disabled={(installed && !update) || !!busy} onClick={() => requestInstall(entry)}>
                    {installed ? update ? t("plugins.update") : t("plugins.installed") : t("plugins.install")}
                  </button>
                </div>
              </article>
            );
          })}
          </>)}
          {((tab === "installed" && visiblePlugins.length === 0) ||
            (tab === "official" && visibleCapabilities.length === 0 && visibleMarket.length === 0) ||
            (tab === "third-party" && visibleMarket.length === 0)) && !loading && (
            <div className="plugin-empty"><Package size={25} /><p>{normalizedQuery ? t("plugins.noResults") : t(tab === "installed" ? "plugins.noneInstalled" : "plugins.noneAvailable")}</p></div>
          )}
        </div>
      )}
      {notice && <div className="plugin-notice" role="status">{notice}<small>{t("plugins.newTaskHint")}</small></div>}
      {confirm && (
        <div className="plugin-confirm-backdrop" onMouseDown={(event) => event.target === event.currentTarget && setConfirm(undefined)}>
          <div className="plugin-confirm" role="alertdialog" aria-modal="true">
            <AlertTriangle size={22} />
            <h4>{t(confirm.kind === "remove" ? "plugins.removeTitle" : "plugins.trustTitle", { name: confirm.label })}</h4>
            <p>{t(confirm.kind === "remove" ? "plugins.removeWarning" : "plugins.trustWarning")}</p>
            <div><button type="button" autoFocus onClick={() => setConfirm(undefined)}>{t("common.cancel")}</button><button className="danger" type="button" onClick={() => { const state = confirm; setConfirm(undefined); if (state.kind === "remove" && state.id) void removePlugin(state.id, state.label); else if (state.source) void install(state.source, state.label); }}>{t(confirm.kind === "remove" ? "plugins.remove" : "plugins.trustInstall")}</button></div>
          </div>
        </div>
      )}
    </section>
  );
}
