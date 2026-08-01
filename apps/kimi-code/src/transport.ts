import { getVersion as tauriGetVersion } from "@tauri-apps/api/app";
import { invoke as tauriInvoke, isTauri } from "@tauri-apps/api/core";
import {
  listen as tauriListen,
  type Event as TauriEvent,
  type UnlistenFn,
} from "@tauri-apps/api/event";
import { open as tauriOpen } from "@tauri-apps/plugin-dialog";
import { openUrl as tauriOpenUrl } from "@tauri-apps/plugin-opener";

export interface AgentEventScope {
  sessionId: string;
  agentId: string;
}

export interface TransportEvent<T> {
  event: string;
  payload: T;
}

export interface ReplayResetEvent {
  scopes: AgentEventScope[];
}

export const TRANSPORT_REPLAY_RESET = "transport-replay-reset";
export const TRANSPORT_AUTH_REQUIRED = "transport-auth-required";

interface RpcResponse<T> {
  id: string;
  ok: boolean;
  result?: T;
  error?: {
    code: string;
    message: string;
    details?: unknown;
  };
}

interface StoredCredential {
  version: 1;
  credential: string;
  expiresAt: number;
}

interface WebMeta {
  serverVersion: string;
  websocket: boolean;
  fileUpload: boolean;
  fsBrowse: boolean;
}

interface WebSubscription {
  scope: AgentEventScope;
  remoteId?: string;
}

type Listener<T = unknown> = (
  event: TransportEvent<T>,
) => void | Promise<void>;

const CREDENTIAL_KEY = "kimi-code.web-credential";
const CREDENTIAL_TTL_MS = 7 * 24 * 60 * 60 * 1000;
const WS_PROTOCOL_PREFIX = "kimi-code.bearer.";

let webCredential = initializeWebCredential();

export function credentialFromFragment(hash: string): string | undefined {
  return new URLSearchParams(hash.replace(/^#/, "")).get("token") || undefined;
}

export function credentialFromStoredValue(
  raw: string | null,
  now = Date.now(),
): string | undefined {
  if (!raw) return undefined;
  try {
    const stored = JSON.parse(raw) as Partial<StoredCredential>;
    return stored.version === 1 &&
      typeof stored.credential === "string" &&
      !!stored.credential &&
      typeof stored.expiresAt === "number" &&
      stored.expiresAt > now
      ? stored.credential
      : undefined;
  } catch {
    return undefined;
  }
}

export function storedCredentialValue(
  credential: string,
  now = Date.now(),
): string {
  const stored: StoredCredential = {
    version: 1,
    credential,
    expiresAt: now + CREDENTIAL_TTL_MS,
  };
  return JSON.stringify(stored);
}

function initializeWebCredential(): string | undefined {
  if (typeof window === "undefined") return undefined;
  const fragment = credentialFromFragment(window.location.hash);
  if (fragment) {
    const url = new URL(window.location.href);
    url.hash = "";
    window.history.replaceState(
      window.history.state,
      "",
      `${url.pathname}${url.search}`,
    );
    persistCredential(fragment);
    return fragment;
  }
  try {
    const raw = window.localStorage.getItem(CREDENTIAL_KEY);
    const credential = credentialFromStoredValue(raw);
    if (!credential) {
      window.localStorage.removeItem(CREDENTIAL_KEY);
      return undefined;
    }
    return credential;
  } catch {
    return undefined;
  }
}

function persistCredential(credential: string): void {
  try {
    window.localStorage.setItem(CREDENTIAL_KEY, storedCredentialValue(credential));
  } catch {
    // Keeping the credential in memory is enough for the current tab.
  }
}

function newId(): string {
  return typeof crypto.randomUUID === "function"
    ? crypto.randomUUID()
    : `${Date.now()}-${Math.random().toString(36).slice(2)}`;
}

function rpcError(response: RpcResponse<unknown>): Error {
  const error = new Error(response.error?.message ?? "RPC request failed");
  Object.assign(error, {
    code: response.error?.code,
    details: response.error?.details,
  });
  return error;
}

class WebTransport {
  private socket?: WebSocket;
  private connectionId?: string;
  private readyPromise?: Promise<string>;
  private resolveReady?: (connectionId: string) => void;
  private rejectReady?: (error: Error) => void;
  private reconnectTimer?: number;
  private reconnectAttempt = 0;
  private connectedOnce = false;
  private closed = false;
  private readonly listeners = new Map<string, Set<Listener>>();
  private readonly subscriptions = new Map<string, WebSubscription>();

  listen<T>(event: string, handler: Listener<T>): Promise<UnlistenFn> {
    const listeners = this.listeners.get(event) ?? new Set<Listener>();
    listeners.add(handler as Listener);
    this.listeners.set(event, listeners);
    if (webCredential) void this.ensureReady().catch(() => undefined);
    return Promise.resolve(() => {
      listeners.delete(handler as Listener);
      if (listeners.size === 0) this.listeners.delete(event);
    });
  }

  async invoke<T>(command: string, args: Record<string, unknown> = {}): Promise<T> {
    const connectionId = await this.ensureReady();
    return this.invokeOnConnection<T>(connectionId, command, args);
  }

  async subscribeAgentEvents(scope: AgentEventScope): Promise<string> {
    const id = newId();
    const subscription: WebSubscription = { scope };
    this.subscriptions.set(id, subscription);
    try {
      subscription.remoteId = await this.invoke<string>(
        "subscribe_agent_events",
        { ...scope },
      );
      return id;
    } catch (error) {
      this.subscriptions.delete(id);
      throw error;
    }
  }

  async unsubscribeAgentEvents(id: string): Promise<void> {
    const subscription = this.subscriptions.get(id);
    this.subscriptions.delete(id);
    if (!subscription?.remoteId || !this.connectionId) return;
    await this.invokeOnConnection<void>(
      this.connectionId,
      "unsubscribe_agent_events",
      { subscriptionId: subscription.remoteId },
    );
  }

  async uploadFile(file: Blob, filename: string): Promise<unknown> {
    const connectionId = await this.ensureReady();
    const data = new FormData();
    data.append("file", file, filename);
    const response = await fetch("/_kimi/v1/files", {
      method: "POST",
      headers: this.authHeaders(connectionId),
      body: data,
    });
    const payload = (await response.json()) as RpcResponse<unknown>;
    if (response.status === 401) this.requireAuthentication();
    if (!response.ok || !payload.ok) throw rpcError(payload);
    return payload.result;
  }

  setCredential(credential: string): void {
    webCredential = credential.trim() || undefined;
    if (webCredential) persistCredential(webCredential);
    this.closeSocket();
    this.closed = false;
    void this.ensureReady().catch(() => undefined);
  }

  credentialRequired(): boolean {
    return !webCredential;
  }

  async meta(): Promise<WebMeta> {
    if (!webCredential) {
      this.emit(TRANSPORT_AUTH_REQUIRED, undefined);
      throw new Error("Web server credential is required");
    }
    const response = await fetch("/_kimi/v1/meta", {
      headers: { authorization: `Bearer ${webCredential}` },
    });
    if (response.status === 401) this.requireAuthentication();
    if (!response.ok) throw new Error("Unable to load server metadata");
    return (await response.json()) as WebMeta;
  }

  private async invokeOnConnection<T>(
    connectionId: string,
    command: string,
    args: Record<string, unknown>,
  ): Promise<T> {
    const request = { id: newId(), command, args };
    const response = await fetch("/_kimi/v1/rpc", {
      method: "POST",
      headers: {
        ...this.authHeaders(connectionId),
        "content-type": "application/json",
      },
      body: JSON.stringify(request),
    });
    const payload = (await response.json()) as RpcResponse<T>;
    if (response.status === 401) this.requireAuthentication();
    if (!response.ok || !payload.ok) throw rpcError(payload);
    return payload.result as T;
  }

  private authHeaders(connectionId: string): Record<string, string> {
    return {
      authorization: `Bearer ${webCredential ?? ""}`,
      "x-kimi-connection-id": connectionId,
    };
  }

  private ensureReady(): Promise<string> {
    if (!webCredential) {
      this.emit(TRANSPORT_AUTH_REQUIRED, undefined);
      return Promise.reject(new Error("Web server credential is required"));
    }
    if (this.connectionId) return Promise.resolve(this.connectionId);
    if (this.readyPromise) return this.readyPromise;

    this.closed = false;
    this.readyPromise = new Promise<string>((resolve, reject) => {
      this.resolveReady = resolve;
      this.rejectReady = reject;
    });
    void this.connectSocket();
    return this.readyPromise;
  }

  private async connectSocket(): Promise<void> {
    try {
      await this.meta();
    } catch (error) {
      if (this.readyPromise) {
        this.rejectReady?.(
          error instanceof Error ? error : new Error(String(error)),
        );
        this.readyPromise = undefined;
        this.resolveReady = undefined;
        this.rejectReady = undefined;
      }
      if (!this.closed && webCredential) this.scheduleReconnect();
      return;
    }
    if (this.closed || this.connectionId || this.socket) return;
    const scheme = window.location.protocol === "https:" ? "wss:" : "ws:";
    const socket = new WebSocket(
      `${scheme}//${window.location.host}/_kimi/v1/events`,
      [`${WS_PROTOCOL_PREFIX}${webCredential}`],
    );
    this.socket = socket;
    socket.onmessage = (message) => {
      if (this.socket === socket) this.handleMessage(String(message.data));
    };
    socket.onerror = () => {
      // onclose supplies the reconnect transition.
    };
    socket.onclose = () => {
      if (this.socket === socket) this.handleClose();
    };
  }

  private handleMessage(raw: string): void {
    let frame: Record<string, unknown>;
    try {
      frame = JSON.parse(raw) as Record<string, unknown>;
    } catch {
      return;
    }
    if (frame.type === "ready" && typeof frame.connectionId === "string") {
      const reconnect = this.connectedOnce;
      this.connectedOnce = true;
      this.connectionId = frame.connectionId;
      this.reconnectAttempt = 0;
      this.resolveReady?.(frame.connectionId);
      this.resolveReady = undefined;
      this.rejectReady = undefined;
      if (reconnect) void this.restoreSubscriptions(frame.connectionId);
      return;
    }
    if (frame.type === "event" && typeof frame.event === "string") {
      this.emit(frame.event, frame.payload);
    }
  }

  private async restoreSubscriptions(connectionId: string): Promise<void> {
    const scopes = [...this.subscriptions.values()].map(({ scope }) => scope);
    await this.emitAsync(TRANSPORT_REPLAY_RESET, { scopes });
    for (const subscription of this.subscriptions.values()) {
      subscription.remoteId = undefined;
      try {
        subscription.remoteId = await this.invokeOnConnection<string>(
          connectionId,
          "subscribe_agent_events",
          { ...subscription.scope },
        );
      } catch {
        // A later reconnect or an explicit session prepare will retry it.
      }
    }
  }

  private handleClose(): void {
    const error = new Error("WebSocket connection closed");
    this.connectionId = undefined;
    this.socket = undefined;
    this.readyPromise = undefined;
    this.rejectReady?.(error);
    this.resolveReady = undefined;
    this.rejectReady = undefined;
    for (const subscription of this.subscriptions.values()) {
      subscription.remoteId = undefined;
    }
    if (!this.closed && webCredential) this.scheduleReconnect();
  }

  private scheduleReconnect(): void {
    if (this.reconnectTimer !== undefined) return;
    const delay = Math.min(30_000, 500 * 2 ** this.reconnectAttempt);
    this.reconnectAttempt += 1;
    this.reconnectTimer = window.setTimeout(() => {
      this.reconnectTimer = undefined;
      void this.ensureReady().catch(() => undefined);
    }, delay);
  }

  private requireAuthentication(): void {
    webCredential = undefined;
    try {
      window.localStorage.removeItem(CREDENTIAL_KEY);
    } catch {
      // Memory state still prevents reuse of the rejected credential.
    }
    this.emit(TRANSPORT_AUTH_REQUIRED, undefined);
    this.closeSocket();
  }

  private closeSocket(): void {
    this.closed = true;
    if (this.reconnectTimer !== undefined) {
      window.clearTimeout(this.reconnectTimer);
      this.reconnectTimer = undefined;
    }
    const socket = this.socket;
    this.socket = undefined;
    this.connectionId = undefined;
    this.rejectReady?.(new Error("WebSocket connection closed"));
    this.resolveReady = undefined;
    this.rejectReady = undefined;
    this.readyPromise = undefined;
    socket?.close();
  }

  private emit(event: string, payload: unknown): void {
    for (const listener of this.listeners.get(event) ?? []) {
      void listener({ event, payload });
    }
  }

  private async emitAsync(event: string, payload: unknown): Promise<void> {
    await Promise.all(
      [...(this.listeners.get(event) ?? [])].map((listener) =>
        listener({ event, payload }),
      ),
    );
  }
}

const webTransport = new WebTransport();

export function isDesktop(): boolean {
  return isTauri();
}

export function invoke<T>(
  command: string,
  args: Record<string, unknown> = {},
): Promise<T> {
  return isDesktop()
    ? tauriInvoke<T>(command, args)
    : webTransport.invoke<T>(command, args);
}

export function listen<T>(
  event: string,
  handler: (event: TransportEvent<T>) => void,
): Promise<UnlistenFn> {
  return isDesktop()
    ? tauriListen<T>(event, handler as (event: TauriEvent<T>) => void)
    : webTransport.listen(event, handler);
}

export function subscribeAgentEventsTransport(
  scope: AgentEventScope,
): Promise<string> {
  return isDesktop()
    ? tauriInvoke<string>("subscribe_agent_events", { ...scope })
    : webTransport.subscribeAgentEvents(scope);
}

export function unsubscribeAgentEventsTransport(id: string): Promise<void> {
  return isDesktop()
    ? tauriInvoke<void>("unsubscribe_agent_events", { subscriptionId: id })
    : webTransport.unsubscribeAgentEvents(id);
}

export async function uploadFileTransport(
  file: Blob,
  filename: string,
): Promise<unknown> {
  if (!isDesktop()) return webTransport.uploadFile(file, filename);
  return tauriInvoke("upload_file", {
    filename,
    mediaType: file.type || "application/octet-stream",
    bytes: Array.from(new Uint8Array(await file.arrayBuffer())),
  });
}

export function setWebCredential(credential: string): void {
  webTransport.setCredential(credential);
}

export function webCredentialRequired(): boolean {
  return !isDesktop() && webTransport.credentialRequired();
}

export async function getAppVersion(): Promise<string> {
  if (isDesktop()) return tauriGetVersion();
  const meta = await webTransport.meta();
  return meta.serverVersion;
}

export async function pickNativeDirectory(): Promise<string | undefined> {
  if (!isDesktop()) return undefined;
  const selected = await tauriOpen({ directory: true, multiple: false });
  return typeof selected === "string" ? selected : undefined;
}

export function openExternalUrl(url: string): Promise<void> {
  if (isDesktop()) return tauriOpenUrl(url);
  window.open(url, "_blank", "noopener,noreferrer");
  return Promise.resolve();
}
