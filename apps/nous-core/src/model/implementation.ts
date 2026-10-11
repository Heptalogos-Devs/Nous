// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { readFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { canonicalDigest } from "../digest.js";

type ImplementationDigests = { provider: string; material: string };
declare const nousModelImplementations: ImplementationDigests;

/** Source startup and Portable assembly use the same owning implementation bytes. */
export async function readModelImplementation(
  repo: string,
): Promise<ImplementationDigests> {
  const metadata = JSON.parse(
    await readFile(join(repo, "apps/nous-core/package.json"), "utf8"),
  ) as { dependencies: Record<string, string> };
  const libraries = Object.fromEntries(
    ["ai", "@ai-sdk/openai", "zod", "get-stream"].map((name) => [
      name,
      metadata.dependencies[name],
    ]),
  );
  const digest = async (files: string[]) =>
    canonicalDigest({
      libraries,
      sources: await Promise.all(
        files.map(async (path) => ({
          path,
          text: (
            await readFile(join(repo, "apps/nous-core/src/model", path), "utf8")
          ).replace(/\r\n/g, "\n"),
        })),
      ),
    });
  const [provider, material] = await Promise.all([
    digest([
      "invocations.ts",
      "execution/snapshot.ts",
      "execution/routes.ts",
      "protocols.ts",
      "input.ts",
      "identity.ts",
      "profiles.ts",
      "configuration.ts",
      "producer.ts",
      "schemas/provider.ts",
    ]),
    digest([
      "interpretation.ts",
      "derivation.ts",
      "input.ts",
      "schemas/material-interpretation.ts",
    ]),
  ]);
  return { provider, material };
}

export const modelImplementations: ImplementationDigests =
  typeof nousModelImplementations === "undefined"
    ? await readModelImplementation(
        resolve(dirname(fileURLToPath(import.meta.url)), "../../../.."),
      )
    : nousModelImplementations;
