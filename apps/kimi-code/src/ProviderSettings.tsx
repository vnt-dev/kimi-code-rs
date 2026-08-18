import { useEffect, useMemo, useRef, useState } from "react";
import {
  AlertTriangle,
  Check,
  ChevronDown,
  ChevronRight,
  Eye,
  EyeOff,
  KeyRound,
  Plus,
  RefreshCw,
  Server,
  Trash2,
} from "lucide-react";

import { t } from "./i18n";
import SettingsSelect from "./components/SettingsSelect";
import {
  PROVIDER_CAPABILITIES,
  PROVIDER_EFFORTS,
  PROVIDER_PROTOCOLS,
  createProviderDraft,
  createProviderModelDraft,
  providerDraft,
  saveProviderInput,
  validateProviderDraft,
  type ProviderDraft,
  type ProviderModelDraft,
  type ProviderProtocol,
  type ProviderSummary,
  type ProviderValidationError,
} from "./providers";
import { invoke } from "./transport";

const NEW_PROVIDER_ID = "__new_provider__";

function messageOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function validationMessage(error: ProviderValidationError): string {
  return t(`providers.error.${error}`);
}

function protocolLabel(protocol: ProviderProtocol): string {
  return t(`providers.type.${protocol}`);
}

function capabilityLabel(capability: string): string {
  switch (capability) {
    case "tool_use": return t("providers.capability.toolUse");
    case "thinking": return t("providers.capability.thinking");
    case "always_thinking": return t("providers.capability.alwaysThinking");
    case "image_in": return t("providers.capability.imageInput");
    case "video_in": return t("providers.capability.videoInput");
    case "audio_in": return t("providers.capability.audioInput");
    case "dynamically_loaded_tools": return t("providers.capability.dynamicTools");
    default: return capability;
  }
}

function effortLabel(effort: string): string {
  switch (effort) {
    case "low": return t("providers.effort.low");
    case "medium": return t("providers.effort.medium");
    case "high": return t("providers.effort.high");
    case "max": return t("providers.effort.max");
    default: return effort;
  }
}

function toggleValue(values: string[], value: string): string[] {
  return values.includes(value)
    ? values.filter((candidate) => candidate !== value)
    : [...values, value];
}

function ProviderForm({
  provider,
  guard,
  onGuardStay,
  onGuardDiscard,
  onChanged,
  onSaved,
  onDeleted,
  onCancel,
}: {
  provider?: ProviderSummary;
  guard: boolean;
  onGuardStay: () => void;
  onGuardDiscard: () => void;
  onChanged: (dirty: boolean) => void;
  onSaved: (id: string) => void;
  onDeleted: () => void;
  onCancel: () => void;
}) {
  const adding = provider === undefined;
  const managed = provider?.managed === true;
  const [draft, setDraft] = useState<ProviderDraft>(() =>
    provider ? providerDraft(provider) : createProviderDraft(),
  );
  const [error, setError] = useState<string>();
  const [busy, setBusy] = useState(false);
  const [showKey, setShowKey] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [expandedModels, setExpandedModels] = useState<Record<number, boolean>>(() =>
    Object.fromEntries(
      (provider ? providerDraft(provider) : createProviderDraft()).models
        .map((model, index) => [index, model.model.trim() === ""]),
    ),
  );
  const mutationRef = useRef(false);

  useEffect(() => {
    const next = provider ? providerDraft(provider) : createProviderDraft();
    setDraft(next);
    setExpandedModels(Object.fromEntries(
      next.models.map((model, index) => [index, model.model.trim() === ""]),
    ));
    setError(undefined);
    setShowKey(false);
    setConfirmDelete(false);
    onChanged(false);
  }, [provider, onChanged]);

  const isModelExpanded = (index: number): boolean =>
    expandedModels[index] ?? (draft.models[index]?.model.trim() ?? "") === "";

  const toggleModel = (index: number): void =>
    setExpandedModels((current) => ({ ...current, [index]: !isModelExpanded(index) }));

  const markChanged = (update: (current: ProviderDraft) => ProviderDraft): void => {
    setDraft(update);
    setError(undefined);
    onChanged(true);
  };

  const updateModel = (
    index: number,
    update: (current: ProviderModelDraft) => ProviderModelDraft,
  ): void => {
    markChanged((current) => ({
      ...current,
      models: current.models.map((model, modelIndex) =>
        modelIndex === index ? update(model) : model,
      ),
    }));
  };

  const save = async (): Promise<void> => {
    if (mutationRef.current || managed) return;
    const validation = validateProviderDraft(draft, adding);
    if (validation) {
      setError(validationMessage(validation));
      return;
    }
    mutationRef.current = true;
    setBusy(true);
    setError(undefined);
    try {
      const saved = await invoke<ProviderSummary>("save_provider", {
        input: saveProviderInput(draft, provider?.id),
      });
      onChanged(false);
      onSaved(saved.id);
    } catch (saveError) {
      setError(messageOf(saveError));
    } finally {
      mutationRef.current = false;
      setBusy(false);
    }
  };

  const remove = async (): Promise<void> => {
    if (!provider || mutationRef.current || managed) return;
    mutationRef.current = true;
    setBusy(true);
    setError(undefined);
    try {
      await invoke("delete_provider", { id: provider.id });
      onChanged(false);
      onDeleted();
    } catch (removeError) {
      setError(messageOf(removeError));
    } finally {
      mutationRef.current = false;
      setBusy(false);
    }
  };

  if (managed) {
    return (
      <div className="provider-form provider-managed">
        <div className="provider-managed-icon"><KeyRound size={18} /></div>
        <div>
          <strong>{t("providers.managedTitle")}</strong>
          <p>{t("providers.managedDescription")}</p>
        </div>
        <dl>
          <div><dt>{t("providers.fieldBaseUrl")}</dt><dd>{provider.baseUrl ?? "—"}</dd></div>
          <div><dt>{t("providers.fieldModels")}</dt><dd>{provider.models.length}</dd></div>
        </dl>
      </div>
    );
  }

  return (
    <div className="provider-form">
      {guard && (
        <div className="provider-guard" role="alert">
          <AlertTriangle size={16} />
          <span>{t("providers.unsaved")}</span>
          <button type="button" onClick={onGuardStay}>{t("providers.keepEditing")}</button>
          <button className="danger" type="button" onClick={onGuardDiscard}>{t("providers.discard")}</button>
        </div>
      )}
      {error && <div className="provider-error" role="alert"><AlertTriangle size={15} />{error}</div>}

      <div className="provider-fields-grid">
        <label>
          <span>{t("providers.fieldName")} <b>*</b></span>
          <input
            value={draft.id}
            maxLength={64}
            disabled={busy}
            placeholder={t("providers.namePlaceholder")}
            onChange={(event) => markChanged((current) => ({ ...current, id: event.target.value }))}
          />
        </label>
        <label>
          <span>{t("providers.fieldProtocol")} <b>*</b></span>
          <SettingsSelect<ProviderProtocol>
            className="provider-select provider-protocol-select"
            value={draft.type}
            disabled={busy}
            ariaLabel={t("providers.fieldProtocol")}
            options={PROVIDER_PROTOCOLS.map((protocol) => ({
              value: protocol,
              label: protocolLabel(protocol),
              description: t(`providers.typeDescription.${protocol}`),
            }))}
            onChange={(type) => markChanged((current) => ({
              ...current,
              type,
            }))}
          />
        </label>
      </div>

      <label className="provider-field">
        <span>{t("providers.fieldApiKey")} {adding && <b>*</b>}</span>
        <div className="provider-secret-input">
          <input
            type={showKey ? "text" : "password"}
            autoComplete="new-password"
            value={draft.apiKey}
            disabled={busy}
            placeholder={provider?.hasApiKey && !draft.replaceApiKey
              ? t("providers.keyPreserved")
              : t("providers.keyPlaceholder")}
            onChange={(event) => markChanged((current) => ({
              ...current,
              apiKey: event.target.value,
              replaceApiKey: true,
            }))}
          />
          <button
            type="button"
            aria-label={t(showKey ? "providers.hideKey" : "providers.showKey")}
            onClick={() => setShowKey((value) => !value)}
          >
            {showKey ? <EyeOff size={15} /> : <Eye size={15} />}
          </button>
        </div>
        {provider?.hasApiKey && (
          <button
            className="provider-clear-key"
            type="button"
            disabled={busy}
            onClick={() => markChanged((current) => ({
              ...current,
              apiKey: "",
              replaceApiKey: !current.replaceApiKey,
            }))}
          >
            {draft.replaceApiKey && !draft.apiKey
              ? t("providers.keepSavedKey")
              : t("providers.clearSavedKey")}
          </button>
        )}
      </label>

      <label className="provider-field">
        <span>{t("providers.fieldBaseUrl")} <b>*</b></span>
        <input
          value={draft.baseUrl}
          disabled={busy}
          placeholder="https://api.example.com/v1"
          onChange={(event) => markChanged((current) => ({ ...current, baseUrl: event.target.value }))}
        />
      </label>

      <div className="provider-models-heading">
        <div><strong>{t("providers.fieldModels")}</strong></div>
        <button
          type="button"
          disabled={busy}
          onClick={() => {
            setExpandedModels((current) => ({ ...current, [draft.models.length]: true }));
            markChanged((current) => ({
              ...current,
              models: [...current.models, createProviderModelDraft()],
            }));
          }}
        >
          <Plus size={14} /> {t("providers.addModel")}
        </button>
      </div>
      <div className="provider-models">
        <div className="provider-models-col-header" aria-hidden="true">
          <span />
          <span>{t("providers.modelId")}</span>
          <span>{t("providers.displayName")}</span>
          <span>{t("providers.contextSize")}</span>
          <span />
        </div>
        {draft.models.map((model, index) => {
          const expanded = isModelExpanded(index);
          return (
            <div className={`provider-model-entry ${expanded ? "expanded" : "collapsed"}`} key={index}>
              <div className="provider-model-card-header">
                <button
                  className="provider-model-toggle"
                  type="button"
                  aria-expanded={expanded}
                  aria-label={t(expanded ? "providers.collapseModel" : "providers.expandModel")}
                  onClick={() => toggleModel(index)}
                >
                  {expanded ? <ChevronDown size={15} /> : <ChevronRight size={15} />}
                </button>
                <input
                  aria-label={t("providers.modelId")}
                  value={model.model}
                  disabled={busy}
                  placeholder="model-id"
                  onChange={(event) => updateModel(index, (current) => ({ ...current, model: event.target.value }))}
                />
                <input
                  aria-label={t("providers.displayName")}
                  value={model.displayName}
                  disabled={busy}
                  placeholder={t("providers.optional")}
                  onChange={(event) => updateModel(index, (current) => ({ ...current, displayName: event.target.value }))}
                />
                <input
                  aria-label={t("providers.contextSize")}
                  value={model.maxContextSize}
                  disabled={busy}
                  placeholder="256K"
                  onChange={(event) => updateModel(index, (current) => ({ ...current, maxContextSize: event.target.value }))}
                />
                <button
                  className="provider-remove-model"
                  type="button"
                  aria-label={t("providers.removeModel")}
                  disabled={busy || draft.models.length === 1}
                  onClick={() => {
                    setExpandedModels((current) => {
                      const next: Record<number, boolean> = {};
                      for (const [key, value] of Object.entries(current)) {
                        const modelIndex = Number(key);
                        if (modelIndex === index) continue;
                        next[modelIndex > index ? modelIndex - 1 : modelIndex] = value;
                      }
                      return next;
                    });
                    markChanged((current) => ({
                      ...current,
                      models: current.models.filter((_, modelIndex) => modelIndex !== index),
                    }));
                  }}
                ><Trash2 size={14} /></button>
              </div>
              {expanded && (
              <div className="provider-model-options">
              <div className="provider-model-option">
                <div className="provider-model-option-label">
                  <strong>{t("providers.modelCapabilities")}</strong>
                </div>
                <div className="provider-option-chips">
                  {Array.from(new Set([...PROVIDER_CAPABILITIES, ...model.capabilities])).map((capability) => (
                    <button
                      key={capability}
                      type="button"
                      className={model.capabilities.includes(capability) ? "active" : ""}
                      aria-pressed={model.capabilities.includes(capability)}
                      disabled={busy}
                      onClick={() => updateModel(index, (current) => ({
                        ...current,
                        capabilities: toggleValue(current.capabilities, capability),
                      }))}
                    >
                      {model.capabilities.includes(capability) && <Check size={12} />}
                      {capabilityLabel(capability)}
                    </button>
                  ))}
                </div>
              </div>
              <div className="provider-model-option provider-effort-option">
                <div className="provider-model-option-label">
                  <strong>{t("providers.supportEfforts")}</strong>
                </div>
                <div className="provider-option-chips">
                  {Array.from(new Set([...PROVIDER_EFFORTS, ...model.supportEfforts])).map((effort) => (
                    <button
                      key={effort}
                      type="button"
                      className={model.supportEfforts.includes(effort) ? "active" : ""}
                      aria-pressed={model.supportEfforts.includes(effort)}
                      disabled={busy}
                      onClick={() => updateModel(index, (current) => {
                        const supportEfforts = toggleValue(current.supportEfforts, effort);
                        return {
                          ...current,
                          supportEfforts,
                          defaultEffort: supportEfforts.includes(current.defaultEffort)
                            ? current.defaultEffort
                            : "",
                          capabilities: supportEfforts.length > 0 &&
                            !current.capabilities.includes("thinking") &&
                            !current.capabilities.includes("always_thinking")
                            ? [...current.capabilities, "thinking"]
                            : current.capabilities,
                        };
                      })}
                    >
                      {model.supportEfforts.includes(effort) && <Check size={12} />}
                      {effortLabel(effort)}
                    </button>
                  ))}
                  <div className="provider-effort-default">
                    <span>{t("providers.defaultEffort")}</span>
                    <SettingsSelect
                      className="provider-select provider-effort-select"
                      value={model.defaultEffort}
                      disabled={busy || model.supportEfforts.length === 0}
                      ariaLabel={t("providers.defaultEffort")}
                      options={[
                        { value: "", label: t("providers.automaticEffort") },
                        ...model.supportEfforts.map((effort) => ({
                          value: effort,
                          label: effortLabel(effort),
                        })),
                      ]}
                      onChange={(defaultEffort) => updateModel(index, (current) => ({
                        ...current,
                        defaultEffort,
                      }))}
                    />
                  </div>
                </div>
              </div>
              </div>
              )}
            </div>
          );
        })}
      </div>

      <label className="provider-field provider-default-model">
        <span>{t("providers.defaultModel")}</span>
        <SettingsSelect
          className="provider-select provider-model-select"
          value={draft.defaultModel}
          disabled={busy}
          ariaLabel={t("providers.defaultModel")}
          options={[
            { value: "", label: t("providers.firstModelDefault") },
            ...draft.models.filter((model) => model.model.trim()).map((model) => ({
              value: model.model.trim(),
              label: model.displayName.trim() || model.model.trim(),
              description: model.displayName.trim() ? model.model.trim() : undefined,
            })),
          ]}
          onChange={(defaultModel) => markChanged((current) => ({ ...current, defaultModel }))}
        />
      </label>

      <div className="provider-form-actions">
        {adding ? (
          <button type="button" disabled={busy} onClick={onCancel}>{t("common.cancel")}</button>
        ) : confirmDelete ? (
          <div className="provider-delete-confirm">
            <span>{t("providers.deleteConfirm", { name: provider.id })}</span>
            <button type="button" disabled={busy} onClick={() => setConfirmDelete(false)}>{t("common.cancel")}</button>
            <button className="danger" type="button" disabled={busy} onClick={() => void remove()}>{t("providers.confirmDelete")}</button>
          </div>
        ) : (
          <button className="danger-text" type="button" disabled={busy} onClick={() => setConfirmDelete(true)}>
            <Trash2 size={14} /> {t("providers.delete")}
          </button>
        )}
        {!confirmDelete && <span className="spacer" />}
        {!confirmDelete && (
          <button className="primary" type="button" disabled={busy} onClick={() => void save()}>
            {busy ? <RefreshCw className="spinning" size={14} /> : <Check size={14} />}
            {t(adding ? "providers.add" : "providers.save")}
          </button>
        )}
      </div>
    </div>
  );
}

export default function ProviderSettings({ onChanged }: { onChanged: () => void }) {
  const [providers, setProviders] = useState<ProviderSummary[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string>();
  const [openId, setOpenId] = useState<string>();
  const [mountedId, setMountedId] = useState<string>();
  const [dirty, setDirty] = useState(false);
  const [guardTarget, setGuardTarget] = useState<string | undefined>();

  const load = async (): Promise<ProviderSummary[]> => {
    setLoading(true);
    setError(undefined);
    try {
      const next = await invoke<ProviderSummary[]>("list_providers");
      setProviders(next);
      return next;
    } catch (loadError) {
      setError(messageOf(loadError));
      return [];
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    void load();
  }, []);

  useEffect(() => {
    if (openId) setMountedId(openId);
  }, [openId]);

  const selectedProvider = useMemo(
    () => providers.find((provider) => provider.id === mountedId),
    [mountedId, providers],
  );

  const requestOpen = (id: string | undefined): void => {
    const target = openId === id ? undefined : id;
    if (dirty) {
      setGuardTarget(target ?? "");
      return;
    }
    setOpenId(target);
    setGuardTarget(undefined);
  };

  const discardAndContinue = (): void => {
    const target = guardTarget;
    setDirty(false);
    setGuardTarget(undefined);
    setOpenId(target || undefined);
  };

  const finishMutation = async (id?: string): Promise<void> => {
    const next = await load();
    setDirty(false);
    setGuardTarget(undefined);
    setOpenId(id && next.some((provider) => provider.id === id) ? id : undefined);
    if (id) setMountedId(id);
    onChanged();
  };

  return (
    <section className="provider-settings" aria-labelledby="providers-heading">
      <div className="provider-settings-heading">
        <div>
          <h3 id="providers-heading">{t("providers.title")}</h3>
          <p>{t("providers.description")}</p>
        </div>
        <button type="button" disabled={loading} onClick={() => requestOpen(NEW_PROVIDER_ID)}>
          <Plus size={15} /> {t("providers.add")}
        </button>
      </div>

      {error && <div className="provider-error" role="alert"><AlertTriangle size={15} />{error}</div>}
      {loading ? (
        <div className="provider-loading"><RefreshCw className="spinning" size={16} />{t("providers.loading")}</div>
      ) : (
        <div className="provider-list">
          {(openId === NEW_PROVIDER_ID || mountedId === NEW_PROVIDER_ID) && (
            <article className={`provider-item add ${openId === NEW_PROVIDER_ID ? "open" : ""}`}>
              <button className="provider-summary" type="button" onClick={() => requestOpen(NEW_PROVIDER_ID)}>
                <span className="provider-icon"><Plus size={16} /></span>
                <strong>{t("providers.addTitle")}</strong>
                <span className="grow" />
                <ChevronRight size={16} />
              </button>
              <div className="provider-panel"><div>
                <ProviderForm
                  guard={guardTarget !== undefined && openId === NEW_PROVIDER_ID}
                  onGuardStay={() => setGuardTarget(undefined)}
                  onGuardDiscard={discardAndContinue}
                  onChanged={setDirty}
                  onSaved={(id) => void finishMutation(id)}
                  onDeleted={() => undefined}
                  onCancel={() => requestOpen(undefined)}
                />
              </div></div>
            </article>
          )}
          {providers.map((provider) => {
            const open = openId === provider.id;
            const mounted = mountedId === provider.id;
            return (
              <article className={`provider-item ${open ? "open" : ""}`} key={provider.id}>
                <button className="provider-summary" type="button" onClick={() => requestOpen(provider.id)}>
                  <span className="provider-icon"><Server size={16} /></span>
                  <span className="provider-name"><strong>{provider.id}</strong><small>{provider.baseUrl ?? t("providers.noBaseUrl")}</small></span>
                  <span className="provider-protocol">{protocolLabel(provider.type)}</span>
                  {provider.managed && <span className="provider-managed-badge">OAuth</span>}
                  <span className={`provider-key-state ${provider.hasApiKey || provider.managed ? "configured" : ""}`}>
                    {provider.hasApiKey || provider.managed ? t("providers.configured") : t("providers.unconfigured")}
                  </span>
                  <span className="provider-model-count">{t("providers.modelCount", { count: provider.models.length })}</span>
                  <ChevronRight size={16} />
                </button>
                <div className="provider-panel"><div>
                  {mounted && (
                    <ProviderForm
                      provider={selectedProvider}
                      guard={guardTarget !== undefined && open}
                      onGuardStay={() => setGuardTarget(undefined)}
                      onGuardDiscard={discardAndContinue}
                      onChanged={setDirty}
                      onSaved={(id) => void finishMutation(id)}
                      onDeleted={() => void finishMutation()}
                      onCancel={() => requestOpen(undefined)}
                    />
                  )}
                </div></div>
              </article>
            );
          })}
          {providers.length === 0 && openId !== NEW_PROVIDER_ID && (
            <div className="provider-empty"><Server size={24} /><p>{t("providers.empty")}</p><button type="button" onClick={() => requestOpen(NEW_PROVIDER_ID)}>{t("providers.add")}</button></div>
          )}
        </div>
      )}
    </section>
  );
}
