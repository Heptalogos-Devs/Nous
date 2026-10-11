// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { execFile } from "node:child_process";
import { lstat, readFile } from "node:fs/promises";
import {
  isAbsolute,
  join,
  matchesGlob,
  relative,
  resolve,
  sep,
} from "node:path";
import { promisify } from "node:util";
import { parse } from "smol-toml";
import { z } from "zod";

const language = z
  .strictObject({
    extensions: z.array(z.string().regex(/^\.[a-z]+$/)).nonempty(),
    warn_at: z.number().int().positive(),
    reject_at: z.number().int().positive(),
  })
  .refine((value) => value.warn_at < value.reject_at, {
    message: "warn_at must be less than reject_at",
  });
const configuration = z.strictObject({
  typescript: language,
  rust: language,
  generated: z.strictObject({ exclude: z.array(z.string().min(1)) }),
});
const decoder = new TextDecoder("utf-8", { fatal: true });

async function check(): Promise<number> {
  const root = process.cwd();
  const policy = configuration.parse(
    parse(
      decoder.decode(
        await readFile(join(root, ".config/scripts/code-length.toml")),
      ),
    ),
  );
  const extensions = new Map<string, z.infer<typeof language>>();
  for (const rules of [policy.typescript, policy.rust]) {
    for (const extension of rules.extensions) {
      if (extensions.has(extension))
        throw new Error(`Conflicting extension: ${extension}`);
      extensions.set(extension, rules);
    }
  }
  const git = promisify(execFile);
  const { stdout: prefix } = await git("git", ["rev-parse", "--show-prefix"], {
    cwd: root,
    windowsHide: true,
    encoding: "utf8",
  });
  if (prefix.trim())
    throw new Error("Run check:length from the repository root");
  const { stdout } = await git(
    "git",
    [
      "ls-files",
      "--cached",
      "--others",
      "--exclude-standard",
      "--deduplicate",
      "-z",
    ],
    {
      cwd: root,
      windowsHide: true,
      encoding: "buffer",
      maxBuffer: 16 * 1024 * 1024,
    },
  );
  const paths = decoder.decode(stdout).split("\0").filter(Boolean).sort();
  let checked = 0;
  let warnings = 0;
  let rejected = 0;
  let failures = 0;
  for (const path of paths) {
    const extension = path.slice(path.lastIndexOf("."));
    const rules = extensions.get(extension);
    if (
      !rules ||
      policy.generated.exclude.some((pattern) => matchesGlob(path, pattern))
    )
      continue;
    try {
      const absolute = resolve(root, path);
      const local = relative(root, absolute);
      if (isAbsolute(local) || local === ".." || local.startsWith(`..${sep}`))
        throw new Error("Source path is outside the repository");
      // Reject linked parents as well as linked files before reading source bytes.
      let current = root;
      let missing = false;
      for (const component of local.split(sep)) {
        current = join(current, component);
        const metadata = await lstat(current).catch(
          (error: NodeJS.ErrnoException) => {
            if (error.code === "ENOENT") return undefined;
            throw error;
          },
        );
        if (!metadata) {
          missing = true;
          break;
        }
        if (metadata.isSymbolicLink())
          throw new Error("Handwritten source cannot be a symlink");
        if (current === absolute && !metadata.isFile())
          throw new Error("Source is not a regular file");
      }
      if (missing) continue;
      const source = decoder.decode(await readFile(absolute));
      const lines =
        source.length === 0
          ? 0
          : source.split(/\r\n|\r|\n/).length - Number(/[\r\n]$/.test(source));
      checked++;
      const label =
        lines >= rules.reject_at
          ? "ERROR"
          : lines >= rules.warn_at
            ? "WARN"
            : undefined;
      if (!label) continue;
      if (label === "ERROR") rejected++;
      else warnings++;
      console.log(
        `${label} ${path} ${lines} lines (warn ${rules.warn_at}, reject ${rules.reject_at})`,
      );
    } catch (error) {
      failures++;
      console.error(
        `ERROR ${path}: ${error instanceof Error ? error.message : String(error)}`,
      );
    }
  }
  console.log(
    `Code length: ${checked} files, ${warnings} warnings, ${rejected} rejected, ${failures} read errors`,
  );
  return failures ? 2 : rejected ? 1 : 0;
}

try {
  process.exitCode = await check();
} catch (error) {
  console.error(
    `Code length: ${error instanceof Error ? error.message : String(error)}`,
  );
  process.exitCode = 2;
}
