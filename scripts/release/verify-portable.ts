// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { spawn, execFile, type ChildProcess } from "node:child_process";
import { promisify, parseArgs } from "node:util";
import { once } from "node:events";
import { mkdir, readFile, writeFile, rename, rm } from "node:fs/promises";
import { join, resolve, sep } from "node:path";
import { createReadStream } from "node:fs";
import { createHash } from "node:crypto";
import { existsSync } from "node:fs";
import { pathToFileURL } from "node:url";
import { workspaceTemp } from "../workspace.js";
import assert from "node:assert/strict";
import { parse, stringify } from "smol-toml";
const execute = promisify(execFile);
const { values } = parseArgs({
  options: {
    bundle: { type: "string" },
    layout: { type: "string", default: "home" },
    relocate: { type: "boolean", default: false },
  },
});
if (!values.bundle || !values.bundle.endsWith(".zip"))
  throw new Error("Provide --bundle <exact portable ZIP>");
const verificationRoot = await workspaceTemp("portable", "verification-");
if (!["home", "colocated", "locator"].includes(values.layout))
  throw new Error("Unknown portable layout");
if (values.layout === "colocated" && values.relocate)
  throw new Error("Relocation uses an independent instance");
let bundle = join(verificationRoot, "installation");
const home =
  values.layout === "colocated"
    ? bundle
    : join(verificationRoot, "separate-instance");
const locator = join(verificationRoot, "locator/bootstrap.toml");
const configRoot =
  values.layout === "locator"
    ? join(verificationRoot, "configuration-volume")
    : join(home, "config");
const runRoot =
  values.layout === "locator"
    ? join(verificationRoot, "runtime-state")
    : join(home, "run");
const instanceRoot =
  values.layout === "locator"
    ? join(verificationRoot, "identity-volume")
    : join(home, "instance");
const locationArgs =
  values.layout === "locator" ? ["--locator", locator] : ["--home", home];
async function writeLocator() {
  if (values.layout !== "locator") return;
  await mkdir(join(verificationRoot, "locator"), { recursive: true });
  const paths = {
    program: join(bundle, "program"),
    runtime: join(bundle, "runtime"),
    config: configRoot,
    run: runRoot,
    instance: instanceRoot,
    data: join(verificationRoot, "database-volume"),
    blob: join(verificationRoot, "blob-volume"),
    cache: join(verificationRoot, "cache-volume"),
    secret: join(verificationRoot, "secret-volume"),
    log: join(verificationRoot, "log-volume"),
    temp: join(verificationRoot, "temp-volume"),
    backup: join(verificationRoot, "backup-volume"),
  };
  await writeFile(
    locator,
    "[paths]\n" +
      Object.entries(paths)
        .map(
          ([key, value]) =>
            `${key} = ${JSON.stringify(value.replaceAll("\\", "/"))}`,
        )
        .join("\n"),
  );
}
const archive = resolve(values.bundle);
const literal = (value: string) => "'" + value.replaceAll("'", "''") + "'";
await execute(
  join(
    process.env.SystemRoot!,
    "System32/WindowsPowerShell/v1.0/powershell.exe",
  ),
  [
    "-NoProfile",
    "-Command",
    `Expand-Archive -LiteralPath ${literal(archive)} -DestinationPath ${literal(bundle)}`,
  ],
  { windowsHide: true },
);
const checksums = JSON.parse(
  await readFile(join(bundle, "manifest/checksums.json"), "utf8"),
) as { path: string; sha256: string }[];
for (const item of checksums) {
  const path = resolve(bundle, item.path);
  assert(path.startsWith(resolve(bundle) + sep));
  const hash = createHash("sha256");
  for await (const bytes of createReadStream(path))
    hash.update(bytes as Buffer);
  assert.equal(hash.digest("hex"), item.sha256, item.path);
}
const components = JSON.parse(
  await readFile(join(bundle, "manifest/components.json"), "utf8"),
) as { missing_cargo_notices: string[] };
assert.deepEqual(components.missing_cargo_notices, []);
const sbom = JSON.parse(
  await readFile(join(bundle, "manifest/sbom.spdx.json"), "utf8"),
) as {
  packages: { SPDXID: string }[];
  relationships: { spdxElementId: string; relatedSpdxElement: string }[];
};
const ids = new Set([
  "SPDXRef-DOCUMENT",
  ...sbom.packages.map((p) => p.SPDXID),
]);
assert.equal(ids.size, sbom.packages.length + 1);
for (const relation of sbom.relationships)
  assert(
    ids.has(relation.spdxElementId) && ids.has(relation.relatedSpdxElement),
  );
for (const path of [
  "licenses/THIRD_PARTY_NOTICES.md",
  "licenses/Nous-Wave-Apache-2.0.txt",
  "licenses/Nous-Wave-NOTICE.txt",
  "licenses/native/LLVM-LICENSE.txt",
  "licenses/native/Rust-library-COPYRIGHT.html",
])
  assert((await readFile(join(bundle, path))).length > 0);
await writeLocator();
assert(!existsSync(join(bundle, "config/nous.toml")));
assert(!existsSync(join(bundle, "program/templates/nous.toml")));
assert(!existsSync(join(bundle, "bootstrap.toml")));
let node = join(bundle, "runtime/node/node.exe"),
  launcher = join(bundle, "program/core/launcher.js");
await execute(node, [launcher, "init", ...locationArgs], {
  cwd: verificationRoot,
  windowsHide: true,
});
const initial = await readFile(join(configRoot, "nous.toml"), "utf8");
await execute(node, [launcher, "init", ...locationArgs], {
  cwd: verificationRoot,
  windowsHide: true,
});
assert.equal(await readFile(join(configRoot, "nous.toml"), "utf8"), initial);
await mkdir(configRoot, { recursive: true });
const configuration = parse(initial);
configuration.host = {
  ...(configuration.host as Record<string, unknown> | undefined),
  port: 0,
};
await writeFile(join(configRoot, "nous.toml"), stringify(configuration));
const env = {
  ...process.env,
  PATH: join(process.env.SystemRoot!, "System32"),
  NODE_PATH: "",
  NODE_OPTIONS: "",
};
let core: ChildProcess | undefined;
async function boot() {
  core = spawn(
    node,
    [launcher, "serve", ...locationArgs, "--stop-on-stdin-close"],
    {
      cwd: verificationRoot,
      env,
      stdio: ["pipe", "pipe", "pipe"],
      windowsHide: true,
    },
  );
  const child = core;
  let errors = "";
  child.stderr!.on("data", (bytes: Buffer) => {
    if (errors.length < 4096) errors += bytes.toString();
  });
  await new Promise<void>((done, failed) => {
    const timer = setTimeout(
      () => failed(new Error("Portable boot timed out: " + errors)),
      120000,
    );
    child.once("exit", (code) => {
      clearTimeout(timer);
      failed(new Error("Portable boot exited " + code + ": " + errors));
    });
    child.once("error", (error) => {
      clearTimeout(timer);
      failed(error);
    });
    let pending = "";
    child.stdout!.on("data", (bytes: Buffer) => {
      pending += bytes.toString();
      if (pending.includes('"discovery"')) {
        clearTimeout(timer);
        done();
      }
    });
  });
}
async function stop() {
  if (!core || core.exitCode !== null) return;
  const exited = once(core, "exit");
  core.stdin!.end();
  await exited;
  core = undefined;
}
async function cli(...args: string[]) {
  const result = await execute(
    node,
    [launcher, ...locationArgs, "--json", ...args],
    { cwd: verificationRoot, env, windowsHide: true, maxBuffer: 1048576 },
  );
  const envelope = JSON.parse(result.stdout) as {
    schemaVersion: string;
    data: Record<string, unknown>;
  };
  assert.equal(envelope.schemaVersion, "nous.cli.v1");
  return envelope.data;
}
let passed = false;
try {
  await boot();
  const status = await cli("status");
  assert(status.status);
  const subject = await cli("subject", "create");
  const subjectId = String(subject.subjectId);
  await cli("session", "open");
  const observation = await cli(
    "observe",
    "text",
    "--text",
    "Synthetic portable wiring: stable source identity.",
  );
  const clientModule = (await import(
    pathToFileURL(join(bundle, "program/client/node.js")).href
  )) as typeof import("@nous-wave/client/node");
  const client = await clientModule.connectNousInstance({
    runRoot,
  });
  const occurrenceId = String(observation.occurrenceId);
  const derived = await client.model.deriveMaterial({
    subjectId,
    sourceRegionId: String(observation.sourceRegionId),
    strategy: "description_only",
  });
  const memory = await client.memory.formMemory({
    subjectId,
    operationId: crypto.randomUUID(),
    input: {
      cognitiveRole: "declarative",
      formationMode: "grounded",
      groundingOccurrenceId: occurrenceId,
      semanticRole: "fact",
      text: "Synthetic portable wiring: stable source identity.",
      epistemicClass: "observed",

      basis: [
        {
          basis: {
            case: "evidence",
            value: {
              occurrenceId,
              basisRole: "direct",
              locator: {
                case: "derivedRepresentationId",
                value: derived.selectedRepresentationId!,
              },
            },
          },
        },
      ],
    },
  });
  const response = await client.cognition.query({
    subjectId: subjectId,
    nousql: "stable source identity $return(memory) $limit(5)",
  });
  assert(
    response.hits.some(
      (hit: { revision?: { value: string } }) =>
        hit.revision?.value === memory.revisionId,
    ),
  );
  await cli("trace", `memory_revision:${memory.revisionId}`);
  await cli("use", `memory_revision:${memory.revisionId}`);
  const databaseState = await readFile(
    join(instanceRoot, "postgres.json"),
    "utf8",
  );
  await stop();
  if (values.relocate) {
    const moved = join(verificationRoot, "moved-installation");
    await rename(bundle, moved);
    bundle = moved;
    node = join(bundle, "runtime/node/node.exe");
    launcher = join(bundle, "program/core/launcher.js");
    await writeLocator();
  }
  await boot();
  const restarted = await clientModule.connectNousInstance({
    runRoot,
  });
  const recalled = await restarted.cognition.query({
    subjectId: subjectId,
    nousql: "stable source identity $return(memory) $limit(5)",
  });
  assert(
    recalled.hits.some(
      (hit: { revision?: { value: string } }) =>
        hit.revision?.value === memory.revisionId,
    ),
  );
  assert.equal(
    await readFile(join(instanceRoot, "postgres.json"), "utf8"),
    databaseState,
  );
  await stop();
  const postgresPack = join(bundle, "runtime/postgresql");
  const hiddenPack = join(bundle, "runtime/postgresql-missing");
  assert(
    postgresPack.startsWith(bundle + "\\") ||
      postgresPack.startsWith(bundle + "/"),
  );
  assert(
    hiddenPack.startsWith(bundle + "\\") || hiddenPack.startsWith(bundle + "/"),
  );
  await rename(postgresPack, hiddenPack);
  try {
    await assert.rejects(
      execute(
        node,
        [launcher, "serve", ...locationArgs, "--stop-on-stdin-close"],
        { cwd: verificationRoot, env, windowsHide: true, timeout: 30000 },
      ),
      (error: unknown) =>
        error instanceof Error &&
        "stderr" in error &&
        /runtime install postgresql/.test(String(error.stderr)),
    );
    assert.equal(
      await readFile(join(instanceRoot, "postgres.json"), "utf8"),
      databaseState,
    );
  } finally {
    await rename(hiddenPack, postgresPack);
  }
  passed = true;
  console.log(
    JSON.stringify({
      result: "PASS",
      platform: process.platform,
      bundle,
      archive,
      layout: values.layout,
      relocated: values.relocate,
      verificationRootRepository: true,
      developerPath: false,
      sourceLess: true,
      restart: true,
      stableDatabasePort: true,
      missingPackRejected: true,
      subjectId,
      memoryId: memory.memoryId,
      liveModel: "NOT_RUN",
      corpus: "synthetic_portable_wiring",
    }),
  );
} finally {
  await stop();
  if (passed) await rm(verificationRoot, { recursive: true, force: true });
}
