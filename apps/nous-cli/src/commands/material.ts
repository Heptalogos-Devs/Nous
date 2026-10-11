// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { create } from "@bufbuild/protobuf";
import { EmptySchema } from "@bufbuild/protobuf/wkt";
import {
  ListRequestSchema,
  ObjectRequestSchema,
  PageSchema,
  RepresentationListRequestSchema,
} from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import { CliError, boundedInteger } from "../agent.js";
import type { CliEnvironment } from "../runtime.js";

export async function materialCommands(
  env: CliEnvironment,
  action?: string,
  argument?: string,
) {
  const { client, subjectId, values, required, resolveReference } = env;
  if (action === "limits") return client.material.limits(create(EmptySchema));
  if (action === "artifacts")
    return client.material.listArtifacts(
      create(ListRequestSchema, {
        subjectId,
        page: create(PageSchema, {
          pageSize: boundedInteger(values["page-size"], 1, 200, "--page-size"),
          pageToken: values["page-token"],
        }),
      }),
    );
  if (action === "representations") {
    const sourceRegion = values["source-region"]
      ? await resolveReference(values["source-region"], "source_region")
      : undefined;
    const artifact = values.artifact
      ? await resolveReference(values.artifact, "artifact")
      : undefined;
    return client.material.representations(
      create(RepresentationListRequestSchema, {
        subjectId,
        sourceRegionId: sourceRegion?.value,
        artifactId: artifact?.value,
        kind: values.kind,
        limit: boundedInteger(values.limit ?? "50", 1, 200, "--limit"),
      }),
    );
  }
  if (action === "producer") {
    const id = required(argument, "producer_signature_id UUID");
    if (!/^[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}$/i.test(id))
      throw new CliError(
        "INVALID_ARGUMENT",
        "producer_signature_id must be a UUID",
      );
    return client.material.producer(
      create(ObjectRequestSchema, { subjectId, id }),
    );
  }
  throw new CliError(
    "INVALID_ARGUMENT",
    "Use material limits|producer|artifacts|representations",
  );
}
