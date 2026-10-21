// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: AGPL-3.0-only

/**
 * Provides the minimal callback-scoped operating-system credential adapter
 * used by Bootstrap and local Management clients.
 * @packageDocumentation
 */

import type { AsyncEntry } from "@napi-rs/keyring";
import { problem, type ProblemError } from "../errors.js";

/** Identifies one credential in the operating-system credential store. */
export interface OsCredentialKey {
  readonly service: string;
  readonly account: string;
}

/** Supplies callback-scoped access to an OS credential without a cache. */
export interface OsCredentialStore {
  /** Reports whether the exact credential currently exists. */
  exists(key: OsCredentialKey): Promise<boolean>;
  /** Replaces the exact credential in the OS store. */
  set(key: OsCredentialKey, secret: Uint8Array): Promise<void>;
  /** Deletes the exact credential and reports whether it existed. */
  delete(key: OsCredentialKey): Promise<boolean>;
  /** Loads one credential for the callback and clears the loaded bytes after use. */
  withCredential<T>(
    key: OsCredentialKey,
    use: (secret: Uint8Array) => Promise<T>,
  ): Promise<T>;
}

function invalidKeyProblem(): ProblemError {
  return problem('credentials.invalid_key','validation','系统凭据标识无效');
}
function unavailableProblem(): ProblemError {
  return problem('credentials.store_unavailable','unavailable','系统凭据存储不可用');
}
function missingProblem(): ProblemError {
  return problem('credentials.not_found','not_found','系统凭据不存在');
}

function assertKey(key: OsCredentialKey): void {
  if (
    typeof key.service !== "string" ||
    typeof key.account !== "string" ||
    key.service.length === 0 ||
    key.account.length === 0 ||
    key.service.length > 256 ||
    key.account.length > 256
  ) {
    throw invalidKeyProblem();
  }
}

async function entryFor(key: OsCredentialKey): Promise<AsyncEntry> {
  try {
    const { AsyncEntry } = await import("@napi-rs/keyring");
    return new AsyncEntry(key.service, key.account);
  } catch {
    throw unavailableProblem();
  }
}

async function loadSecret(key: OsCredentialKey): Promise<Uint8Array | undefined> {
  assertKey(key);
  try {
    const secret = await (await entryFor(key)).getSecret();
    return secret === undefined || secret === null
      ? undefined
      : Uint8Array.from(secret);
  } catch {
    throw unavailableProblem();
  }
}

/** Creates the adopted native OS credential/keyring adapter. */
export function createOsCredentialStore(): OsCredentialStore {
  return Object.freeze({
    async exists(key: OsCredentialKey): Promise<boolean> {
      const secret = await loadSecret(key);
      if (secret === undefined) return false;
      secret.fill(0);
      return true;
    },
    async set(key: OsCredentialKey, secret: Uint8Array): Promise<void> {
      assertKey(key);
      const copy = new Uint8Array(secret);
      try {
        await (await entryFor(key)).setSecret(copy);
      } catch {
        throw unavailableProblem();
      } finally {
        copy.fill(0);
      }
    },
    async delete(key: OsCredentialKey): Promise<boolean> {
      assertKey(key);
      try {
        return await (await entryFor(key)).deleteCredential();
      } catch {
        throw unavailableProblem();
      }
    },
    async withCredential<T>(
      key: OsCredentialKey,
      use: (secret: Uint8Array) => Promise<T>,
    ): Promise<T> {
      const secret = await loadSecret(key);
      if (secret === undefined) throw missingProblem();
      try {
        return await use(secret);
      } finally {
        secret.fill(0);
      }
    },
  });
}
