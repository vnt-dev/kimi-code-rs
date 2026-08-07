import assert from "node:assert/strict";
import test from "node:test";

import {
  accountProfileFromUserInfo,
  clearCachedAccountProfiles,
  readCachedAccountProfile,
  writeCachedAccountProfile,
  type AccountProfileStorage,
} from "../src/accountProfileCache.ts";
import type { ManagedUserInfo } from "../src/types.ts";

class MemoryStorage implements AccountProfileStorage {
  private readonly values = new Map<string, string>();

  get length(): number {
    return this.values.size;
  }

  getItem(key: string): string | null {
    return this.values.get(key) ?? null;
  }

  key(index: number): string | null {
    return [...this.values.keys()][index] ?? null;
  }

  removeItem(key: string): void {
    this.values.delete(key);
  }

  setItem(key: string, value: string): void {
    this.values.set(key, value);
  }
}

const userInfo: ManagedUserInfo = {
  userId: "user-1",
  nickname: "Kimi User",
  status: "active",
  region: "cn",
  userLevel: 3,
  userLevelName: "Allegro",
  domain: 1,
  domainName: "kimi",
  avatar: "https://example.com/avatar.png",
  username: "kimi-user",
  email: "private@example.com",
  phone: { countryCode: "+86", number: "13800000000" },
  bio: "private bio",
};

test("account profile cache persists only display-safe fields", () => {
  const storage = new MemoryStorage();
  const profile = accountProfileFromUserInfo(userInfo);

  writeCachedAccountProfile("kimi-code", profile, storage, 1_000);

  assert.deepEqual(readCachedAccountProfile("kimi-code", storage, 2_000), profile);
  const persisted = storage.key(0);
  assert.ok(persisted);
  const raw = storage.getItem(persisted);
  assert.ok(raw);
  assert.equal(raw.includes(userInfo.email ?? ""), false);
  assert.equal(raw.includes(userInfo.phone?.number ?? ""), false);
  assert.equal(raw.includes(userInfo.bio ?? ""), false);
});

test("account profile cache rejects expired and malformed entries", () => {
  const storage = new MemoryStorage();
  const profile = accountProfileFromUserInfo(userInfo);
  writeCachedAccountProfile("kimi-code", profile, storage, 1_000);

  const afterThirtyOneDays = 1_000 + 31 * 24 * 60 * 60 * 1_000;
  assert.equal(
    readCachedAccountProfile("kimi-code", storage, afterThirtyOneDays),
    undefined,
  );
  assert.equal(storage.length, 0);

  storage.setItem("kimi-code.account-profile.v1:kimi-code", "not-json");
  assert.equal(readCachedAccountProfile("kimi-code", storage), undefined);
  assert.equal(storage.length, 0);
});

test("clearing account profiles preserves unrelated frontend settings", () => {
  const storage = new MemoryStorage();
  const profile = accountProfileFromUserInfo(userInfo);
  writeCachedAccountProfile("kimi-code", profile, storage);
  writeCachedAccountProfile("another-provider", profile, storage);
  storage.setItem("kimi-code.language", "zh-CN");

  clearCachedAccountProfiles(storage);

  assert.equal(readCachedAccountProfile("kimi-code", storage), undefined);
  assert.equal(readCachedAccountProfile("another-provider", storage), undefined);
  assert.equal(storage.getItem("kimi-code.language"), "zh-CN");
});
