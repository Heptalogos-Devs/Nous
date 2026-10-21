// Copyright 2026 Aravine Zhu; SPDX-License-Identifier: AGPL-3.0-only
import type { z } from 'zod';
import type { CallContext } from '@nous-wave/contracts/common';
import type {
  ConfigScope,
  JsonObject,
  FieldMetadata,
  AuthorizationRequest,
} from '@nous-wave/contracts/settings';
import type { Kysely } from 'kysely';
import type { SettingsDatabase } from './migration.js';
export * from '@nous-wave/contracts/settings';

export interface ValidationContext {
  ctx: CallContext;
  namespace: string;
  scope: ConfigScope;
  /** The current short transaction; validation must not wait for external effects. */
  db: Kysely<SettingsDatabase>;
}
export interface SettingsModule<T = JsonObject> {
  id: string;
  definitionRevision: string;
  owner: string;
  schema: z.ZodType<T>;
  defaults: Readonly<T>;
  fields: Record<string, FieldMetadata>;
  validate?(value: T, context: ValidationContext): Promise<void>;
}

export type SettingsAuthorize = (
  ctx: CallContext,
  request: AuthorizationRequest,
  client?: Kysely<SettingsDatabase>,
) => Promise<void> | void;
