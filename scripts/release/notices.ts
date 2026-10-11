// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { workspacePaths } from "../workspace.js";
import { createHash } from "node:crypto";
import {
  cp,
  mkdir,
  readFile,
  readdir,
  stat,
  writeFile,
} from "node:fs/promises";
import { dirname, join } from "node:path";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import type { BundledPackage } from "./bundle.js";
const execute = promisify(execFile);
let repo = "";
export async function preparedPublicFile(repositoryRoot: string, url: string) {
  const key = createHash("sha256").update(url).digest("hex");
  return readFile(
    join(repositoryRoot, "data", "cache", "licenses", key),
    "utf8",
  ).catch(() => {
    throw new Error(
      "Prepared notice missing; run corepack pnpm release:notices",
    );
  });
}
async function publicFile(url: string) {
  const key = createHash("sha256").update(url).digest("hex");
  const cache = join(repo, "data", "cache", "licenses", key);
  const cached = await readFile(cache, "utf8").catch(() => undefined);
  if (cached !== undefined) return cached;
  for (let attempt = 0; attempt < 2; attempt++) {
    try {
      const response = await fetch(url, {
        signal: AbortSignal.timeout(30000),
        headers: { connection: "close" },
      });
      if (response.status === 404) return undefined;
      if (!response.ok) throw new Error("Public notice fetch failed");
      const content = await response.text();
      await mkdir(dirname(cache), { recursive: true });
      await writeFile(cache, content);
      return content;
    } catch (error) {
      if (attempt === 1) {
        const source =
          /^https:\/\/raw\.githubusercontent\.com\/([^/]+\/[^/]+)\/([^/]+)\/(.+)$/.exec(
            url,
          );
        if (!source) throw error;
        try {
          const response = await execute(
            "gh",
            [
              "api",
              `repos/${source[1]}/contents/${source[3]}?ref=${source[2]}`,
              "--jq",
              ".content",
            ],
            { windowsHide: true, maxBuffer: 2 * 1024 * 1024 },
          );
          const content = Buffer.from(
            response.stdout.replaceAll(/\s/g, ""),
            "base64",
          ).toString("utf8");
          await mkdir(dirname(cache), { recursive: true });
          await writeFile(cache, content);
          return content;
        } catch (fallback) {
          if (
            fallback instanceof Error &&
            "stderr" in fallback &&
            String(fallback.stderr).includes("HTTP 404")
          )
            return undefined;
          throw error;
        }
      }
    }
  }
  throw new Error("Public notice unavailable");
}

async function noticeKey(
  repositoryRoot: string,
  packages: Map<string, BundledPackage>,
) {
  const hash = createHash("sha256");
  for (const name of [
    "Cargo.lock",
    "Cargo.toml",
    "pnpm-lock.yaml",
    "LICENSE",
    "scripts/release/notices.ts",
    "scripts/release/manifest.ts",
    "data/runtime/manifest/runtimes.json",
  ])
    hash.update(await readFile(join(repositoryRoot, name)));
  hash.update(
    JSON.stringify(
      [...packages.values()]
        .map(({ name, version, license }) => ({ name, version, license }))
        .sort((a, b) => a.name.localeCompare(b.name)),
    ),
  );
  return hash.digest("hex");
}
export async function prepareNotices(
  repositoryRoot: string,
  packages: Map<string, BundledPackage>,
  runtimeRoot = workspacePaths.runtime,
) {
  repo = repositoryRoot;
  const catalog = JSON.parse(
    await readFile(join(runtimeRoot, "manifest/runtimes.json"), "utf8"),
  ) as {
    packs: {
      component: string;
      platform: string;
      arch: string;
      version: string;
    }[];
  };
  const node = catalog.packs.find(
    (p) =>
      p.component === "node" &&
      p.platform === process.platform &&
      p.arch === process.arch,
  );
  if (!node) throw new Error("Node runtime catalog missing");
  for (const path of [
    "deps/nbytes/LICENSE",
    "LICENSE",
    "deps/sqlite/sqlite3.h",
  ])
    await publicFile(
      `https://raw.githubusercontent.com/nodejs/node/v${node.version}/${path}`,
    );
  const output = join(
    workspacePaths.cache,
    "release/notices",
    await noticeKey(repo, packages),
  );
  const existing = await readFile(join(output, "inventory.json"), "utf8").catch(
    () => undefined,
  );
  if (existing) return output;
  await mkdir(output, { recursive: true });
  await mkdir(join(output, "licenses/npm"), { recursive: true });
  await cp(
    join(repo, "LICENSE"),
    join(output, "licenses/Nous-Wave-Apache-2.0.txt"),
  );
  await cp(join(repo, "NOTICE"), join(output, "licenses/Nous-Wave-NOTICE.txt"));
  for (const pkg of packages.values()) {
    const destination = join(
      output,
      "licenses/npm",
      pkg.name.replace(/[\\/@]/g, "_") + "-" + pkg.version,
    );
    await mkdir(destination, { recursive: true });
    const notices = (await readdir(pkg.root)).filter((name) =>
      /^(licen[cs]e|copying|notice)(\.|$)/i.test(name),
    );
    if (!notices.length) {
      const metadata = JSON.parse(
        await readFile(join(pkg.root, "package.json"), "utf8"),
      ) as { repository?: string | { url?: string } };
      const repository =
        typeof metadata.repository === "string"
          ? metadata.repository
          : metadata.repository?.url;
      const match =
        repository &&
        /github\.com[/:]([^/]+\/[^/.]+)(?:\.git)?/.exec(repository);
      if (!match)
        throw new Error(
          "Bundled dependency has no traceable license source: " + pkg.name,
        );
      let base = `https://raw.githubusercontent.com/${match[1]}/v${pkg.version}/`;
      let licenseSource = base + "LICENSE";
      let response = await publicFile(licenseSource);
      if (response === undefined) {
        base = `https://raw.githubusercontent.com/${match[1]}/${encodeURI(pkg.name + "@" + pkg.version)}/`;
        licenseSource = base + "LICENSE";
        response = await publicFile(licenseSource);
      }
      if (response !== undefined)
        await writeFile(join(destination, "LICENSE"), response);
      else {
        const readmeName = (await readdir(pkg.root)).find((name) =>
          /^readme\.md$/i.test(name),
        );
        const readme = readmeName
          ? await readFile(join(pkg.root, readmeName), "utf8")
          : "";
        const linked = /\[(?:MIT )?License\]\((https?:\/\/[^)]+)\)/i.exec(
          readme,
        );
        if (!linked)
          throw new Error(
            "Version-specific license source unavailable: " + pkg.name,
          );
        const url = new URL(linked[1]!);
        url.protocol = "https:";
        licenseSource = url.toString();
        const license = await publicFile(licenseSource);
        if (license === undefined)
          throw new Error("Publisher license source unavailable: " + pkg.name);
        await writeFile(join(destination, "LICENSE.html"), license);
        await writeFile(join(destination, "README-license-source.md"), readme);
      }
      const notice = await publicFile(base + "NOTICE");
      if (notice !== undefined)
        await writeFile(join(destination, "NOTICE"), notice);
      await writeFile(
        join(destination, "source.json"),
        JSON.stringify({
          license_source: licenseSource,
          package: pkg.name,
          version: pkg.version,
        }) + "\n",
      );
    }
    for (const name of notices)
      if ((await stat(join(pkg.root, name))).isFile())
        await cp(join(pkg.root, name), join(destination, name));
  }
  const inventory = [...packages.values()].map(
    ({ name, version, license }) => ({
      name,
      version,
      license,
    }),
  );
  const kernelClosure = await kernelInventory(repo, output);
  if (kernelClosure.missingNotices.length)
    throw new Error(
      "Kernel license text missing: " + kernelClosure.missingNotices.join(", "),
    );

  const compilerRoot = join(
    workspacePaths.tools,
    "llvm-mingw-windows/llvm-mingw-20260922-ucrt-x86_64",
  );
  await mkdir(join(output, "licenses/native"), { recursive: true });
  await cp(
    join(compilerRoot, "LICENSE.TXT"),
    join(output, "licenses/native/LLVM-LICENSE.txt"),
  );
  await cp(
    join(compilerRoot, "x86_64-w64-mingw32/share/mingw32"),
    join(output, "licenses/native/mingw-runtime"),
    { recursive: true },
  );
  const rustSysroot = (
    await execute("rustc", ["--print", "sysroot"], { windowsHide: true })
  ).stdout.trim();
  const rustVersion = (await execute("rustc", ["-Vv"], { windowsHide: true }))
    .stdout;
  await cp(
    join(rustSysroot, "share/doc/rust/COPYRIGHT-library.html"),
    join(output, "licenses/native/Rust-library-COPYRIGHT.html"),
  );
  await cp(
    join(rustSysroot, "share/doc/rust/licenses"),
    join(output, "licenses/native/rust"),
    { recursive: true },
  );
  await writeFile(join(output, "licenses/native/rust-source.txt"), rustVersion);
  await writeFile(
    join(output, "inventory.json"),
    JSON.stringify({ inventory, kernelClosure }),
  );
  return output;
}
export async function loadNotices(
  repositoryRoot: string,
  packages: Map<string, BundledPackage>,
  output: string,
) {
  const cache = join(
    workspacePaths.cache,
    "release/notices",
    await noticeKey(repositoryRoot, packages),
  );
  const metadata = JSON.parse(
    await readFile(join(cache, "inventory.json"), "utf8").catch(() => {
      throw new Error(
        "Prepared notices missing; run corepack pnpm release:notices",
      );
    }),
  ) as {
    inventory: { name: string; version: string; license: string }[];
    kernelClosure: {
      components: Component[];
      relationships: Relationship[];
      missingNotices: string[];
      root: string;
    };
  };
  await cp(join(cache, "licenses"), join(output, "licenses"), {
    recursive: true,
  });
  return metadata;
}
type CargoPackage = {
  id: string;
  name: string;
  version: string;
  license: string | null;
  license_file: string | null;
  manifest_path: string;
  source: string | null;
  repository: string | null;
};
type CargoNode = {
  id: string;
  deps: { pkg: string; dep_kinds: { kind: string | null }[] }[];
};
export type Component = {
  id: string;
  name: string;
  version: string;
  license: string;
  source: string;
  purpose: "APPLICATION" | "LIBRARY" | "OTHER";
  notice?: string;
};
export type Relationship = {
  spdxElementId: string;
  relationshipType: string;
  relatedSpdxElement: string;
};

/** The release graph excludes test-only edges and records build inputs explicitly. */
async function kernelInventory(repositoryRoot: string, bundleRoot: string) {
  const executeCargo = promisify(execFile);
  const metadata = JSON.parse(
    (
      await executeCargo(
        "cargo",
        [
          "metadata",
          "--locked",
          "--format-version",
          "1",
          "--filter-platform",
          "x86_64-pc-windows-gnullvm",
        ],
        { cwd: repositoryRoot, windowsHide: true, maxBuffer: 16 * 1024 * 1024 },
      )
    ).stdout,
  ) as {
    packages: CargoPackage[];
    resolve: { nodes: CargoNode[] };
  };
  const cargoPackages = new Map(metadata.packages.map((pkg) => [pkg.id, pkg]));
  const nodes = new Map(metadata.resolve.nodes.map((node) => [node.id, node]));
  const kernel = metadata.packages.find(
    (pkg) => pkg.name === "nous-kernel" && !pkg.source,
  );
  if (!kernel) throw new Error("Kernel release dependency root missing");
  const selected = new Set<string>();
  const buildOnly = new Set<string>();
  const pending = [{ id: kernel.id, build: false }];
  const walked = new Set<string>();
  while (pending.length) {
    const item = pending.pop()!;
    const traversal = `${item.id}:${item.build}`;
    if (walked.has(traversal)) continue;
    walked.add(traversal);
    if (!selected.has(item.id) && item.build) buildOnly.add(item.id);
    if (!item.build) buildOnly.delete(item.id);
    selected.add(item.id);
    for (const dep of nodes.get(item.id)?.deps ?? []) {
      for (const kind of dep.dep_kinds) {
        if (kind.kind === "dev") continue;
        pending.push({
          id: dep.pkg,
          build: item.build || kind.kind === "build",
        });
      }
    }
  }
  const ordered = [...selected].sort();
  const ids = new Map(
    ordered.map((id, index) => [id, `SPDXRef-cargo-${index}`]),
  );
  const cargoComponents: Component[] = [];
  const relationships: Relationship[] = [];
  const missingNotices: string[] = [];
  for (const id of ordered) {
    const pkg = cargoPackages.get(id);
    if (!pkg) throw new Error("Release dependency package missing");
    const root = dirname(pkg.manifest_path);
    const destination = join(
      bundleRoot,
      "licenses/cargo",
      pkg.name + "-" + pkg.version,
    );
    await mkdir(destination, { recursive: true });
    await cp(pkg.manifest_path, join(destination, "Cargo.toml"));
    const names = (await readdir(root)).filter((name) =>
      /^(?:license|copying|notice|readme)(?:[._-]|$)/i.test(name),
    );
    if (pkg.license_file && !names.includes(pkg.license_file))
      names.push(pkg.license_file);
    let notices = 0;
    for (const name of names) {
      const content = await readFile(join(root, name)).catch(
        (error: NodeJS.ErrnoException) => {
          if (error.code === "EISDIR") return undefined;
          throw error;
        },
      );
      if (!content) continue;
      await writeFile(
        join(destination, name.replaceAll(/[\\/]/g, "_")),
        content,
      );
      if (
        /^(?:license|copying|notice)/i.test(name) ||
        name === pkg.license_file
      )
        notices++;
    }
    if (!pkg.source) {
      await cp(join(repositoryRoot, "LICENSE"), join(destination, "LICENSE"));
      notices++;
    }
    if (!notices && pkg.source?.startsWith("git+")) {
      let parent = dirname(root);
      for (let depth = 0; depth < 3 && !notices; depth++) {
        for (const name of [
          "LICENSE",
          "LICENSE-MIT",
          "LICENSE-APACHE",
          "NOTICE",
        ]) {
          const content = await readFile(join(parent, name)).catch(
            (error: NodeJS.ErrnoException) => {
              if (error.code === "ENOENT") return undefined;
              throw error;
            },
          );
          if (!content) continue;
          await writeFile(join(destination, name), content);
          await writeFile(
            join(destination, name + ".source.txt"),
            pkg.source + "\n",
          );
          if (name !== "NOTICE") notices++;
        }
        parent = dirname(parent);
      }
    }
    if (!notices && pkg.repository) {
      const vcs = await readFile(
        join(root, ".cargo_vcs_info.json"),
        "utf8",
      ).catch(() => undefined);
      const revision = vcs
        ? (JSON.parse(vcs) as { git: { sha1: string } }).git.sha1
        : undefined;
      if (revision && /^[a-f0-9]{40}$/.test(revision)) {
        const repository = /github\.com[/:]([^/]+\/[^/]+?)(?:\.git)?\/?$/.exec(
          pkg.repository,
        )?.[1];
        if (repository) {
          for (const name of [
            "LICENSE",
            "LICENSE-MIT",
            "LICENSE-APACHE",
            "LICENSE.txt",
            "LICENSES/Apache-2.0.txt",
            "LICENSES/MIT.txt",
            "NOTICE",
          ]) {
            const url = `https://raw.githubusercontent.com/${repository}/${revision}/${name}`;
            const notice = await publicFile(url);
            if (notice === undefined) continue;
            const filename = name.replaceAll("/", "_");
            await writeFile(join(destination, filename), notice);
            await writeFile(
              join(destination, filename + ".source.txt"),
              url + "\n",
            );
            if (name !== "NOTICE") notices++;
          }
        }
      }
    }
    if (!notices && pkg.name === "htmlescape" && pkg.version === "0.3.1") {
      // The published crate grants Apache/MIT/MPL alternatives; its old repository is unavailable.
      const source = "https://www.apache.org/licenses/LICENSE-2.0.txt";
      const license = await publicFile(source);
      if (!license) throw new Error("Apache license text unavailable");
      await writeFile(join(destination, "LICENSE-APACHE"), license);
      await writeFile(
        join(destination, "license-choice.txt"),
        "Distributed under the Apache-2.0 option declared by the publisher in the retained Cargo.toml. " +
          "Publisher metadata and README are retained; no additional NOTICE was present in the published crate.\n" +
          source +
          "\n",
      );
      notices++;
    }
    if (!notices) missingNotices.push(pkg.name + "@" + pkg.version);
    cargoComponents.push({
      id: ids.get(id)!,
      name: pkg.name,
      version: pkg.version,
      license: pkg.license?.replaceAll(/\s*\/\s*/g, " OR ") ?? "NOASSERTION",
      source: pkg.source?.startsWith("registry+")
        ? `https://crates.io/api/v1/crates/${pkg.name}/${pkg.version}/download`
        : (pkg.source ?? "NOASSERTION"),
      purpose: buildOnly.has(id)
        ? "OTHER"
        : pkg.name === "nous-kernel"
          ? "APPLICATION"
          : "LIBRARY",
    });
    for (const dep of nodes.get(id)?.deps ?? []) {
      if (!selected.has(dep.pkg)) continue;
      const kinds = new Set(
        dep.dep_kinds
          .filter((kind) => kind.kind !== "dev")
          .map((kind) => kind.kind),
      );
      for (const kind of kinds)
        relationships.push({
          spdxElementId: ids.get(kind === "build" ? dep.pkg : id)!,
          relationshipType:
            kind === "build" ? "BUILD_DEPENDENCY_OF" : "DEPENDS_ON",
          relatedSpdxElement: ids.get(kind === "build" ? id : dep.pkg)!,
        });
    }
  }
  return {
    components: cargoComponents,
    relationships,
    missingNotices,
    root: ids.get(kernel.id)!,
  };
}
