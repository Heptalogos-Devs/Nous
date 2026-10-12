// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { randomBytes } from "node:crypto";
import { open } from "node:fs/promises";
import { basename } from "node:path";
import { Readable } from "node:stream";
import type { RequestOptions } from "./index.js";

export interface ArtifactUploadOptions extends RequestOptions {
  mediaType: string;
  filename?: string;
}
export interface UploadedArtifact {
  artifactId: string;
  subjectId: string;
  contentHash: string;
  byteLength: bigint;
  mediaType: string;
}
export function artifactUploads(
  baseUrl: string,
  token: string,
  limits: (options: RequestOptions) => Promise<{ maxUploadBytes: bigint }>,
) {
  function endpoint() {
    const url = new URL(baseUrl);
    if (
      url.username ||
      url.password ||
      url.search ||
      url.hash ||
      (url.protocol !== "https:" &&
        !(
          url.protocol === "http:" &&
          ["127.0.0.1", "[::1]"].includes(url.hostname)
        ))
    )
      throw new Error(
        "Artifact endpoint must be credential-free HTTPS or literal loopback HTTP",
      );
    return url;
  }
  async function upload(
    subjectId: string,
    source: AsyncIterable<Uint8Array>,
    length: number,
    options: ArtifactUploadOptions,
  ): Promise<UploadedArtifact> {
    if (!Number.isSafeInteger(length) || length < 0)
      throw new Error("Artifact byte length is invalid");
    if (!/^[a-zA-Z0-9.+-]+\/[a-zA-Z0-9.+-]+$/.test(options.mediaType))
      throw new Error("Invalid artifact media type");
    const url = endpoint();
    const { maxUploadBytes } = await limits(options);
    if (maxUploadBytes < 1n || BigInt(length) > maxUploadBytes)
      throw new Error(
        `Artifact upload exceeds instance limit (${maxUploadBytes} bytes)`,
      );
    url.pathname = `${url.pathname.replace(/\/$/, "")}/artifacts/${encodeURIComponent(subjectId)}`;
    const boundary = `nous-${randomBytes(24).toString("hex")}`;
    const filename = basename(options.filename ?? "artifact")
      .replace(/[^a-zA-Z0-9._-]/g, "_")
      .slice(0, 256);
    const prefix = Buffer.from(
      `--${boundary}\r\nContent-Disposition: form-data; name="file"; filename="${filename}"\r\nContent-Type: ${options.mediaType}\r\n\r\n`,
    );
    const suffix = Buffer.from(`\r\n--${boundary}--\r\n`);
    const stream = Readable.from(
      (async function* () {
        yield prefix;
        let sent = 0;
        for await (const bytes of source) {
          sent += bytes.byteLength;
          if (sent > length) throw new Error("Artifact changed during upload");
          yield bytes;
        }
        if (sent !== length) throw new Error("Artifact changed during upload");
        yield suffix;
      })(),
    );
    const timeout = AbortSignal.timeout(options.timeoutMs ?? 300_000);
    const signal = options.signal
      ? AbortSignal.any([timeout, options.signal])
      : timeout;
    try {
      const request: RequestInit & { duplex: "half" } = {
        method: "POST",
        redirect: "error",
        signal,
        headers: {
          authorization: `Bearer ${token}`,
          "content-type": `multipart/form-data; boundary=${boundary}`,
          "content-length": String(prefix.length + length + suffix.length),
        },
        body: Readable.toWeb(stream) as ReadableStream<Uint8Array>,
        duplex: "half",
      };
      const response = await fetch(url, request);
      if (!response.ok || !response.body) {
        await response.body?.cancel();
        throw new Error(`Artifact upload failed (HTTP ${response.status})`);
      }
      const reader = response.body.getReader();
      const chunks: Uint8Array[] = [];
      let size = 0;
      try {
        while (true) {
          const item = await reader.read();
          if (item.done) break;
          size += item.value.byteLength;
          if (size > 65_536) {
            await reader.cancel();
            throw new Error("Artifact response exceeds bounds");
          }
          chunks.push(item.value);
        }
      } finally {
        reader.releaseLock();
      }
      const value: unknown = JSON.parse(Buffer.concat(chunks).toString("utf8"));
      if (!value || typeof value !== "object")
        throw new Error("Invalid artifact response");
      const record = value as Record<string, unknown>;
      if (
        typeof record.artifactId !== "string" ||
        typeof record.subjectId !== "string" ||
        typeof record.contentHash !== "string" ||
        typeof record.mediaType !== "string" ||
        typeof record.byteLength !== "string" ||
        !/^\d+$/.test(record.byteLength) ||
        record.subjectId !== subjectId ||
        BigInt(record.byteLength) !== BigInt(length)
      )
        throw new Error("Invalid artifact response");
      return {
        artifactId: record.artifactId,
        subjectId: record.subjectId,
        contentHash: record.contentHash,
        mediaType: record.mediaType,
        byteLength: BigInt(record.byteLength),
      };
    } finally {
      stream.destroy();
    }
  }
  return {
    async download(
      subjectId: string,
      artifactId: string,
      options: RequestOptions = {},
    ) {
      const url = endpoint();
      url.pathname = `${url.pathname.replace(/\/$/, "")}/artifacts/${encodeURIComponent(subjectId)}/${encodeURIComponent(artifactId)}`;
      const timeout = AbortSignal.timeout(options.timeoutMs ?? 300_000);
      const signal = options.signal
        ? AbortSignal.any([timeout, options.signal])
        : timeout;
      const response = await fetch(url, {
        headers: { authorization: `Bearer ${token}` },
        redirect: "error",
        signal,
      });
      if (!response.ok || !response.body) {
        await response.body?.cancel();
        throw new Error(`Artifact download failed (HTTP ${response.status})`);
      }
      return {
        mediaType:
          response.headers.get("content-type") ?? "application/octet-stream",
        content: Readable.fromWeb(
          response.body as import("node:stream/web").ReadableStream<Uint8Array>,
        ),
      };
    },
    async uploadFile(
      subjectId: string,
      path: string,
      options: ArtifactUploadOptions,
    ) {
      const file = await open(path, "r");
      try {
        const stat = await file.stat();
        if (!stat.isFile())
          throw new Error("Artifact input is not a regular file");
        return await upload(
          subjectId,
          file.createReadStream({ autoClose: false, highWaterMark: 262_144 }),
          stat.size,
          { ...options, filename: options.filename ?? basename(path) },
        );
      } finally {
        await file.close();
      }
    },
    uploadBytes(
      subjectId: string,
      bytes: Uint8Array,
      options: ArtifactUploadOptions,
    ) {
      return upload(
        subjectId,
        (async function* () {
          for (let offset = 0; offset < bytes.length; offset += 262_144)
            yield bytes.subarray(offset, offset + 262_144);
        })(),
        bytes.byteLength,
        options,
      );
    },
  };
}
