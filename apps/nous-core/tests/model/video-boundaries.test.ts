// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import type { MessageInitShape } from "@bufbuild/protobuf";
import { ReserveWorkflowRequestSchema } from "@nous-wave/protocol/nous/wave/kernel/v1alpha1/model_pb.js";
import { coreExecutionSchema } from "../../src/configuration/catalog.js";
import { expect, it, vi } from "vitest";
import { dirname, join } from "node:path";
import { mkdtemp, rm, stat } from "node:fs/promises";
import { tmpdir } from "node:os";
const observed = vi.hoisted(() => ({
  commands: [] as string[][],
  roots: [] as string[],
  tokenInherited: false,
  failFrames: false,
}));
vi.mock("node:child_process", async (original) => {
  const actual = await original<typeof import("node:child_process")>();
  const fs = await import("node:fs/promises");
  return {
    ...actual,
    execFile: (
      _exe: string,
      args: string[],
      options: { env?: NodeJS.ProcessEnv },
      callback: (error: Error | null, stdout: string, stderr: string) => void,
    ) => {
      observed.commands.push(args);
      observed.tokenInherited ||= Object.hasOwn(
        options.env ?? {},
        "NOUS_MEDIA_TEST_TOKEN",
      );
      if (args[0] === "-version") {
        callback(null, "ffmpeg version unit-process-fixture\n", "");
        return;
      }
      const input = args[args.indexOf("-i") + 1]!;
      observed.roots.push(input);
      if (args.at(-1) === "-") {
        // The shipped bounded codec set has no wrapped_avframe null-output encoder.
        if (args[args.indexOf("-c:v") + 1] !== "copy") {
          callback(
            new Error("Encoder not found"),
            "",
            "Default null encoder unavailable",
          );
          return;
        }
        callback(null, "", "Duration: 00:00:02.00\nStream #0:0 Video:");
        return;
      }
      if (observed.failFrames) {
        callback(new Error("fixture failure"), "", "ignored private error");
        return;
      }
      void fs
        .writeFile(args.at(-1)!, new Uint8Array([255, 216, 255, 217]))
        .then(
          () => callback(null, "", "showinfo pts_time:0"),
          () => callback(new Error("fixture write failure"), "", ""),
        );
    },
  };
});
import { sampleVideo } from "../../src/model/video.js";
import { modelConfigurationSchema } from "../../src/model/configuration.js";
import { deriveMaterial } from "../../src/model/derivation.js";
import { ModelRuntime } from "../../src/model/runtime.js";
import { type ModelInvocations } from "../../src/model/invocations.js";
import type { KernelClient } from "../../src/kernel-client.js";
import { create } from "@bufbuild/protobuf";
import { DeriveMaterialRequestSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/model_pb.js";
it("bounds samples and subprocess input, excludes gateway credentials and removes temporary media on success and failure", async () => {
  const policy = modelConfigurationSchema.parse({
    video: {
      ffmpeg_executable: "fixture",
      max_frames: 3,
      max_video_seconds: 1,
    },
  }).video;
  process.env.NOUS_MEDIA_TEST_TOKEN = "fixture-secret";
  const instance = await mkdtemp(join(tmpdir(), "nous-video-instance-"));
  const freshTempRoot = join(instance, "separate-temp-volume");
  try {
    const result = await sampleVideo(
      new Uint8Array([1, 2, 3]),
      policy,
      freshTempRoot,
      ["nous_media_test_token"],
      false,
    );
    expect(result.frames).toHaveLength(3);
    expect(result.quality.sampled_timestamps).toEqual([0, 0.45, 0.9]);
    expect(result.quality.truncated).toBe(true);
    expect(observed.tokenInherited).toBe(false);
    expect(
      observed.commands.filter((args) => args.includes("-frames:v")),
    ).toHaveLength(3);
    for (const args of observed.commands.filter((entry) =>
      entry.includes("-i"),
    )) {
      expect(args[args.indexOf("-protocol_whitelist") + 1]).toBe("file,pipe");
      expect(args).toContain("-nostdin");
    }
    for (const input of observed.roots)
      await expect(stat(dirname(input))).rejects.toMatchObject({
        code: "ENOENT",
      });
    observed.failFrames = true;
    await expect(
      sampleVideo(new Uint8Array([1]), policy, tmpdir(), [], false),
    ).rejects.toThrow("FFmpeg frame execution");
    for (const input of observed.roots)
      await expect(stat(dirname(input))).rejects.toMatchObject({
        code: "ENOENT",
      });
    await expect(
      sampleVideo(
        new Uint8Array(policy.max_source_bytes + 1),
        policy,
        tmpdir(),
        [],
        false,
      ),
    ).rejects.toThrow("byte bound");
  } finally {
    delete process.env.NOUS_MEDIA_TEST_TOKEN;
    await rm(instance, { recursive: true, force: true });
  }
});

it("frames without a transcript cannot acquire audio evidence in either structured strategy", async () => {
  observed.failFrames = false;
  const instance = await mkdtemp(join(tmpdir(), "nous-frame-evidence-"));
  const policy = modelConfigurationSchema.parse({
    video: {
      input_mode: "frames",
      ffmpeg_executable: "fixture",
      max_frames: 1,
    },
  });
  const committed: { kind: string }[] = [];
  const modelInputs: unknown[] = [];
  const kernel = {
    execution: coreExecutionSchema.parse(undefined),
    material: {
      getSourceRegion: async () => ({
        artifactId: "artifact",
        coordinateKind: "whole_artifact",
      }),
      getArtifact: async () => ({ byteLength: 3n, mediaType: "video/mp4" }),
    },
    artifacts: {
      downloadArtifact: async function* () {
        yield { content: new Uint8Array([1, 2, 3]) };
      },
    },
    modelWorkflow: {
      reserveWorkflow: async (
        input: MessageInitShape<typeof ReserveWorkflowRequestSchema>,
      ) => ({
        lease: { token: "lease" },
        snapshot: input.snapshot,
      }),
      saveWorkflow: async () => ({}),
      releaseWorkflow: async () => ({}),
    },
    materialWorkflow: {
      commitInterpretation: async (input: { kind: string; text: string }) => {
        committed.push(input);
        return {
          ...input,
          representationId: "00000000-0000-0000-0000-000000000001",
        };
      },
      segmentDescription: async () => ({
        segments: [
          {
            key: "D001",
            text: "Only frames are visible.",
            reference: { kind: "derived_region", value: "segment" },
          },
        ],
      }),
    },
  } as unknown as KernelClient;
  const invocations = {
    profile: (role: string) =>
      role === "speech_transcription"
        ? undefined
        : { capabilities: ["image_input"] },
    snapshotPrompt: async (role: string) => ({
      role,
      configDigest: "fixture-config",
    }),
    generate: async (role: string, request: { content: unknown }) => {
      const input = request.content;
      modelInputs.push(input);
      return {
        value:
          role === "material_description"
            ? "Only frames are visible."
            : {
                summary: {
                  content: "Invented speech",
                  basis_keys: [
                    role === "material_structuring" ? "D001" : "S000",
                  ],
                },
                coverage: {
                  visual: "observed",
                  audio: "observed",
                  source_text: "not_available",
                  embedded_text: "not_available",
                },
                observations: [],
                mentions: [],
                embedded_text: [],
                source_text: [],
                speech: [],
                interpretations: [],
                uncertainties: [],
              },
        producerMetadata: {
          protocol: "openai-chat",
          implementation: "fixture",
        },
        input: { visual: Array.isArray(input), audio: false, text: true },
      };
    },
  } as unknown as ModelInvocations;
  const models = new ModelRuntime(
    invocations,
    "description_only",
    policy.video,
    [],
    policy.audio,
    instance,
  );
  try {
    for (const strategy of ["direct_structured", "describe_then_structure"]) {
      const result = await deriveMaterial(
        kernel,
        models,
        create(DeriveMaterialRequestSchema, {
          subjectId: "subject",
          sourceRegionId: "source",
          strategy,
        }),
        {},
      );
      expect(result.degradation.length).toBeGreaterThan(0);
      expect(
        committed.some((item) => item.kind === "structured_interpretation"),
      ).toBe(false);
    }
    const frameInput = modelInputs.find(Array.isArray) as {
      type: string;
      text?: string;
    }[];
    expect(JSON.parse(frameInput[1]!.text!)).toMatchObject({
      frame_index: 0,
      timestamp_seconds: 0,
    });
    expect(frameInput[2]!.type).toBe("file");
    const structuredText = modelInputs.find(
      (input) => typeof input === "string",
    ) as string;
    expect(JSON.parse(structuredText) as unknown).toMatchObject({
      modalities: { audio: false },
    });
  } finally {
    await rm(instance, { recursive: true, force: true });
  }
});

it("rejects invalid derivation choices before reading or interpreting source", async () => {
  const kernel = {} as Parameters<typeof deriveMaterial>[0];
  const models = { materialStrategy: "description_only" } as Parameters<
    typeof deriveMaterial
  >[1];
  const request = { strategy: "frames" } as Parameters<
    typeof deriveMaterial
  >[2];
  const invalid = deriveMaterial(kernel, models, request, {});
  await expect(invalid).rejects.toMatchObject({ code: 3 });
  await expect(invalid).rejects.toThrow("config describe video");
  await expect(
    deriveMaterial(
      kernel,
      models,
      { ...request, strategy: "description_only", target: "structured" },
      {},
    ),
  ).rejects.toMatchObject({ code: 3 });
});
