// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import { writeFile } from "node:fs/promises";
import { join } from "node:path";
import { connectNousInstance } from "@nous-wave/client/node";
import type { SelfRecord } from "@nous-wave/protocol/nous/wave/v1alpha1/self_pb.js";
import { runCli } from "../../apps/nous-cli/src/commands/index.js";
import { cliExecutor } from "../../apps/nous-cli/src/mcp.js";

export async function selfCliSmoke(
  home: string,
  subjectId: string,
  seedOperationId: string,
) {
  const runRoot = join(home, "run");
  const instanceRoot = join(home, "self-cli");
  const cli = (...args: string[]) =>
    runCli([
      "--run-root",
      runRoot,
      "--instance-root",
      instanceRoot,
      "--subject",
      subjectId,
      ...args,
    ]);
  const data = async <T>(...args: string[]) =>
    ((await cli(...args)) as { data: T }).data;
  const first = await data<{ actions: { record: SelfRecord }[] }>(
    "self",
    "interpret-seed",
    "--operation-id",
    seedOperationId,
  );
  assert(first.actions[0]?.record.reference);
  const trace = await data<{ nodes: { content: { case: string } }[] }>(
    "trace",
    `${first.actions[0]!.record.reference!.kind}:${first.actions[0]!.record.reference!.value}`,
  );
  assert(trace.nodes.some((node) => node.content.case === "seed"));
  const projected = await data<{ text: string }>("self", "soul");
  assert(projected.text.includes("青岚"));
  const ordinary = cliExecutor({
    runRoot,
    stateRoot: join(home, "self-mcp"),
    consumer: "default",
  });
  const managed = cliExecutor({
    runRoot,
    stateRoot: join(home, "self-mcp-management"),
    consumer: "default",
    management: true,
  });
  const denied = await ordinary([
    "--subject",
    subjectId,
    "self",
    "interpret-seed",
  ]);
  assert.equal(denied.isError, true);
  assert(JSON.stringify(denied).includes("PERMISSION_DENIED"));
  assert(!JSON.stringify(denied).includes('"receipt"'));
  assert.equal(
    (await ordinary(["--subject", subjectId, "self", "soul"])).isError,
    true,
  );
  assert.equal(
    (
      await managed([
        "--subject",
        subjectId,
        "self",
        "interpret-seed",
        "--operation-id",
        seedOperationId,
      ])
    ).isError,
    undefined,
  );
  const connection = await connectNousInstance({ runRoot });
  const source = await connection.runtime.recordObservation({
    subjectId,
    requestId: crypto.randomUUID(),
    sourceClass: "generated",
    material: {
      case: "inlineText",
      value: {
        text: "我明确选择：核对项目事实时优先阅读原始材料；服务不可用时说明限制。此偏好仅适用于工作核验，闲聊不强制工具调用。",
        mediaType: "text/plain",
      },
    },
    admit: false,
  });
  const occurrenceId = source.occurrenceId;
  const contentFile = join(home, "self-cli-content.toml");
  const writeContent = (statement: string) =>
    writeFile(
      contentFile,
      `
[facet]
kind="preference"
statement="${statement}"
stance="adopted"
epistemic_class="observed"
[facet.scope]
description="核对项目事实时适用，闲聊不强制工具调用。"
purposes=["professional_execution"]
[facet.valid_time]
time_domain_ref="time:real"
[facet.valid_time.extent]
[[facet.basis]]
[facet.basis.evidence]
occurrence_id="${occurrenceId}"
whole_occurrence=true
basis_role="direct"
`,
    );
  await writeContent("核对项目事实时优先阅读原始材料。");
  const args = [
    "--subject",
    subjectId,
    "self",
    "adopt",
    "--request-file",
    contentFile,
    "--text",
    "明确采用有来源的工作情境偏好。",
  ];
  assert.equal((await ordinary(args)).isError, true);
  const original = await data<SelfRecord>(
    "self",
    "adopt",
    "--request-file",
    contentFile,
    "--text",
    "显式管理采用工作情境偏好。",
  );
  const reference = `${original.objectRef!.kind}:${original.objectRef!.value}`;
  assert.equal(
    (await data<SelfRecord>("self", "get", reference)).reference?.value,
    original.reference?.value,
  );
  await writeContent("核对项目事实时优先阅读原始材料；服务不可用时说明限制。");
  const revised = await data<SelfRecord>(
    "self",
    "revise",
    `${original.reference!.kind}:${original.reference!.value}`,
    "--request-file",
    contentFile,
    "--text",
    "明确补充工具不可用的边界。",
  );
  assert.equal(revised.objectRef?.value, original.objectRef?.value);
  assert.equal(
    (
      await data<SelfRecord>(
        "self",
        "revision",
        `${original.reference!.kind}:${original.reference!.value}`,
      )
    ).content?.content.case,
    "facet",
  );
  assert.equal(
    (await data<{ items: unknown[] }>("self", "history", reference)).items
      .length,
    2,
  );
  assert.equal(
    (
      await data<SelfRecord>(
        "show",
        `${original.reference!.kind}:${original.reference!.value}`,
      )
    ).reference?.value,
    original.reference?.value,
  );
  const results = await data<{
    results: { ref: { value: string }; next: string }[];
  }>(
    "query",
    "核对项目事实时优先阅读原始材料；服务不可用时说明限制。 $return(self_facet) $limit(5)",
  );
  const index = results.results.findIndex(
    (result) => result.ref.value === revised.reference!.value,
  );
  assert(index >= 0);
  assert(results.results[index]!.next.includes("show result:"));
  assert(results.results[index]!.next.includes("trace result:"));
  assert.equal(
    (await data<SelfRecord>("show", `result:${index + 1}`)).reference?.value,
    revised.reference?.value,
  );
  const used = await data<{ acceptedCount: number }>(
    "use",
    `result:${index + 1}`,
  );
  assert.equal(used.acceptedCount, 1);
  const evidenceFile = join(home, "self-cli-evidence.toml");
  await writeFile(
    evidenceFile,
    `role="boundary_case"\n[basis.evidence]\noccurrence_id="${occurrenceId}"\nwhole_occurrence=true\nbasis_role="direct"\n`,
  );
  const evidence = await data<SelfRecord>(
    "self",
    "evidence",
    reference,
    "--request-file",
    evidenceFile,
  );
  assert.equal(evidence.reference?.value, revised.reference?.value);
  assert.equal(evidence.evidence.length, 1);
  await cli("self", "suppress", reference);
  assert(
    !(await data<{ text: string }>("self", "soul")).text.includes(
      "服务不可用时说明限制",
    ),
  );
  await cli("self", "restore", reference);
  assert(
    (await data<{ text: string }>("self", "soul")).text.includes(
      "服务不可用时说明限制",
    ),
  );
  const current = await data<SelfRecord>("self", "get", reference);
  const purge = [
    "self",
    "purge",
    reference,
    "--expected-object-epoch",
    current.objectEpoch.toString(),
    "--operation-id",
    crypto.randomUUID(),
  ];
  await cli(...purge);
  await cli(...purge);
  await assert.rejects(
    connection.self.getRevision({ subjectId, id: original.reference!.value }),
  );
  console.log(
    "Self CLI/MCP contracts passed: official operations, derived soul, exact history, purge replay and operator-fixed management scope.",
  );
}
