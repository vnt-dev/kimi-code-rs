import {
  LogIn,
  LogOut,
  RefreshCw,
  Settings as SettingsIcon,
  Sparkles,
} from "lucide-react";

import { resolveAccountMenuVisibility } from "../accountMenu";
import { localeTag, t } from "../i18n";
import type { AccountUsage, ManagedUsageRow } from "../types";

export function AccountUsagePopover({
  appVersion,
  loggedIn,
  usage,
  busy,
  error,
  onRefresh,
  onLogin,
  onOpenSettings,
  onSignOut,
}: {
  appVersion?: string;
  loggedIn: boolean;
  usage?: AccountUsage;
  busy: boolean;
  error?: string;
  onRefresh: () => void;
  onLogin: () => void;
  onOpenSettings: () => void;
  onSignOut: () => void;
}) {
  const visibility = resolveAccountMenuVisibility(loggedIn);
  const rows = usage
    ? [...(usage.summary ? [usage.summary] : []), ...usage.limits]
    : [];

  return (
    <div
      id="account-popover"
      className="profile-popover"
      role="dialog"
      aria-label={
        visibility.showUsage
          ? t("account.usageTitle")
          : t("account.openMenu")
      }
    >
      <div className="profile-popover-header">
        <div className="profile-identity">
          <span className="profile-identity-mark">
            <Sparkles size={14} />
          </span>
          <span className="profile-identity-copy">
            <span className="profile-identity-title">
              <strong>Kimi Code</strong>
              {appVersion && <small>v{appVersion}</small>}
            </span>
          </span>
        </div>
        {visibility.showUsage && (
          <button
            className="profile-refresh"
            type="button"
            title={t("account.refreshUsage")}
            aria-label={t("account.refreshUsage")}
            disabled={busy}
            onClick={onRefresh}
          >
            <RefreshCw className={busy ? "spinning" : ""} size={13} />
          </button>
        )}
      </div>

      {visibility.showUsage && (
        <div className="account-usage-content" aria-live="polite">
          <div className="account-usage-heading">
            <span>{t("account.planUsage")}</span>
            {busy && usage && <small>{t("account.updating")}</small>}
          </div>

          {busy && !usage ? (
            <div className="account-usage-skeleton" aria-label={t("account.loadingUsage")}>
              <i />
              <i />
            </div>
          ) : rows.length > 0 ? (
            <div className="account-usage-list">
              {rows.map((row, index) => (
                <ManagedUsageProgress
                  key={`${row.label}-${String(index)}`}
                  row={row}
                  primary={index === 0 && usage?.summary !== null}
                />
              ))}
            </div>
          ) : (
            <div className="account-usage-empty">
              {error ? t("account.usageError") : t("account.noUsage")}
            </div>
          )}

          {error && (
            <div className="account-usage-error">
              <span>{error}</span>
              <button type="button" disabled={busy} onClick={onRefresh}>
                {t("common.retry")}
              </button>
            </div>
          )}

          {usage?.extraUsage && (
            <BoosterWalletSummary wallet={usage.extraUsage} />
          )}
        </div>
      )}

      <div className="profile-popover-footer">
        {visibility.showLogin && (
          <button className="profile-login" type="button" onClick={onLogin}>
            <LogIn size={14} />
            {t("account.login")}
          </button>
        )}
        <button
          className="profile-settings"
          type="button"
          onClick={onOpenSettings}
        >
          <SettingsIcon size={14} />
          {t("settings.title")}
        </button>
        {visibility.showSignOut && (
          <button className="profile-signout" type="button" onClick={onSignOut}>
            <LogOut size={14} />
            {t("account.signOut")}
          </button>
        )}
      </div>
    </div>
  );
}

function ManagedUsageProgress({
  row,
  primary,
}: {
  row: ManagedUsageRow;
  primary: boolean;
}) {
  const used = Math.max(0, row.used);
  const limit = Math.max(0, row.limit);
  const ratio = limit > 0 ? Math.min(1, used / limit) : 0;
  const percentage = Math.round(ratio * 100);
  const level = ratio >= 0.9 ? "danger" : ratio >= 0.72 ? "warning" : "";

  return (
    <div className={`managed-usage-row ${primary ? "primary" : ""}`}>
      <div className="managed-usage-label">
        <strong>{formatUsageLabel(row.label)}</strong>
        <span>{percentage}%</span>
      </div>
      <div
        className="managed-usage-track"
        role="progressbar"
        aria-label={row.label}
        aria-valuemin={0}
        aria-valuemax={limit}
        aria-valuenow={Math.min(used, limit)}
      >
        <i
          className={level}
          style={{ width: `${String(ratio * 100)}%` }}
        />
      </div>
      {row.resetHint && (
        <div className="managed-usage-meta">
          <span>{formatResetHint(row.resetHint)}</span>
        </div>
      )}
    </div>
  );
}

function BoosterWalletSummary({
  wallet,
}: {
  wallet: NonNullable<AccountUsage["extraUsage"]>;
}) {
  const hasMonthlyLimit =
    wallet.monthlyChargeLimitEnabled && wallet.monthlyChargeLimitCents > 0;
  const monthlyRatio = hasMonthlyLimit
    ? Math.min(1, wallet.monthlyUsedCents / wallet.monthlyChargeLimitCents)
    : 0;

  return (
    <div className="booster-wallet">
      <div className="account-usage-heading">
        <span>{t("account.extraUsage")}</span>
        <small>Booster</small>
      </div>
      <div className="booster-balance">
        <span>{t("account.balance")}</span>
        <strong>{formatCurrency(wallet.balanceCents, wallet.currency)}</strong>
      </div>
      <div className="booster-details">
        <span>
          {t("account.monthlyUsed", { amount: formatCurrency(wallet.monthlyUsedCents, wallet.currency) })}
        </span>
        <span>
          {hasMonthlyLimit
            ? t("account.monthlyLimit", { amount: formatCurrency(wallet.monthlyChargeLimitCents, wallet.currency) })
            : t("account.monthlyLimitUnlimited")}
        </span>
      </div>
      {hasMonthlyLimit && (
        <div className="managed-usage-track compact" aria-hidden="true">
          <i
            className={monthlyRatio >= 0.9 ? "danger" : monthlyRatio >= 0.72 ? "warning" : ""}
            style={{ width: `${String(monthlyRatio * 100)}%` }}
          />
        </div>
      )}
    </div>
  );
}

function formatUsageLabel(label: string): string {
  const normalized = label.trim().toLowerCase();
  if (normalized === "weekly limit") return t("usage.weeklyLimit");
  return label
    .replace(/^(\d+)h limit$/i, t("usage.hoursLimit", { count: "$1" }))
    .replace(/^(\d+)d limit$/i, t("usage.daysLimit", { count: "$1" }))
    .replace(/^(\d+)m limit$/i, t("usage.minutesLimit", { count: "$1" }));
}

function formatResetHint(hint: string): string {
  if (hint === "reset") return t("usage.resetDone");
  if (hint.startsWith("resets in "))
    return t("usage.resetsIn", { time: hint.slice(10) });
  if (hint.startsWith("resets at "))
    return t("usage.resetsAt", { time: hint.slice(10) });
  return hint;
}

function formatCurrency(cents: number, currency: string): string {
  try {
    return new Intl.NumberFormat(localeTag(), {
      style: "currency",
      currency: currency || "USD",
      currencyDisplay: "narrowSymbol",
    }).format(cents / 100);
  } catch {
    return `${(cents / 100).toFixed(2)} ${currency}`;
  }
}
