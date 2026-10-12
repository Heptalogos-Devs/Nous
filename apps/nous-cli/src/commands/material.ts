// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import { boundedInteger, CliError } from "../agent.js";
import { page, target } from "../content.js";
import type { CliEnvironment } from "../runtime.js";
export async function materialCommands(
  env: CliEnvironment,
  action?: string,
  ref?: string,
) {
  const { client, subjectId } = env;
  if (action === "clean") return client.material.clean({});
  if (action === "limits") return client.material.getLimits({});
  if (action === "artifacts")
    return client.material.listArtifacts({ subjectId, page: page(env) });
  if (action === "producer")
    return client.material.getProducer({
      subjectId,
      id: env.required(ref, "Producer signature identity"),
    });
  const limit = boundedInteger(
    env.values["page-size"] ?? "50",
    1,
    200,
    "--page-size",
  );
  if (action === "occurrences")
    return client.material.listOccurrences({
      subjectId,
      artifactId: await target(env, ref, "artifact"),
      limit,
    });
  if (action === "representations") {
    const source = ref ? await env.resolveReference(ref) : undefined;
    if (source && !["artifact", "source_region"].includes(source.kind))
      throw new CliError(
        "REFERENCE_TYPE_MISMATCH",
        "Representation list requires Artifact or SourceRegion",
      );
    return client.material.listDerivedRepresentations({
      subjectId,
      artifactId: source?.kind === "artifact" ? source.value : undefined,
      sourceRegionId:
        source?.kind === "source_region" ? source.value : undefined,
      kind: env.values.kind,
      limit,
    });
  }
  throw new CliError("INVALID_ARGUMENT", "Unknown Material command");
}
