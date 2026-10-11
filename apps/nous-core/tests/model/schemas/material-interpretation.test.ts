// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { describe, expect, it } from "vitest";
import { structuredMaterialResult } from "../../../src/model/schemas/material-interpretation.js";

const output = () => ({
  summary: {
    content: "A rocket launch with uncertain commentary.",
    basis_keys: ["S000"],
  },
  coverage: {
    visual: "observed",
    audio: "not_available",
    embedded_text: "not_available",
    source_text: "not_available",
  },
  observations: [
    {
      kind: "event",
      content: "Rocket launch",
      evidence_channel: "visual",
      basis: "direct",
      certainty: "uncertain",
      start_ms: 0,
      end_ms: 1000,
      basis_keys: ["S000"],
    },
  ],
  mentions: [
    {
      surface: "rocket",
      category: "object",
      role: null,
      basis_keys: ["S000"],
    },
  ],
  embedded_text: [],
  source_text: [],
  speech: [],
  interpretations: [
    {
      content: "May be a historical launch",
      status: "tentative",
      basis_keys: ["S000"],
    },
  ],
  uncertainties: [
    { issue: "No audio available", alternatives: [], basis_keys: ["S000"] },
  ],
});
const source = {
  access: {
    visual: "original",
    audio: "unavailable",
    source_text: "unavailable",
  } as const,
  catalog: { S000: { kind: "source_region", value: "source-id" } },
  durationMs: 1000,
};
describe("Material interpretation contract", () => {
  it("does not treat a Transcript as directly heard audio when original frames are also supplied", () => {
    const heard = output();
    heard.coverage.audio = "observed";
    heard.observations[0]!.evidence_channel = "audio";
    expect(() =>
      structuredMaterialResult(heard, {
        ...source,
        access: {
          visual: "original",
          audio: "representation",
          source_text: "unavailable",
        },
      }),
    ).toThrow("representation");
  });
  it("does not promote a committed media description into directly observed pixels", () => {
    const context = {
      ...source,
      access: { ...source.access, visual: "representation" as const },
    };
    expect(() => structuredMaterialResult(output(), context)).toThrow(
      "representation",
    );
    const reported = output();
    reported.coverage.visual = "reported";
    reported.observations[0]!.basis = "reported";
    const result = structuredMaterialResult(reported, context);
    expect(result.structuredPayload.evidence_access).toMatchObject({
      visual: "representation",
    });
    expect(result.text).toContain("visual=representation");
    expect(result.text).toContain("[reported/uncertain/event");
    reported.observations[0]!.basis = "direct";
    expect(() => structuredMaterialResult(reported, context)).toThrow(
      "representation",
    );
  });

  it("rejects unsupported direct facts, forged keys and source-time overflow", () => {
    const summary = output();
    summary.summary.basis_keys = [];
    expect(() => structuredMaterialResult(summary, source)).toThrow(
      "Summary requires",
    );
    summary.summary.basis_keys = ["D999"];
    expect(() => structuredMaterialResult(summary, source)).toThrow("Unknown");
    const forged = output();
    forged.observations[0]!.basis_keys = ["D999"];
    expect(() => structuredMaterialResult(forged, source)).toThrow("Unknown");
    forged.observations[0]!.basis_keys = [];
    expect(() => structuredMaterialResult(forged, source)).toThrow(
      "requires source support",
    );
    const late = output();
    late.observations[0]!.end_ms = 1001;
    expect(() => structuredMaterialResult(late, source)).toThrow("timestamp");
  });
  it("does not turn absent modalities into model observations", () => {
    expect(() =>
      structuredMaterialResult(output(), {
        ...source,
        access: { ...source.access, visual: "unavailable", audio: "original" },
      }),
    ).toThrow("visual");
    const invented = output();
    invented.coverage.audio = "observed";
    expect(() => structuredMaterialResult(invented, source)).toThrow("audio");
    const unavailable = output();
    unavailable.coverage.visual = "not_available";
    unavailable.observations[0]!.content = "No visual input was provided";
    expect(() => structuredMaterialResult(unavailable, source)).toThrow(
      "unavailable visual evidence",
    );
  });
  it("projects repeatably without promoting uncertainty or tentative interpretations", () => {
    const { text, structuredPayload } = structuredMaterialResult(
      output(),
      source,
    );
    expect(structuredMaterialResult(output(), source).text).toBe(text);
    expect(structuredPayload.observations).toMatchObject([
      { basis_refs: [{ kind: "source_region", value: "source-id" }] },
    ]);
    expect(JSON.stringify(structuredPayload)).not.toContain("basis_keys");
    expect(structuredPayload.summary).toMatchObject({
      basis_refs: [{ kind: "source_region", value: "source-id" }],
    });
    expect(text).toContain("[direct/uncertain/event; evidence=visual]");
    expect(text).toContain("[tentative] May be a historical launch");
    expect(text).toContain("Uncertainties:\n- No audio available");
  });
  it("expresses original source text without inventing embedded visual text", () => {
    const value = {
      ...output(),
      coverage: {
        visual: "not_available",
        audio: "not_available",
        embedded_text: "not_available",
        source_text: "observed",
      },
      observations: [],
      source_text: [
        {
          text: "Original passage",
          fidelity: "verbatim",
          start_ms: null,
          end_ms: null,
          basis_keys: ["S000"],
        },
      ],
    };
    const context = {
      ...source,
      access: {
        ...source.access,
        visual: "unavailable" as const,
        source_text: "original" as const,
      },
    };
    expect(structuredMaterialResult(value, context).text).toContain(
      "Source text:\n- [verbatim] Original passage",
    );
    expect(() =>
      structuredMaterialResult(value, {
        ...context,
        access: { ...context.access, source_text: "unavailable" },
      }),
    ).toThrow("source text");
  });
  it("accepts narrated states independently of visual observation and rejects absent evidence channels", () => {
    const value = output();
    value.coverage.visual = "not_available";
    value.coverage.audio = "observed";
    value.observations[0] = {
      ...value.observations[0]!,
      kind: "state",
      evidence_channel: "audio",
      content:
        "The narrator says Anil served as SpaceX's first flight surgeon.",
    };
    const context = {
      ...source,
      access: {
        ...source.access,
        visual: "unavailable" as const,
        audio: "original" as const,
      },
    };
    expect(structuredMaterialResult(value, context).text).toContain(
      "state; evidence=audio",
    );
    value.observations[0]!.evidence_channel = "visual";
    expect(() => structuredMaterialResult(value, context)).toThrow(
      "unavailable visual evidence",
    );
  });
});
