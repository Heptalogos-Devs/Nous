// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import assert from "node:assert/strict";
import { createServer } from "node:http";
import { readFile, writeFile, rm } from "node:fs/promises";
import { join } from "node:path";
import { randomUUID } from "node:crypto";
import { parse, stringify } from "smol-toml";
import { workspaceTemp } from "../workspace.js";
import { boot, prepare, stop, type Boot } from "./support.js";
import { resolveLocations } from "../../apps/nous-core/src/locations.js";

// Controlled HTTP outputs test route admission and persisted access, not media quality.
const calls: string[] = [];
let holdFallback = false;
const provider = createServer((request, response) => {
  void (async () => {
    const chunks: Uint8Array[] = [];
    for await (const chunk of request) chunks.push(chunk as Uint8Array);
    const body = JSON.parse(Buffer.concat(chunks).toString()) as {
      model: string;
      messages: unknown;
    };
    calls.push(body.model);
    assert(JSON.stringify(body.messages).includes("video_url"));
    response.setHeader("content-type", "application/json");
    if (body.model === "preferred") {
      response.writeHead(503).end('{"error":"controlled route refusal"}');
      return;
    }
    if (holdFallback) return; // The work clock must interrupt this transmitted attempt.
    response.end(
      JSON.stringify({
        choices: [
          {
            finish_reason: "stop",
            message: {
              content: JSON.stringify({
                summary: {
                  content: "The supplied audio reports a test event.",
                  basis_keys: ["S000"],
                },
                coverage: {
                  visual: "not_available",
                  audio: "observed",
                  embedded_text: "not_available",
                  source_text: "not_available",
                },
                observations: [
                  {
                    kind: "speech",
                    content: "A test event is reported.",
                    evidence_channel: "audio",
                    basis: "direct",
                    certainty: "clear",
                    start_ms: null,
                    end_ms: null,
                    basis_keys: ["S000"],
                  },
                ],
                mentions: [],
                embedded_text: [],
                source_text: [],
                speech: [],
                interpretations: [],
                uncertainties: [],
              }),
            },
          },
        ],
        usage: { prompt_tokens: 1, completion_tokens: 1, total_tokens: 2 },
      }),
    );
  })().catch(() => response.destroy());
});
await new Promise<void>((resolve) => provider.listen(0, "127.0.0.1", resolve));
const address = provider.address();
if (!address || typeof address === "string")
  throw new Error("Missing provider port");
const root = await workspaceTemp("smoke", "media-routes-");
let current: Boot | undefined;
let verified = false;
try {
  const locator = await prepare(root);
  const locations = await resolveLocations({
    locator,
    installationHome: process.cwd(),
  });
  const configPath = join(locations.config, "nous.toml");
  const base = parse(await readFile(configPath, "utf8"));
  const videoPath = join(root, "opaque-video-input.bin");
  await writeFile(videoPath, Uint8Array.of(1, 2, 3));
  process.env.NOUS_MEDIA_SMOKE = "controlled-local-provider";
  let subjectId = "",
    sourceRegionId = "";
  for (const preferredAudio of [true, false]) {
    await stop(current);
    current = undefined;
    const profile = (model: string, audio: boolean) => ({
      gateway: "controlled",
      protocol: "openai-chat",
      model,
      capabilities: [
        "text",
        "structured_output",
        "video_input",
        ...(audio ? ["audio_input"] : []),
      ],
    });
    await writeFile(
      configPath,
      stringify({
        ...base,
        maintenance: { enabled: false },
        models: {
          gateway_profiles: {
            controlled: {
              base_url: `http://127.0.0.1:${address.port}/v1`,
              credential_env: "NOUS_MEDIA_SMOKE",
            },
          },
          model_profiles: {
            preferred: profile("preferred", preferredAudio),
            fallback: profile("fallback", !preferredAudio),
          },
          execution_profiles: {
            preferred: { model: "preferred" },
            fallback: { model: "fallback" },
          },
          roles: {
            material_direct_structuring: { routes: ["preferred", "fallback"] },
          },
        },
      }),
    );
    current = await boot(locator);
    const client = current.client;
    if (!subjectId) {
      subjectId = (
        await client.subjects.create({
          operationId: randomUUID(),
          cognitiveSeed: {
            text: "schema_version = 1",
            format: "application/vnd.nous-wave.cognitive-seed+toml;version=1",
          },
        })
      ).subjectId;
      const artifact = await client.artifacts.uploadFile(subjectId, videoPath, {
        mediaType: "video/mp4",
      });
      const observed = await client.cognition.observe({
        subjectId,
        requestId: randomUUID(),
        sourceClass: "file",
        admit: true,
        material: { case: "artifactId", value: artifact.artifactId },
      });
      sourceRegionId = observed.sourceRegionId!;
    }
    const start = calls.length;
    const result = await client.model.deriveMaterial({
      subjectId,
      sourceRegionId,
      strategy: "direct_structured",
    });
    assert.deepEqual(calls.slice(start), ["preferred", "fallback"]);
    if (preferredAudio) {
      assert.equal(result.representations.length, 0);
      assert.match(
        result.degradation[0]?.detail ?? "",
        /output_semantics_invalid/,
      );
    } else {
      assert.equal(result.degradation.length, 0);
      const representationId = result.selectedRepresentationId!;
      const persisted = await client.material.representation({
        subjectId,
        id: representationId,
      });
      assert.deepEqual(persisted.quality?.input_access, {
        visual: "original",
        audio: "original",
        source_text: "unavailable",
      });
      assert.deepEqual(
        persisted.structuredPayload?.evidence_access,
        persisted.quality?.input_access,
      );
      const beforeReplay = calls.length;
      const replay = await client.model.deriveMaterial({
        subjectId,
        sourceRegionId,
        strategy: "direct_structured",
      });
      assert.equal(replay.selectedRepresentationId, representationId);
      assert.equal(calls.length, beforeReplay);
    }
  }
  await stop(current);
  current = undefined;
  const deadlineConfiguration = parse(await readFile(configPath, "utf8"));
  deadlineConfiguration.core_execution = {
    opportunity: {
      work_timeout_ms: 1000,
      cleanup_timeout_ms: 1000,
      acknowledgement_timeout_ms: 1000,
      response_margin_ms: 1000,
    },
  };
  await writeFile(configPath, stringify(deadlineConfiguration));
  current = await boot(locator);
  await writeFile(videoPath, Uint8Array.of(4, 5, 6, 7));
  const timedArtifact = await current.client.artifacts.uploadFile(
    subjectId,
    videoPath,
    { mediaType: "video/mp4" },
  );
  const timedSource = await current.client.cognition.observe({
    subjectId,
    requestId: randomUUID(),
    sourceClass: "file",
    admit: true,
    material: { case: "artifactId", value: timedArtifact.artifactId },
  });
  const timedInput = {
    subjectId,
    sourceRegionId: timedSource.sourceRegionId!,
    strategy: "direct_structured",
  };
  const beforeCancel = calls.length;
  holdFallback = true;
  await assert.rejects(
    current.client.model.deriveMaterial(timedInput),
    (error: unknown) =>
      Boolean(
        error &&
        typeof error === "object" &&
        "code" in error &&
        error.code === 4,
      ),
  );
  assert.deepEqual(calls.slice(beforeCancel), ["preferred", "fallback"]);
  holdFallback = false;
  const resumed = await current.client.model.deriveMaterial(timedInput);
  assert.equal(resumed.degradation.length, 0);
  assert(resumed.selectedRepresentationId);
  assert.deepEqual(calls.slice(beforeCancel), [
    "preferred",
    "fallback",
    "preferred",
    "fallback",
  ]);
  console.log(
    "MEDIA_ROUTES public_core=true persisted_access=true opposite_capabilities=true successful_replay_no_provider=true work_deadline_cleanup_resume=true",
  );
  verified = true;
} finally {
  await stop(current);
  delete process.env.NOUS_MEDIA_SMOKE;
  provider.closeAllConnections();
  await new Promise<void>((resolve) => provider.close(() => resolve()));
  if (verified) await rm(root, { recursive: true, force: true });
  else console.error(`Media route diagnostics retained: ${root}`);
}
