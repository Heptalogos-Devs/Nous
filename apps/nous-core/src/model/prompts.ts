// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { open, realpath } from "node:fs/promises";
import { isAbsolute, relative, resolve, sep } from "node:path";
import { createHash } from "node:crypto";
import type { ModelRole } from "./roles.js";

const MAX_PROMPT_BYTES = 128 * 1024;
const defaults: Partial<Record<ModelRole, string>> = {
  query_concept_enrichment: "query/concept-enrichment.md",
  projection_steward: "projection/steward.md",
  memory_formation: "memory/formation.md",
  episode_segmentation: "episode/segmentation.md",
  journal_synthesis: "journal/synthesis.md",
  memory_consolidation: "memory/consolidation.md",
  concept_maintenance: "memory/concept-maintenance.md",
  material_description: "material/description.md",
  material_structuring: "material/structure.md",
  material_direct_structuring: "material/direct-structure.md",
};
function digest(value: string): string {
  return createHash("sha256").update(value).digest("hex");
}
export type PromptAsset = { id: string; digest: string; text: string };
export class PromptRegistry {
  constructor(
    private readonly root: string,
    private readonly overrideRoot?: string,
  ) {}
  async load(
    role: ModelRole,
    path = defaults[role],
  ): Promise<PromptAsset | undefined> {
    if (!path) return undefined;
    const custom = /^config-prompts[\\/]/.test(path);
    if (custom && !this.overrideRoot)
      throw new Error("Configuration Prompt root is unavailable");
    const root = await realpath(custom ? this.overrideRoot! : this.root);
    // Config paths use the documented repository-relative prompts/ prefix.
    const local = path.replace(
      custom ? /^config-prompts[\\/]/ : /^prompts[\\/]/,
      "",
    );
    const target = await realpath(resolve(root, local));
    const rel = relative(root, target);
    if (isAbsolute(rel) || rel === ".." || rel.startsWith(`..${sep}`) || !rel)
      throw new Error("Prompt path is outside the allowed root");
    const handle = await open(target, "r");
    try {
      const stat = await handle.stat();
      if (!stat.isFile() || stat.size > MAX_PROMPT_BYTES)
        throw new Error("Prompt exceeds 128 KiB or is not a file");
      const buffer = Buffer.alloc(MAX_PROMPT_BYTES + 1);
      let size = 0;
      while (size < buffer.length) {
        const { bytesRead } = await handle.read(
          buffer,
          size,
          buffer.length - size,
          null,
        );
        if (!bytesRead) break;
        size += bytesRead;
      }
      if (size > MAX_PROMPT_BYTES) throw new Error("Prompt exceeds 128 KiB");
      const bytes = buffer.subarray(0, size);
      const text = new TextDecoder("utf-8", { fatal: true }).decode(bytes);
      if (!text.trim()) throw new Error("Empty prompt asset");
      return {
        id: `${custom ? "config" : "program"}/${rel.split(sep).join("/")}`,
        digest: digest(text),
        text,
      };
    } finally {
      await handle.close();
    }
  }
}
