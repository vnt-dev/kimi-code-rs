import { useEffect, useRef } from "react";
import { LogIn, LogOut } from "lucide-react";

import { AccountAvatar } from "./components/AccountAvatar";
import {
  BoosterWalletSummary,
  ManagedUsageProgress,
} from "./components/AccountUsagePopover";
import { t } from "./i18n";
import type { AccountUsage, AuthStatus, ManagedUserInfo } from "./types";

export default function AccountSettings({
  auth,
  profile,
  usage,
  usageBusy,
  usageError,
  onRefreshUsage,
  onLogin,
  onSignOut,
}: {
  auth: AuthStatus;
  profile?: ManagedUserInfo;
  usage?: AccountUsage;
  usageBusy: boolean;
  usageError?: string;
  onRefreshUsage: () => void;
  onLogin: () => void;
  onSignOut: () => void;
}) {
  const requested = useRef(false);
  const rows = usage
    ? [...(usage.summary ? [usage.summary] : []), ...usage.limits]
    : [];

  useEffect(() => {
    if (auth.loggedIn && !requested.current) {
      requested.current = true;
      onRefreshUsage();
    }
  }, [auth.loggedIn, onRefreshUsage]);

  return (
    <>
      <section
        className="settings-section"
        aria-labelledby="account-profile-heading"
      >
        <h3 id="account-profile-heading">{t("settings.tabAccount")}</h3>
        <div className="settings-account-card">
          {auth.loggedIn && <AccountAvatar profile={profile} size={40} />}
          <div className="settings-account-meta">
            <span className="settings-account-name-row">
              <strong className="settings-account-name">
                {auth.loggedIn
                  ? profile?.nickname || t("account.defaultUserName")
                  : t("account.login")}
              </strong>
              {auth.loggedIn && profile?.userLevelName && (
                <span className="settings-account-badge">
                  {profile.userLevelName}
                </span>
              )}
            </span>
            <small className="settings-account-sub">
              {auth.loggedIn
                ? t("account.signedIn")
                : t("account.signedOutHint")}
            </small>
          </div>
          {auth.loggedIn ? (
            <button
              className="settings-account-action danger"
              type="button"
              onClick={onSignOut}
            >
              <LogOut size={14} />
              {t("account.signOut")}
            </button>
          ) : (
            <button
              className="settings-account-action primary"
              type="button"
              onClick={onLogin}
            >
              <LogIn size={14} />
              {t("account.login")}
            </button>
          )}
        </div>
      </section>

      {auth.loggedIn && (
        <section
          className="settings-section"
          aria-labelledby="account-usage-heading"
        >
          <h3 id="account-usage-heading">{t("account.planUsage")}</h3>
          {usageBusy && !usage ? (
            <div
              className="account-usage-skeleton"
              aria-label={t("account.loadingUsage")}
            >
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
              {usageError ? t("account.usageError") : t("account.noUsage")}
            </div>
          )}
          {usageError && (
            <div className="account-usage-error">
              <span>{usageError}</span>
              <button type="button" disabled={usageBusy} onClick={onRefreshUsage}>
                {t("common.retry")}
              </button>
            </div>
          )}
          {usage?.extraUsage && (
            <BoosterWalletSummary wallet={usage.extraUsage} />
          )}
        </section>
      )}
    </>
  );
}
