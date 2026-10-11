// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { expect, it } from "vitest";
import { createServer } from "node:http";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { connectNous } from "../src/node.js";
import { create, toBinary } from "@bufbuild/protobuf";
import { MaterialLimitsSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";

it("streams a file with exact multipart length and rejects header injection", async () => {
  const dir = await mkdtemp(join(tmpdir(), "nous-upload-"));
  const size = 1024 * 1024 + 7;
  const path = join(dir, "payload.bin");
  await writeFile(path, Buffer.alloc(size, 97));
  let seenBytes = 0;
  let seenLength = 0;
  let authorization = "";
  let advertisedLimit = size;
  const server = createServer((req, res) => {
    void (async () => {
      if (req.url?.endsWith("/GetLimits")) {
        for await (const _ of req) {
          /* Drain the Connect request. */
        }
        res.setHeader("Content-Type", "application/proto");
        res.end(
          toBinary(
            MaterialLimitsSchema,
            create(MaterialLimitsSchema, {
              maxUploadBytes: BigInt(advertisedLimit),
            }),
          ),
        );
        return;
      }
      seenLength = Number(req.headers["content-length"]);
      authorization = req.headers.authorization ?? "";
      for await (const chunk of req) {
        const data: unknown = chunk;
        if (!(data instanceof Uint8Array))
          throw new Error("Invalid upload chunk");
        seenBytes += data.byteLength;
      }
      res.setHeader("Content-Type", "application/json");
      res.end(
        JSON.stringify({
          artifactId: "artifact",
          subjectId: "subject",
          byteLength: String(size),
          contentHash: "a".repeat(64),
          mediaType: "application/octet-stream",
        }),
      );
    })().catch(() => {
      res.destroy();
    });
  });
  await new Promise<void>((done) => server.listen(0, "127.0.0.1", done));
  try {
    const address = server.address();
    if (!address || typeof address === "string")
      throw new Error("Missing upload fixture port");
    const client = connectNous(
      `http://127.0.0.1:${address.port}`,
      "fixture-token",
    );
    const result = await client.artifacts.uploadFile("subject", path, {
      mediaType: "application/octet-stream",
      filename: 'name"\r\nInjected: true',
    });
    expect(result.byteLength).toBe(BigInt(size));
    expect(seenBytes).toBe(seenLength);
    expect(seenBytes).toBeGreaterThan(size);
    expect(authorization).toBe("Bearer fixture-token");
    advertisedLimit = size - 1;
    await expect(
      client.artifacts.uploadFile("subject", path, {
        mediaType: "application/octet-stream",
      }),
    ).rejects.toThrow("instance limit");
    expect(seenBytes).toBe(seenLength);
    await expect(
      client.artifacts.uploadBytes("subject", new Uint8Array(), {
        mediaType: "text/plain\r\nInjected: true",
      }),
    ).rejects.toThrow("media type");
  } finally {
    await new Promise<void>((done) => server.close(() => done()));
    await rm(dir, { recursive: true, force: true });
  }
});
