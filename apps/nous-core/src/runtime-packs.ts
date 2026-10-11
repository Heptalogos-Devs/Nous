// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { hostSchema } from "./configuration/catalog.js";
import { createHash, randomUUID } from "node:crypto";
import { createReadStream, createWriteStream } from "node:fs";
import { mkdir, readFile, rename, rm, stat } from "node:fs/promises";
import { dirname, join, resolve, sep } from "node:path";
import { Readable, Transform } from "node:stream";
import { pipeline } from "node:stream/promises";
import { open as openZip, type Entry, type ZipFile } from "yauzl";
import { z } from "zod";
import type { RuntimeLocations } from "./locations.js";

const MAX_RUNTIME_ARCHIVE_BYTES = 512 * 1024 * 1024;
const MAX_RUNTIME_EXTRACTED_BYTES = 2 * 1024 * 1024 * 1024;
const MAX_RUNTIME_ARCHIVE_ENTRIES = 10000;
const component = z.enum(["node", "postgresql", "ffmpeg"]);
const checksum = z.string().regex(/^[a-f0-9]{64}$/);
const relativePath = z
  .string()
  .min(1)
  .max(1024)
  .refine(
    (value) =>
      !value.startsWith("/") &&
      !value.startsWith("\\") &&
      !value.includes(":") &&
      value
        .split(/[\\/]/)
        .every(
          (part) =>
            part !== ".." &&
            part !== "." &&
            !!part &&
            !/[. ]$/.test(part) &&
            !/^(con|prn|aux|nul|com[1-9]|lpt[1-9])(?:\.|$)/i.test(part),
        ),
    "Unsafe pack path",
  );
const inventoryEntry = z.strictObject({ path: relativePath, sha256: checksum });
const packSchema = z.strictObject({
  component,
  version: z.string().min(1).max(128),
  platform: z.string().min(1),
  arch: z.string().min(1),
  license: z.string().min(1),
  source: z.string().min(1),
  required_executables: z.array(relativePath).min(1).max(16),
  files: z.array(inventoryEntry).min(1).max(MAX_RUNTIME_ARCHIVE_ENTRIES),
});
const catalogSchema = z.strictObject({
  packs: z
    .array(
      z.strictObject({
        component,
        version: z.string().min(1),
        platform: z.string().min(1),
        arch: z.string().min(1),
        sha256: checksum,
        manifest_sha256: checksum,
        url: z.string().url().optional(),
        archive: relativePath,
      }),
    )
    .max(64),
});

async function zipArchive(path: string) {
  return new Promise<ZipFile>((done, failed) =>
    openZip(
      path,
      { lazyEntries: true, autoClose: false, validateEntrySizes: true },
      (error, zip) =>
        error || !zip ? failed(error ?? new Error("Invalid ZIP")) : done(zip),
    ),
  );
}
async function nextEntry(zip: ZipFile) {
  return new Promise<Entry | undefined>((done, failed) => {
    const clean = () => {
      zip.off("entry", entry);
      zip.off("end", end);
      zip.off("error", error);
    };
    const entry = (value: Entry) => {
      clean();
      done(value);
    };
    const end = () => {
      clean();
      done(undefined);
    };
    const error = (value: Error) => {
      clean();
      failed(value);
    };
    zip.once("entry", entry).once("end", end).once("error", error);
    zip.readEntry();
  });
}

async function hashFile(path: string) {
  const hash = createHash("sha256");
  for await (const chunk of createReadStream(path))
    hash.update(chunk as Buffer);
  return hash.digest("hex");
}
async function verifyAt(
  root: string,
  name: z.infer<typeof component>,
  manifestHash: string,
) {
  let actualManifestHash: string;
  try {
    actualManifestHash = await hashFile(join(root, "manifest.json"));
  } catch (error) {
    if (error instanceof Error && "code" in error && error.code === "ENOENT")
      throw new Error(
        `UNAVAILABLE: ${name} runtime pack missing; run nous runtime install ${name}`,
        { cause: error },
      );
    throw error;
  }
  if (actualManifestHash !== manifestHash)
    throw new Error("Runtime manifest disagrees with the application catalog");
  const pack = packSchema.parse(
    JSON.parse(await readFile(join(root, "manifest.json"), "utf8")),
  );
  if (
    pack.component !== name ||
    pack.platform !== process.platform ||
    pack.arch !== process.arch
  )
    throw new Error("Runtime pack platform/component mismatch");
  if (
    new Set(pack.files.map((file) => file.path.toLowerCase())).size !==
    pack.files.length
  )
    throw new Error("Duplicate runtime inventory path");
  for (const file of pack.files) {
    const path = resolve(root, file.path);
    if (
      !path.startsWith(resolve(root) + sep) ||
      !(await stat(path)).isFile() ||
      (await hashFile(path)) !== file.sha256
    )
      throw new Error("Runtime pack inventory verification failed");
  }
  for (const executable of pack.required_executables)
    if (!pack.files.some((file) => file.path === executable))
      throw new Error("Runtime executable is not in verified inventory");
  return pack;
}

export async function verifyRuntime(locations: RuntimeLocations, name: string) {
  const selected = component.parse(name);
  const catalog = catalogSchema.parse(
    JSON.parse(
      await readFile(
        join(locations.program, "manifest", "runtimes.json"),
        "utf8",
      ),
    ),
  );
  const entries = catalog.packs.filter(
    (pack) =>
      pack.component === selected &&
      pack.platform === process.platform &&
      pack.arch === process.arch,
  );
  if (entries.length !== 1)
    throw new Error("No unique runtime pack for this platform");
  return verifyAt(
    join(locations.runtime, selected),
    selected,
    entries[0]!.manifest_sha256,
  );
}
export async function listRuntimes(locations: RuntimeLocations) {
  return Promise.all(
    component.options.map(async (name) => {
      try {
        const pack = await verifyRuntime(locations, name);
        return { component: name, version: pack.version, status: "PASS" };
      } catch {
        return {
          component: name,
          status: "BLOCKED",
          reason: "Runtime pack missing or invalid",
        };
      }
    }),
  );
}

/** Explicit installer only. Normal serve never invokes this function. */
export async function installRuntime(
  locations: RuntimeLocations,
  name: string,
  localArchive?: string,
  signal?: AbortSignal,
  downloadTimeoutMs = hostSchema.parse(undefined).runtime_download_timeout_ms,
) {
  const selected = component.parse(name);
  const catalog = catalogSchema.parse(
    JSON.parse(
      await readFile(
        join(locations.program, "manifest", "runtimes.json"),
        "utf8",
      ),
    ),
  );
  const entries = catalog.packs.filter(
    (pack) =>
      pack.component === selected &&
      pack.platform === process.platform &&
      pack.arch === process.arch,
  );
  if (entries.length !== 1)
    throw new Error("No unique runtime pack for this platform");
  const entry = entries[0]!;
  const destination = join(locations.runtime, selected);
  if (await stat(destination).catch(() => undefined)) {
    const existing = await verifyAt(
      destination,
      selected,
      entry.manifest_sha256,
    );
    if (existing.version !== entry.version)
      throw new Error(
        "Runtime version change requires explicit offline migration",
      );
    return {
      component: selected,
      version: existing.version,
      status: "PASS",
      reused: true,
    };
  }
  await mkdir(locations.runtime, { recursive: true });
  const stage = resolve(
    locations.runtime,
    `.install-${selected}-${randomUUID()}`,
  );
  if (dirname(stage) !== resolve(locations.runtime))
    throw new Error("Unsafe runtime staging target");
  await mkdir(stage);
  try {
    let archive = localArchive
      ? resolve(localArchive)
      : join(locations.program, "packs", entry.archive);
    if (!localArchive && !(await stat(archive).catch(() => undefined))) {
      if (!entry.url)
        throw new Error(
          "Runtime pack is not local and has no approved download URL",
        );
      const url = new URL(entry.url);
      if (
        url.protocol !== "https:" ||
        url.username ||
        url.password ||
        url.search ||
        url.hash
      )
        throw new Error("Unsafe runtime download URL");
      const timeout = AbortSignal.timeout(downloadTimeoutMs);
      const response = await fetch(url, {
        signal: signal ? AbortSignal.any([signal, timeout]) : timeout,
      });
      if (!response.ok || !response.body)
        throw new Error("Runtime pack download failed");
      archive = join(stage, "download.zip");
      let length = 0;
      await pipeline(
        Readable.fromWeb(
          response.body as import("node:stream/web").ReadableStream<Uint8Array>,
        ),
        new Transform({
          transform(chunk: Buffer, _encoding, callback) {
            length += chunk.length;
            callback(
              length > MAX_RUNTIME_ARCHIVE_BYTES
                ? new Error("Runtime archive exceeds bound")
                : null,
              chunk,
            );
          },
        }),
        createWriteStream(archive, { flags: "wx" }),
      );
    }
    if (
      (await stat(archive)).size > MAX_RUNTIME_ARCHIVE_BYTES ||
      (await hashFile(archive)) !== entry.sha256
    )
      throw new Error("Runtime archive checksum/bound failed");
    const zip = await zipArchive(archive);
    if (zip.entryCount > MAX_RUNTIME_ARCHIVE_ENTRIES) {
      zip.close();
      throw new Error("Runtime archive entry bound exceeded");
    }
    const content = join(stage, "content");
    await mkdir(content);
    let total = 0;
    const names = new Set<string>();
    try {
      for (let file = await nextEntry(zip); file; file = await nextEntry(zip)) {
        if (((file.externalFileAttributes >>> 16) & 0o170000) === 0o120000)
          throw new Error("Runtime archive symlink rejected");
        if (file.fileName.endsWith("/")) continue;
        const safe = relativePath.parse(file.fileName);
        if (names.has(safe.toLowerCase()))
          throw new Error("Invalid/duplicate runtime archive entry");
        names.add(safe.toLowerCase());
        total += file.uncompressedSize;
        if (
          file.uncompressedSize > MAX_RUNTIME_ARCHIVE_BYTES ||
          total > MAX_RUNTIME_EXTRACTED_BYTES
        )
          throw new Error("Runtime extraction exceeds bound");
        const target = resolve(content, safe);
        if (!target.startsWith(content + sep))
          throw new Error("Runtime entry escapes staging");
        await mkdir(dirname(target), { recursive: true });
        let bytes = 0;
        const stream = await new Promise<import("node:stream").Readable>(
          (done, failed) =>
            zip.openReadStream(file!, (error, input) =>
              error || !input
                ? failed(error ?? new Error("Invalid ZIP stream"))
                : done(input),
            ),
        );
        await pipeline(
          stream,
          new Transform({
            transform(chunk: Buffer, _encoding, callback) {
              bytes += chunk.length;
              callback(
                bytes > file.uncompressedSize
                  ? new Error("Runtime entry exceeds declared size")
                  : null,
                chunk,
              );
            },
          }),
          createWriteStream(target, { flags: "wx" }),
        );
        if (bytes !== file.uncompressedSize)
          throw new Error("Runtime entry size mismatch");
      }
    } finally {
      zip.close();
    }
    const verified = await verifyAt(content, selected, entry.manifest_sha256);
    if (verified.version !== entry.version)
      throw new Error("Runtime pack version mismatch");
    if (names.size !== verified.files.length + 1 || !names.has("manifest.json"))
      throw new Error("Runtime archive has unlisted payload");
    await rename(content, destination);
    return {
      component: selected,
      version: verified.version,
      status: "PASS",
      reused: false,
    };
  } finally {
    await rm(stage, { recursive: true, force: true });
  }
}
