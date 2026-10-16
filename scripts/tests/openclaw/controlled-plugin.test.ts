// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import { execFileSync, spawn } from "node:child_process";
import {
  mkdtemp,
  mkdir,
  readFile,
  writeFile,
  rm,
  access,
} from "node:fs/promises";
import { join, dirname } from "node:path";
import { tmpdir } from "node:os";
import { test } from "vitest";
import { buildDistribution } from "../../openclaw/distribution.js";
import { repositoryRoot } from "../../workspace.js";

interface RuntimePackage {
  operatorUnknown?: unknown;
  overrides: Record<string, string>;
  dependencies: Record<string, string>;
}
interface InstallationRegistry {
  plugins: Array<{ artifact: string }>;
}

// Exercises the actual pinned npm installer; optional offline mode uses its existing release cache.
test.skipIf(process.env.NOUS_TEST_NATIVE_INSTALLER !== "1")(
  "controlled local install grants only declared state, verifies artifacts and refuses replacement while state holders run",
  async () => {
    const temporary = await mkdtemp(join(tmpdir(), "nous-controlled-local-"));
    const runtime = join(temporary, "runtime");
    const distribution = join(temporary, "distribution");
    const source = join(temporary, "local-package");
    const env = {
      ...process.env,
      ...(process.env.NOUS_TEST_NATIVE_INSTALLER_OFFLINE === "1"
        ? { npm_config_offline: "true" }
        : {}),
      OPENCLAW_STATE_DIR: join(temporary, "profile"),
      OPENCLAW_CONFIG_PATH: join(temporary, "profile/config.json"),
    };
    const pluginId = "local-state-fixture",
      packageName = "@fixture/local-state";
    const packageRoot = join(runtime, "node_modules", packageName);
    const installer = join(distribution, "bin/install.mjs");
    const probe = join(
      repositoryRoot,
      "scripts/tests/openclaw/local-plugin-probe.ts",
    );
    const run = (path: string, args: string[] = []) =>
      execFileSync(process.execPath, [path, ...args], {
        env:
          path === installer
            ? {
                ...env,
                npm_command: "exec",
                npm_config_allow_scripts: "@fixture/exec-only-policy",
              }
            : env,
        encoding: "utf8",
        windowsHide: true,
        stdio: "pipe",
      });
    const install = (...args: string[]) => run(installer, [runtime, ...args]);
    const npmCli = join(
      dirname(process.execPath),
      "node_modules/npm/bin/npm-cli.js",
    );
    const npmExecutable =
      process.platform === "win32" ? process.execPath : "npm";
    const npmArgs = (args: string[]) =>
      process.platform === "win32" ? [npmCli, ...args] : args;
    const npm = (...args: string[]) =>
      execFileSync(npmExecutable, npmArgs(args), {
        cwd: runtime,
        env,
        encoding: "utf8",
        windowsHide: true,
      });
    const probeState = (location = packageRoot) => {
      const output = run(probe, [runtime, location, pluginId]);
      const line = output
        .split("\n")
        .find((value) => value.startsWith("NATIVE_STATE_PROBE:"));
      assert(line, output);
      return JSON.parse(line.slice("NATIVE_STATE_PROBE:".length)) as Record<
        string,
        string
      >;
    };
    const fixtureCode = (revision: string) =>
      `export default {id:${JSON.stringify(pluginId)},configSchema:{type:'object',additionalProperties:false},register(api){const results={revision:${JSON.stringify(revision)}};for(const name of ['openChannelIngressQueue','openKeyedStore','openBlobStore','openSyncKeyedStore']){try{api.runtime.state[name](name==='openChannelIngressQueue'?{accountId:'fixture'}:{namespace:'fixture-state',maxEntries:2});results[name]='allowed'}catch{results[name]='denied'}}console.log('NATIVE_STATE_PROBE:'+JSON.stringify(results));if(process.env.PROBE_HOLD==='1'){setInterval(()=>{},1000);process.on('SIGTERM',()=>process.exit(0))}}};\n`;
    const makeArtifact = async (revision: string) => {
      await writeFile(join(source, "index.mjs"), fixtureCode(revision));
      return join(
        temporary,
        (
          JSON.parse(
            execFileSync(
              npmExecutable,
              npmArgs([
                "pack",
                "--json",
                "--ignore-scripts",
                "--pack-destination",
                temporary,
              ]),
              { cwd: source, env, encoding: "utf8" },
            ),
          ) as Array<{ filename: string }>
        )[0]!.filename,
      );
    };
    try {
      await buildDistribution(distribution);
      await mkdir(source);
      await mkdir(join(temporary, "profile"));
      await writeFile(
        env.OPENCLAW_CONFIG_PATH,
        JSON.stringify({ operatorUnknown: { keep: true } }),
      );
      await writeFile(
        join(source, "package.json"),
        JSON.stringify({
          name: packageName,
          version: "1.0.0",
          type: "module",
          openclaw: { extensions: ["./index.mjs"] },
          nousNativeState: ["openChannelIngressQueue", "openKeyedStore"],
        }),
      );
      await writeFile(
        join(source, "openclaw.plugin.json"),
        JSON.stringify({
          id: pluginId,
          configSchema: { type: "object", additionalProperties: false },
        }),
      );
      const artifact = await makeArtifact("first");
      install();
      const staleLock = join(runtime, ".nous-install-lock");
      await mkdir(staleLock);
      await writeFile(
        join(staleLock, "owner.json"),
        JSON.stringify({ pid: process.pid }),
      );
      assert.throws(() => install(), /Controlled installer lock exists/u);
      assert.throws(() => install(), /Controlled installer lock exists/u);
      await access(staleLock);
      await rm(staleLock, { recursive: true });
      const packageJson = join(runtime, "package.json");
      const operator = JSON.parse(
        await readFile(packageJson, "utf8"),
      ) as RuntimePackage;
      operator.operatorUnknown = { keep: true };
      operator.overrides["unused-operator-package"] = "1.0.0";
      await writeFile(packageJson, JSON.stringify(operator));
      const other = join(temporary, "operator-package");
      await mkdir(other);
      await writeFile(
        join(other, "package.json"),
        JSON.stringify({ name: "@fixture/operator-package", version: "1.0.0" }),
      );
      const otherPacked = JSON.parse(
        execFileSync(
          npmExecutable,
          npmArgs([
            "pack",
            "--json",
            "--ignore-scripts",
            "--pack-destination",
            temporary,
          ]),
          { cwd: other, env, encoding: "utf8" },
        ),
      ) as Array<{ filename: string }>;
      npm(
        "install",
        "--ignore-scripts",
        "--no-audit",
        "--no-fund",
        join(temporary, otherPacked[0]!.filename),
      );
      const otherSpec = (
        JSON.parse(await readFile(packageJson, "utf8")) as RuntimePackage
      ).dependencies["@fixture/operator-package"];
      npm("install", "--ignore-scripts", "--no-audit", "--no-fund", artifact);
      assert.deepEqual(probeState(), {
        revision: "first",
        openChannelIngressQueue: "denied",
        openKeyedStore: "denied",
        openBlobStore: "denied",
        openSyncKeyedStore: "denied",
      });
      install("--plugin", artifact);
      const allowed = {
        revision: "first",
        openChannelIngressQueue: "allowed",
        openKeyedStore: "allowed",
        openBlobStore: "denied",
        openSyncKeyedStore: "denied",
      };
      assert.deepEqual(probeState(), allowed);
      const registryPath = join(runtime, "nous-local-plugins.json");
      const originalRegistry = await readFile(registryPath, "utf8");
      const registry = JSON.parse(originalRegistry) as InstallationRegistry,
        record = registry.plugins[0]!;
      const entryPath = join(packageRoot, "index.mjs"),
        originalEntry = await readFile(entryPath, "utf8");
      await writeFile(
        entryPath,
        originalEntry + "// changed installed content\n",
      );
      assert.equal(probeState().openKeyedStore, "denied");
      await writeFile(entryPath, originalEntry);
      const archivePath = join(runtime, record.artifact),
        archiveBytes = await readFile(archivePath);
      await writeFile(
        archivePath,
        Buffer.concat([archiveBytes, Buffer.from("changed")]),
      );
      assert.equal(probeState().openChannelIngressQueue, "denied");
      await writeFile(archivePath, archiveBytes);
      await writeFile(
        registryPath,
        JSON.stringify({ schemaVersion: 1, plugins: [] }),
        { mode: 0o600 },
      );
      assert.equal(probeState().openKeyedStore, "denied");
      await writeFile(registryPath, "{invalid record", { mode: 0o600 });
      assert.equal(probeState().openKeyedStore, "denied");
      await writeFile(registryPath, originalRegistry, { mode: 0o600 });
      const holder = spawn(
        process.execPath,
        [probe, runtime, packageRoot, pluginId],
        { env: { ...env, PROBE_HOLD: "1" }, stdio: ["ignore", "pipe", "pipe"] },
      );
      await new Promise<void>((resolve, reject) => {
        let text = "";
        holder.stdout.on("data", (chunk: Buffer) => {
          text += chunk.toString("utf8");
          if (text.includes("NATIVE_STATE_PROBE:")) resolve();
        });
        holder.once("error", reject);
        holder.once("exit", () =>
          reject(Error("State holder stopped before admission")),
        );
      });
      try {
        assert.throws(
          () => install("--remove-plugin", pluginId),
          /Stop the controlled runtime/u,
        );
      } finally {
        holder.kill("SIGTERM");
        await new Promise<void>((resolve) =>
          holder.once("exit", () => resolve()),
        );
      }
      assert.equal(
        (
          JSON.parse(
            await readFile(registryPath, "utf8"),
          ) as InstallationRegistry
        ).plugins.length,
        1,
      );
      const second = await makeArtifact("second");
      install("--plugin", second);
      assert.equal(probeState().revision, "second");
      assert.equal(probeState().openKeyedStore, "allowed");
      const secondEntry = await readFile(entryPath, "utf8");
      await writeFile(entryPath, originalEntry);
      assert.equal(probeState().openKeyedStore, "denied");
      await writeFile(entryPath, secondEntry);
      const restricted = JSON.parse(
        await readFile(join(source, "package.json"), "utf8"),
      ) as { nousNativeState: string[] };
      restricted.nousNativeState = ["openChannelIngressQueue"];
      await writeFile(join(source, "package.json"), JSON.stringify(restricted));
      const narrowed = await makeArtifact("narrowed");
      install("--plugin", narrowed);
      assert.equal(probeState().openChannelIngressQueue, "allowed");
      assert.equal(probeState().openKeyedStore, "denied");
      install("--remove-plugin", pluginId);
      await assert.rejects(access(packageRoot));
      const final = JSON.parse(
        await readFile(packageJson, "utf8"),
      ) as RuntimePackage;
      assert.equal(final.dependencies[packageName], undefined);
      assert.equal(final.dependencies["@fixture/operator-package"], otherSpec);
      await access(
        join(runtime, "node_modules/@fixture/operator-package/package.json"),
      );
      assert.deepEqual(final.operatorUnknown, { keep: true });
      assert.equal(final.overrides["unused-operator-package"], "1.0.0");
      assert.deepEqual(
        JSON.parse(await readFile(env.OPENCLAW_CONFIG_PATH, "utf8")),
        { operatorUnknown: { keep: true } },
      );
      assert.deepEqual(
        (
          JSON.parse(
            await readFile(registryPath, "utf8"),
          ) as InstallationRegistry
        ).plugins,
        [],
      );
    } finally {
      await rm(temporary, { recursive: true, force: true });
    }
  },
  120_000,
);
