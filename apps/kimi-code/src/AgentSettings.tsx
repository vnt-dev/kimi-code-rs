import { AlertCircle, RefreshCw } from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";

import SettingsSelect from "./components/SettingsSelect";
import { t } from "./i18n";
import { invoke } from "./transport";
import type { Model, PermissionMode } from "./types";

interface AgentSettingsData {
  defaultModel?: string;
  defaultPermission: PermissionMode;
  defaultThinking: boolean;
  defaultPlanMode: boolean;
}

type AgentSettingsPatch = Partial<AgentSettingsData>;

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export default function AgentSettings() {
  const [settings, setSettings] = useState<AgentSettingsData>();
  const [models, setModels] = useState<Model[]>([]);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string>();

  const load = useCallback(async (): Promise<void> => {
    setLoading(true);
    setError(undefined);
    const [settingsResult, modelsResult] = await Promise.allSettled([
      invoke<AgentSettingsData>("get_agent_settings"),
      invoke<Model[]>("list_models"),
    ]);
    if (settingsResult.status === "fulfilled") {
      setSettings(settingsResult.value);
    } else {
      setError(errorMessage(settingsResult.reason));
    }
    if (modelsResult.status === "fulfilled") {
      setModels(modelsResult.value);
    }
    setLoading(false);
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const modelOptions = useMemo(
    () =>
      models.map((model) => ({
        value: model.id,
        label: model.displayName,
        description:
          model.displayName === model.id
            ? model.providerId
            : `${model.id} · ${model.providerId}`,
      })),
    [models],
  );

  const permissionOptions = (["manual", "auto", "yolo"] as const).map(
    (mode) => ({
      value: mode,
      label: t(`permission.${mode}`),
      description: t(`permission.${mode}Desc`),
    }),
  );

  const update = async (patch: AgentSettingsPatch): Promise<void> => {
    if (!settings || saving) return;
    const previous = settings;
    setSettings({ ...settings, ...patch });
    setSaving(true);
    setError(undefined);
    try {
      const saved = await invoke<AgentSettingsData>("update_agent_settings", {
        patch,
      });
      setSettings(saved);
      if (patch.defaultModel) {
        setModels((current) =>
          current.map((model) => ({
            ...model,
            isDefault: model.id === saved.defaultModel,
          })),
        );
      }
    } catch (nextError) {
      setSettings(previous);
      setError(errorMessage(nextError));
    } finally {
      setSaving(false);
    }
  };

  if (loading && !settings) {
    return (
      <div className="settings-agent-state">
        <span className="spinner" />
        {t("settings.agentLoading")}
      </div>
    );
  }

  if (!settings) {
    return (
      <div className="settings-agent-state error" role="alert">
        <AlertCircle size={15} />
        <span>{error ?? t("settings.agentUnavailable")}</span>
        <button type="button" onClick={() => void load()}>
          <RefreshCw size={13} />
          {t("settings.agentRetry")}
        </button>
      </div>
    );
  }

  return (
    <div className="settings-agent">
      <section className="settings-section" aria-labelledby="agent-defaults-heading">
        <div className="settings-agent-heading">
          <h3 id="agent-defaults-heading">{t("settings.agentDefaults")}</h3>
          {saving && <span>{t("settings.agentSaving")}</span>}
        </div>

        {error && (
          <div className="settings-agent-error" role="alert">
            <AlertCircle size={14} />
            <span>{error}</span>
          </div>
        )}

        <div className="settings-agent-card">
          <div className="settings-row">
            <div>
              <span className="settings-row-label">{t("settings.defaultModel")}</span>
              <small>{t("settings.defaultModelHint")}</small>
            </div>
            {modelOptions.length > 0 ? (
              <SettingsSelect
                className="settings-agent-select"
                value={settings.defaultModel ?? modelOptions[0].value}
                options={modelOptions}
                ariaLabel={t("settings.defaultModel")}
                disabled={saving}
                onChange={(defaultModel) => void update({ defaultModel })}
              />
            ) : (
              <span className="settings-agent-empty-value">
                {settings.defaultModel ?? t("settings.noDefaultModel")}
              </span>
            )}
          </div>

          <div className="settings-row">
            <div>
              <span className="settings-row-label">{t("settings.defaultPermission")}</span>
              <small>{t("settings.defaultPermissionHint")}</small>
            </div>
            <SettingsSelect
              className="settings-agent-select"
              value={settings.defaultPermission}
              options={permissionOptions}
              ariaLabel={t("settings.defaultPermission")}
              disabled={saving}
              onChange={(defaultPermission) => void update({ defaultPermission })}
            />
          </div>

          <div className="settings-row">
            <div>
              <span className="settings-row-label">{t("settings.defaultThinking")}</span>
              <small>{t("settings.defaultThinkingHint")}</small>
            </div>
            <button
              className={`settings-toggle ${settings.defaultThinking ? "active" : ""}`}
              type="button"
              role="switch"
              aria-label={t("settings.defaultThinking")}
              aria-checked={settings.defaultThinking}
              disabled={saving}
              onClick={() => void update({ defaultThinking: !settings.defaultThinking })}
            >
              <span />
            </button>
          </div>

          <div className="settings-row">
            <div>
              <span className="settings-row-label">{t("settings.defaultPlanMode")}</span>
              <small>{t("settings.defaultPlanModeHint")}</small>
            </div>
            <button
              className={`settings-toggle ${settings.defaultPlanMode ? "active" : ""}`}
              type="button"
              role="switch"
              aria-label={t("settings.defaultPlanMode")}
              aria-checked={settings.defaultPlanMode}
              disabled={saving}
              onClick={() => void update({ defaultPlanMode: !settings.defaultPlanMode })}
            >
              <span />
            </button>
          </div>
        </div>
      </section>
    </div>
  );
}
