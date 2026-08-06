import {
  isPermissionGranted,
  requestPermission,
  type Options,
} from "@tauri-apps/plugin-notification";

import {
  compactNotificationText,
  conversationNotificationText,
  shouldNotifyConversation,
} from "./notificationUtils";
import { invoke, isDesktop, listen } from "./transport";

export { compactNotificationText, shouldNotifyConversation };

const NOTIFICATIONS_ENABLED_STORAGE_KEY = "kimi-code.notifications-enabled";
const DEFAULT_NOTIFICATIONS_ENABLED = true;

export type ConversationNotificationKind =
  | "completed"
  | "question"
  | "approval"
  | "planReview";

export interface ConversationNotification {
  sessionId: string;
  conversationTitle?: string;
  kind: ConversationNotificationKind;
  content?: string;
}

let permissionRequest: Promise<boolean> | undefined;

export function loadNotificationsEnabled(): boolean {
  if (typeof window === "undefined") return DEFAULT_NOTIFICATIONS_ENABLED;
  try {
    const stored = window.localStorage.getItem(
      NOTIFICATIONS_ENABLED_STORAGE_KEY,
    );
    return stored === null ? DEFAULT_NOTIFICATIONS_ENABLED : stored === "true";
  } catch {
    return DEFAULT_NOTIFICATIONS_ENABLED;
  }
}

export function saveNotificationsEnabled(enabled: boolean): void {
  if (typeof window === "undefined") return;
  try {
    window.localStorage.setItem(
      NOTIFICATIONS_ENABLED_STORAGE_KEY,
      String(enabled),
    );
  } catch {
    // Storage failures should not prevent the setting from changing in memory.
  }
}

export function notificationOptions({
  sessionId,
  conversationTitle,
  kind,
  content,
}: ConversationNotification): Options {
  const text = conversationNotificationText(conversationTitle, content);
  return {
    ...text,
    autoCancel: true,
    group: sessionId,
    extra: { sessionId, kind },
  };
}

export async function ensureNotificationPermission(
  retry = false,
): Promise<boolean> {
  if (!isDesktop()) return false;
  if (retry) permissionRequest = undefined;
  permissionRequest ??= (async () => {
    if (await isPermissionGranted()) return true;
    return (await requestPermission()) === "granted";
  })();
  return permissionRequest;
}

export async function sendConversationNotification(
  notification: ConversationNotification,
): Promise<boolean> {
  if (!(await ensureNotificationPermission())) return false;
  const options = notificationOptions(notification);
  if (!options.body) return false;
  await invoke<void>("show_conversation_notification", {
    sessionId: notification.sessionId,
    title: options.title ?? "Kimi Code",
    body: options.body ?? "",
  });
  return true;
}

export async function listenForNotificationActions(
  onOpenSession: (sessionId: string) => void,
): Promise<(() => void) | undefined> {
  if (!isDesktop()) return undefined;
  return listen<string>("notification-open-session", (event) => {
    if (event.payload) onOpenSession(event.payload);
  });
}
