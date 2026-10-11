// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { expect, it } from "vitest";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { createHash } from "node:crypto";
import {
  TraceBody,
  traceSecrets,
  writeGatewayTrace,
} from "../../research/gateway-trace.js";

it("captures transcription fields and binary identity without storing audio bytes", async () => {
  const root = await mkdtemp(join(tmpdir(), "nous-transcription-trace-"));
  try {
    const audio = new Uint8Array([0, 255, 42, 13, 10]);
    const form = new FormData();
    form.set("model", "speech-model");
    form.set("language", "en");
    form.set("api_key", "private-token");
    form.set("file", new Blob([audio], { type: "audio/wav" }), "sample.wav");
    const wire = new Request("http://localhost/transcriptions", {
      method: "POST",
      body: form,
    });
    const capture = new TraceBody(1024, wire.headers.get("content-type")!);
    capture.add(Buffer.from(await wire.arrayBuffer()));
    const info = await capture.save(root, "request", ["private-token"]);
    const saved = await readFile(join(root, info.file), "utf8");
    expect(JSON.parse(saved)).toEqual({
      model: "speech-model",
      language: "en",
      file: {
        filename: "sample.wav",
        media_type: "audio/wav",
        decoded_bytes: audio.length,
        sha256: createHash("sha256").update(audio).digest("hex"),
      },
    });
    expect(saved).not.toContain("private-token");
    const malformed = new TraceBody(
      1024,
      "multipart/form-data; boundary=wrong",
    );
    malformed.add(Buffer.from("binary private-token"));
    const broken = await malformed.save(root, "broken", ["private-token"]);
    expect(await readFile(join(root, broken.file), "utf8")).not.toContain(
      "private-token",
    );
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

it("preserves text/schema and media identity while stripping echoed credentials", async () => {
  const root = await mkdtemp(join(tmpdir(), "nous-trace-"));
  try {
    const request = new TraceBody(1024 * 1024);
    const response = new TraceBody(1024 * 1024);
    const source = Buffer.from(
      JSON.stringify({
        model: "real-model",
        messages: [
          { role: "system", content: "real prompt" },
          {
            role: "user",
            content: [
              {
                type: "input_audio",
                input_audio: {
                  data: Buffer.from("media bytes").toString("base64"),
                  format: "mp3",
                },
              },
            ],
          },
        ],
        response_format: { json_schema: { schema: { type: "object" } } },
      }),
    );
    request.add(source);
    response.add(
      Buffer.from(
        JSON.stringify({
          error: {
            message: "echo Bearer private-token",
            headers: {
              authorization: "Bearer private-token",
              cookie: "session=private-cookie",
            },
            credential: "private-provider-secret",
          },
          usage: { total_tokens: 3 },
        }),
      ),
    );
    await writeGatewayTrace(
      root,
      1,
      { endpoint: "/chat/completions", status: 400 },
      request,
      response,
      traceSecrets({
        authorization: "Bearer private-token",
        cookie: "session=private-cookie",
      }),
    );
    const saved = await readFile(join(root, "1/request.json"), "utf8");
    expect(saved).toContain("real prompt");
    expect(saved).toContain("real-model");
    expect(saved).toContain('"media_type": "audio/mp3"');
    expect(saved).toContain('"decoded_bytes": 11');
    expect(saved).not.toContain(Buffer.from("media bytes").toString("base64"));
    const reply = await readFile(join(root, "1/response.json"), "utf8");
    for (const secret of [
      "private-token",
      "private-cookie",
      "private-provider-secret",
      "authorization",
      "cookie",
    ])
      expect(reply).not.toContain(secret);
    const meta = JSON.parse(
      await readFile(join(root, "1/meta.json"), "utf8"),
    ) as { request: { sha256: string; bytes: number } };
    expect(meta.request.sha256).toBe(
      createHash("sha256").update(source).digest("hex"),
    );
    expect(meta.request.bytes).toBe(source.length);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

it("hashes bytes beyond the capture bound without storing a partial credential", async () => {
  const root = await mkdtemp(join(tmpdir(), "nous-truncated-trace-"));
  try {
    const capture = new TraceBody(12);
    const source = Buffer.from('{"authorization":"Bearer private-token"}');
    capture.add(source.subarray(0, 8));
    capture.add(source.subarray(8));
    const info = await capture.save(root, "response", []);
    expect(info.truncated).toBe(true);
    expect(info.sha256).toBe(createHash("sha256").update(source).digest("hex"));
    expect(await readFile(join(root, info.file), "utf8")).not.toContain(
      "authorization",
    );
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});
