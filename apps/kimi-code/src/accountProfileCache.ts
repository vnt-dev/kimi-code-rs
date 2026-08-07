import type { AccountProfile, ManagedUserInfo } from "./types";

const CACHE_KEY_PREFIX = "kimi-code.account-profile.v1:";
const CACHE_VERSION = 1;
const CACHE_MAX_AGE_MS = 30 * 24 * 60 * 60 * 1000;

export interface AccountProfileStorage {
  readonly length: number;
  getItem(key: string): string | null;
  key(index: number): string | null;
  removeItem(key: string): void;
  setItem(key: string, value: string): void;
}

interface CachedAccountProfile {
  version: typeof CACHE_VERSION;
  provider: string;
  updatedAt: number;
  profile: AccountProfile;
}

function browserStorage(): AccountProfileStorage | undefined {
  try {
    return typeof window === "undefined" ? undefined : window.localStorage;
  } catch {
    return undefined;
  }
}

function cacheKey(provider: string): string {
  return `${CACHE_KEY_PREFIX}${provider}`;
}

function optionalString(value: unknown): string | undefined {
  return typeof value === "string" ? value : undefined;
}

function parseProfile(value: unknown): AccountProfile | undefined {
  if (!value || typeof value !== "object") return undefined;
  const profile = value as Record<string, unknown>;
  if (
    typeof profile.userId !== "string" ||
    typeof profile.nickname !== "string" ||
    typeof profile.userLevel !== "number" ||
    typeof profile.userLevelName !== "string"
  ) {
    return undefined;
  }
  return {
    userId: profile.userId,
    nickname: profile.nickname,
    userLevel: profile.userLevel,
    userLevelName: profile.userLevelName,
    avatar: optionalString(profile.avatar),
    username: optionalString(profile.username),
  };
}

export function accountProfileFromUserInfo(
  userInfo: ManagedUserInfo,
): AccountProfile {
  return {
    userId: userInfo.userId,
    nickname: userInfo.nickname,
    userLevel: userInfo.userLevel,
    userLevelName: userInfo.userLevelName,
    avatar: userInfo.avatar,
    username: userInfo.username,
  };
}

export function readCachedAccountProfile(
  provider: string,
  storage: AccountProfileStorage | undefined = browserStorage(),
  now = Date.now(),
): AccountProfile | undefined {
  if (!storage) return undefined;
  const key = cacheKey(provider);
  try {
    const raw = storage.getItem(key);
    if (!raw) return undefined;
    const cached = JSON.parse(raw) as Partial<CachedAccountProfile>;
    const profile = parseProfile(cached.profile);
    if (
      cached.version !== CACHE_VERSION ||
      cached.provider !== provider ||
      typeof cached.updatedAt !== "number" ||
      now - cached.updatedAt > CACHE_MAX_AGE_MS ||
      cached.updatedAt > now ||
      !profile
    ) {
      storage.removeItem(key);
      return undefined;
    }
    return profile;
  } catch {
    try {
      storage.removeItem(key);
    } catch {
      // Storage access is best-effort.
    }
    return undefined;
  }
}

export function writeCachedAccountProfile(
  provider: string,
  profile: AccountProfile,
  storage: AccountProfileStorage | undefined = browserStorage(),
  now = Date.now(),
): void {
  if (!storage) return;
  const cached: CachedAccountProfile = {
    version: CACHE_VERSION,
    provider,
    updatedAt: now,
    profile,
  };
  try {
    storage.setItem(cacheKey(provider), JSON.stringify(cached));
  } catch {
    // Profile display remains functional when persistence is unavailable.
  }
}

export function clearCachedAccountProfiles(
  storage: AccountProfileStorage | undefined = browserStorage(),
): void {
  if (!storage) return;
  try {
    for (let index = storage.length - 1; index >= 0; index -= 1) {
      const key = storage.key(index);
      if (key?.startsWith(CACHE_KEY_PREFIX)) storage.removeItem(key);
    }
  } catch {
    // Storage access is best-effort.
  }
}
