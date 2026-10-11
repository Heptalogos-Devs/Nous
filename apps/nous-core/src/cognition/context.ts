// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { createHash, randomUUID } from "node:crypto";
import { Code, ConnectError } from "@connectrpc/connect";
import { create, type Message } from "@bufbuild/protobuf";
import {
  domainError,
  ErrorCode,
  ErrorOwner,
  ErrorRecovery,
} from "@nous-wave/client/errors";
import {
  ContextCursorSchema,
  type ContextCursor as Cursor,
  type Projection,
  type ManagedContextResponse,
} from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";

type ContextPatch = Omit<
  ManagedContextResponse,
  keyof Message | "cursor" | "projection"
> & {
  cursor: Cursor;
  projection: Projection;
};

interface Track {
  subjectId: string;
  cursor: Cursor;
  segmentDigests: string[];
  profile: string;
}
const digest = (value: unknown) =>
  createHash("sha256").update(JSON.stringify(value)).digest("hex");

export class ContextCompiler {
  private readonly tracks = new Map<string, Track>();
  private readonly subjectGenerations = new Map<string, number>();
  constructor(private readonly maxTracks: number) {}

  generation(subjectId: string) {
    return this.subjectGenerations.get(subjectId) ?? 0;
  }

  invalidateSubject(subjectId: string) {
    const generation = this.generation(subjectId) + 1;
    this.subjectGenerations.set(subjectId, generation);
    for (const [key, track] of this.tracks) {
      if (track.subjectId === subjectId) this.tracks.delete(key);
    }
    return generation;
  }

  isCurrent(subjectId: string, generation: number) {
    return this.generation(subjectId) === generation;
  }

  compile(
    key: {
      subjectId: string;
      sessionId: string;
      workContextId?: string;
      consumerId: string;
    },
    profile: string,
    projection: Projection,
    known?: Cursor,
    expectedGeneration = this.generation(key.subjectId),
  ): ContextPatch {
    if (!this.isCurrent(key.subjectId, expectedGeneration))
      throw domainError(
        "Context projection crossed a Subject purge",
        Code.Aborted,
        {
          code: ErrorCode.STALE_CONTEXT,
          owner: ErrorOwner.RUNTIME,
          recovery: ErrorRecovery.REFRESH_STATE,
          context: { subject_id: key.subjectId },
        },
      );
    const trackId = digest([
      key.subjectId,
      key.sessionId,
      key.workContextId ?? "",
      key.consumerId,
    ]);
    const previous = this.tracks.get(trackId);
    const synchronized =
      previous &&
      known?.trackId === trackId &&
      known.epochId === previous.cursor.epochId &&
      known.revision === previous.cursor.revision &&
      previous.profile === profile;
    const segmentDigests = projection.segments.map(digest);
    const appendable =
      synchronized &&
      previous.segmentDigests.length <= segmentDigests.length &&
      previous.segmentDigests.every(
        (value, index) => value === segmentDigests[index],
      );
    if (!appendable) {
      const cursor = create(ContextCursorSchema, {
        trackId,
        epochId: randomUUID(),
        revision: 1,
      });
      this.save(trackId, {
        subjectId: key.subjectId,
        cursor,
        segmentDigests,
        profile,
      });
      return { kind: "RESET", cursor, projection };
    }
    const delta = projection.segments.slice(previous.segmentDigests.length);
    if (previous.cursor.revision === 0xffffffff)
      throw new ConnectError(
        "Context revision exhausted",
        Code.ResourceExhausted,
      );
    const cursor = {
      ...previous.cursor,
      revision: previous.cursor.revision + (delta.length > 0 ? 1 : 0),
    };
    this.save(trackId, {
      subjectId: key.subjectId,
      cursor,
      segmentDigests,
      profile,
    });
    return {
      kind: "APPEND",
      cursor,
      projection: { ...projection, segments: delta },
    };
  }
  private save(key: string, track: Track) {
    this.tracks.delete(key);
    this.tracks.set(key, track);
    if (this.tracks.size > this.maxTracks)
      this.tracks.delete(this.tracks.keys().next().value!);
  }
}
