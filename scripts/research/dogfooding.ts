// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { readFile } from "node:fs/promises";
import { createHash } from "node:crypto";
import { parseArgs } from "node:util";
import { z } from "zod";
import { runCore } from "../../apps/nous-core/src/main.js";
import { ResourceRegistry } from "../../apps/nous-core/src/resources/registry.js";
import {
  ResourceProviderError,
  type ExternalResourceAdapter,
  type ResourceBinding,
  type ResourceRecord,
  type StableExternalRef,
} from "../../apps/nous-core/src/resources/adapter.js";

const entrySchema = z.object({
  id: z.string().min(1),
  version: z.string().min(1),
  title: z.string(),
  content: z.string().max(1048576),
  accessible: z.boolean(),
});
const digest = (text: string) =>
  createHash("sha256").update(text).digest("hex");

/** Research-only mutable source; the production Core owns all admission and cognition. */
class LocalDocuments implements ExternalResourceAdapter {
  readonly profileDigest = digest("nous-dogfooding-local-documents-v1");
  constructor(private readonly file: string) {}
  private async entries() {
    return z
      .array(entrySchema)
      .parse(JSON.parse(await readFile(this.file, "utf8")));
  }
  async describe(_binding: ResourceBinding) {
    const entries = await this.entries();
    return {
      accessible: entries.some((e) => e.accessible),
    };
  }
  async search(
    binding: ResourceBinding,
    _question: string,
    limit: number,
  ): Promise<ResourceRecord[]> {
    return (await this.entries())
      .filter((e) => e.accessible)
      .slice(0, limit)
      .map((entry, index) => ({
        resourceRef: binding.resourceRef,
        reference: {
          providerKind: binding.adapterKind,
          providerProfile: binding.providerProfile,
          profileDigest: this.profileDigest,
          resourceRef: binding.resourceRef,
          providerResourceId: binding.providerLocator,
          entryId: entry.id,
          entryVersion: entry.version,
          contentDigest: digest(entry.content),
          sourceLocator: `local-resource://dogfooding/${entry.id}`,
          retrievedAt: new Date().toISOString(),
          accessScope: binding.providerLocator,
        },
        title: entry.title,
        content: entry.content,
        providerRank: index + 1,
        versionStatus: "current",
        accessStatus: "allowed",
      }));
  }
  async checkVersion(reference: StableExternalRef) {
    const entry = (await this.entries()).find(
      (e) => e.id === reference.entryId,
    );
    return !entry
      ? ("missing" as const)
      : entry.version === reference.entryVersion &&
          digest(entry.content) === reference.contentDigest
        ? ("current" as const)
        : ("stale" as const);
  }
  async checkAccess(reference: StableExternalRef) {
    const entry = (await this.entries()).find(
      (e) => e.id === reference.entryId,
    );
    return entry?.accessible ? ("allowed" as const) : ("denied" as const);
  }
  async materialize(reference: StableExternalRef) {
    const entry = (await this.entries()).find(
      (e) => e.id === reference.entryId,
    );
    if (
      !entry ||
      entry.version !== reference.entryVersion ||
      digest(entry.content) !== reference.contentDigest
    )
      throw new ResourceProviderError("stale", "Source changed or was deleted");
    if (!entry.accessible)
      throw new ResourceProviderError("denied", "Source access revoked");
    return {
      content: entry.content,
      mediaType: "text/markdown",
      versionStatus: "current" as const,
      accessStatus: "allowed" as const,
    };
  }
}

const { values } = parseArgs({
  options: {
    locator: { type: "string" },
    documents: { type: "string" },
    "stop-on-stdin-close": { type: "boolean", default: false },
  },
});
if (!values.locator || !values.documents)
  throw new Error(
    "Use --locator <research instance> --documents <local documents JSON>",
  );
await runCore(
  {
    locator: values.locator,
    development: true,
    "stop-on-stdin-close": values["stop-on-stdin-close"],
  },
  new ResourceRegistry([
    {
      adapterKind: "local-documents",
      providerProfile: "dogfooding",
      adapter: new LocalDocuments(values.documents),
    },
  ]),
);
