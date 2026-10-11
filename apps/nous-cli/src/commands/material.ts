// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import { defineCommand } from "citty";
import { commonArgs, stringArgs } from "./command-args.js";
import type { CliLeaf } from "./command-types.js";

import { CliError, boundedInteger } from "../agent.js";
import type { CliEnvironment } from "../runtime.js";

const materialLimits = (env: CliEnvironment) => env.client.material.limits({});
const materialArtifacts = (env: CliEnvironment) => {
  const { client, subjectId, values } = env;
  return client.material.listArtifacts({
    subjectId,
    page: {
      pageSize: boundedInteger(values["page-size"], 1, 200, "--page-size"),
      pageToken: values["page-token"],
    },
  });
};
const materialRepresentations = async (env: CliEnvironment) => {
  const { client, subjectId, values, resolveReference } = env;
  const sourceRegion = values["source-region"]
    ? await resolveReference(values["source-region"], "source_region")
    : undefined;
  const artifact = values.artifact
    ? await resolveReference(values.artifact, "artifact")
    : undefined;
  return client.material.representations({
    subjectId,
    sourceRegionId: sourceRegion?.value,
    artifactId: artifact?.value,
    kind: values.kind,
    limit: boundedInteger(values.limit ?? "50", 1, 200, "--limit"),
  });
};
const materialProducer = (env: CliEnvironment, argument?: string) => {
  const { client, subjectId, required } = env;
  const id = required(argument, "producer_signature_id UUID");
  if (!/^[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}$/i.test(id))
    throw new CliError(
      "INVALID_ARGUMENT",
      "producer_signature_id must be a UUID",
    );
  return client.material.producer({ subjectId, id });
};

export function createMaterialCommand(leaf: CliLeaf) {
  return defineCommand({
    args: commonArgs,
    subCommands: {
      limits: leaf({}, materialLimits),
      producer: leaf({}, materialProducer, {
        description:
          "Read a producer by the canonical ID returned on a derived representation; the signature hash is not an ID.",
        parameters: ["<producer_signature_id UUID>"],
      }),
      artifacts: leaf(
        { ...stringArgs("page-size", "page-token") },
        materialArtifacts,
      ),
      representations: leaf(
        { ...stringArgs("source-region", "artifact", "kind", "limit") },
        materialRepresentations,
      ),
    },
  });
}
