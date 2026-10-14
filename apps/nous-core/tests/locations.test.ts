// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import {
  mkdir,
  mkdtemp,
  readFile,
  rm,
  stat,
  writeFile,
} from "node:fs/promises";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { expect, it } from "vitest";
import { claimInstance } from "../src/discovery.js";
import { resolveLocations } from "../src/locations.js";
import { initializeConfiguration } from "../src/configuration/file.js";
import { parseConfiguration, loadConfig } from "../src/configuration/schema.js";
import { checkConfiguration } from "../src/configuration/check.js";

it("resolves split roots relative to the locator while installation and instance remain independent", async () => {
  const root = await mkdtemp(join(tmpdir(), "nous-locations-"));
  try {
    const locator = join(root, "bootstrap.toml");
    await writeFile(
      locator,
      '[paths]\ndata="authority"\nrun="process"\nconfig="settings"\nblob="media"\n',
    );
    const locations = await resolveLocations({
      locator,
      installationHome: join(root, "install"),
    });
    expect(locations.program).toBe(join(root, "install", "program"));
    expect(locations.data).toBe(join(root, "authority"));
    expect(locations.run).toBe(join(root, "process"));
    expect(locations.config).toBe(join(root, "settings"));
    expect(locations.blob).toBe(join(root, "media"));
    expect(Object.isFrozen(locations)).toBe(true);
    await expect(
      resolveLocations({ home: root, locator, installationHome: root }),
    ).rejects.toThrow("Choose");
    await writeFile(locator, '[paths]\nunknown="escape"\n');
    await expect(
      resolveLocations({ locator, installationHome: root }),
    ).rejects.toThrow();
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

it("initializes a portable instance once and preserves operator edits byte for byte", async () => {
  const root = await mkdtemp(join(tmpdir(), "nous-initial-config-"));
  try {
    const locations = await resolveLocations({
      home: root,
      installationHome: join(root, "install"),
    });
    const results = await Promise.all([
      initializeConfiguration(locations),
      initializeConfiguration(locations),
    ]);
    expect(results.filter((result) => result.created)).toHaveLength(1);
    const config = await loadConfig(locations);
    expect(config.deployment).toBe("portable");
    expect(
      config.kernelBootstrap.configuration.core_descriptors.some(
        (d) => d.path === "models",
      ),
    ).toBe(true);

    const development = await loadConfig(locations, true);
    expect(development.deployment).toBe("development");
    expect(development.kernelExecutable).toBe(
      join(
        locations.program,
        "target/debug",
        process.platform === "win32" ? "nous-kernel.exe" : "nous-kernel",
      ),
    );
    const edited = `# operator configuration\r\n[host]\r\nport = 12345\r\n[[consumers]]\r\nconsumer_id = "custom"\r\n`;
    await writeFile(results[0]!.path, edited);
    expect((await initializeConfiguration(locations)).created).toBe(false);
    expect(await readFile(results[0]!.path, "utf8")).toBe(edited);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

it("checks current configuration examples and explicit references offline", async () => {
  const example = await readFile(
    join(process.cwd(), "docs/reference/examples/nous.toml"),
    "utf8",
  );
  expect(parseConfiguration(example).config.host.port).toBe(9470);

  const root = await mkdtemp(join(tmpdir(), "nous-config-check-"));
  try {
    const locations = await resolveLocations({
      home: root,
      installationHome: process.cwd(),
      programRoot: process.cwd(),
    });
    const initialized = await initializeConfiguration(locations);
    const text = `[[consumers]]\nconsumer_id = "default"\n[models.gateway_profiles.local]\nbase_url = "http://127.0.0.1:3000/v1"\ncredential_env = "NOUS_OFFLINE_CHECK_TOKEN"\n[models.model_profiles.local]\ngateway = "local"\nprotocol = "openai-chat"\nmodel = "local"\ncapabilities = ["text", "structured_output"]\n[models.execution_profiles.local]\nmodel = "local"\n[models.roles.memory_formation]\nroutes = ["local"]\nprompt = "config-prompts/missing.md"\n`;
    const missingKernel = `\n[host]\nkernel_executable = ${JSON.stringify(join(root, "missing-kernel"))}\n`;
    await writeFile(initialized.path, text + missingKernel);
    const result = await checkConfiguration(locations, true);
    expect(result.issues).toContainEqual(
      expect.objectContaining({
        path: "models.roles.memory_formation.prompt",
        code: "invalid_reference",
      }),
    );
    expect(result.issues).toContainEqual(
      expect.objectContaining({
        path: "host.kernel_executable",
        code: "validator_unavailable",
      }),
    );
    expect(await readFile(initialized.path, "utf8")).toBe(text + missingKernel);
    expect(process.env.NOUS_OFFLINE_CHECK_TOKEN).toBeUndefined();
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

it("releases a failed identity claim and atomically publishes split-root discovery", async () => {
  const root = await mkdtemp(join(tmpdir(), "nous-discovery-"));
  try {
    const locations = await resolveLocations({
      home: root,
      installationHome: root,
    });
    await mkdir(locations.instance, { recursive: true });
    const identity = join(locations.instance, "instance.json");
    await writeFile(identity, "invalid-json");
    await expect(claimInstance(locations)).rejects.toThrow();
    await expect(
      stat(join(locations.instance, "instance.lock")),
    ).rejects.toMatchObject({ code: "ENOENT" });
    await rm(identity);
    const instance = await claimInstance(locations);
    try {
      await instance.publish("http://127.0.0.1:12345", "fixture");
      expect(JSON.parse(await readFile(instance.path, "utf8"))).toMatchObject({
        endpoint: "http://127.0.0.1:12345",
        pid: process.pid,
      });
      await expect(stat(`${instance.path}.tmp`)).rejects.toMatchObject({
        code: "ENOENT",
      });
      await expect(claimInstance(locations)).rejects.toThrow("already owns");
    } finally {
      await instance.release();
    }
    await expect(stat(instance.path)).rejects.toMatchObject({ code: "ENOENT" });
    await expect(
      stat(join(locations.instance, "instance.lock")),
    ).rejects.toMatchObject({ code: "ENOENT" });
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});
