// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { createServer } from "node:http";
import { expect, test } from "vitest";
import { modelConfigurationSchema } from "../../src/model/configuration.js";
import { ModelRuntime } from "../../src/model/runtime.js";

test.each([true, false])(
  "binds direct-media interpretation to the actual successful route (preferred audio=%s)",
  async (preferredAudio) => {
    const received: string[] = [];
    const server = createServer((request, response) => {
      void (async () => {
        const chunks: Uint8Array[] = [];
        for await (const chunk of request) chunks.push(chunk as Uint8Array);
        const { model } = JSON.parse(Buffer.concat(chunks).toString()) as {
          model: string;
        };
        received.push(model);
        response.setHeader("content-type", "application/json");
        if (model === "preferred") {
          response.writeHead(503).end("{}");
          return;
        }
        response.end(
          JSON.stringify({
            choices: [
              {
                finish_reason: "stop",
                message: {
                  content: JSON.stringify({
                    summary: {
                      content: "A reported test event.",
                      basis_keys: ["S000"],
                    },
                    coverage: {
                      visual: "not_available",
                      audio: "observed",
                      embedded_text: "not_available",
                      source_text: "not_available",
                    },
                    observations: [],
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
          }),
        );
      })().catch(() => response.destroy());
    });
    await new Promise<void>((resolve) =>
      server.listen(0, "127.0.0.1", resolve),
    );
    const address = server.address();
    if (!address || typeof address === "string")
      throw new Error("Missing provider port");
    process.env.NOUS_MEDIA_INPUT = "controlled-local-provider";
    try {
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
      const models = await ModelRuntime.fromConfig(
        modelConfigurationSchema.parse({
          gateway_profiles: {
            controlled: {
              base_url: `http://127.0.0.1:${address.port}/v1`,
              credential_env: "NOUS_MEDIA_INPUT",
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
        }),
      );
      const interpreted = models.material.describeMedia(
        Uint8Array.of(1, 2, 3),
        "video/mp4",
        true,
        undefined,
        undefined,
        {
          access: {
            visual: "original",
            audio: "unavailable",
            source_text: "unavailable",
          },
          catalog: { S000: { kind: "source_region", value: "source" } },
        },
      );
      if (preferredAudio)
        await expect(interpreted).rejects.toThrow("output_semantics_invalid");
      else {
        const result = await interpreted;
        expect(result.inputAccess.audio).toBe("original");
        expect(result.producerMetadata.executionProfile).toBe("fallback");
        expect(
          "structuredPayload" in result && result.structuredPayload,
        ).toMatchObject({
          evidence_access: { audio: "original" },
        });
      }
      expect(received).toEqual(["preferred", "fallback"]);
    } finally {
      delete process.env.NOUS_MEDIA_INPUT;
      server.closeAllConnections();
      await new Promise<void>((resolve) => server.close(() => resolve()));
    }
  },
);
