// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { createHash } from "node:crypto";
import { createReadStream } from "node:fs";
import { mkdir, readFile, readdir, writeFile } from "node:fs/promises";
import { join, sep } from "node:path";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import type { Component, Relationship, loadNotices } from "./notices.js";
import { preparedPublicFile } from "./notices.js";
const execute = promisify(execFile);
export async function writeManifest(
  repo: string,
  output: string,
  notices: Awaited<ReturnType<typeof loadNotices>>,
  components: {
    packs: {
      component: string;
      version: string;
      platform: string;
      arch: string;
      sha256: string;
      manifest_sha256: string;
      archive: string;
    }[];
  },
) {
  const runtime = join(output, "runtime");
  const { inventory, kernelClosure } = notices;
  const runtimeComponents: Component[] = [];
  const nativeRelationships: Relationship[] = [];
  for (const pack of components.packs) {
    const runtimeManifest = JSON.parse(
      await readFile(join(runtime, pack.component, "manifest.json"), "utf8"),
    ) as { license: string; source: string };
    runtimeComponents.push({
      id: `SPDXRef-runtime-${pack.component}`,
      name: pack.component,
      version: pack.version,
      license: runtimeManifest.license,
      source: runtimeManifest.source,
      purpose: "APPLICATION",
      notice: `runtime/${pack.component}`,
    });
  }
  const nodeVersions = JSON.parse(
    (
      await execute(
        join(runtime, "node/node.exe"),
        ["-p", "JSON.stringify(process.versions)"],
        { windowsHide: true },
      )
    ).stdout,
  ) as Record<string, string>;
  const nodeLicensePath = "licenses/node-embedded/LICENSE";
  const nodeLicense = await preparedPublicFile(
    repo,
    `https://raw.githubusercontent.com/nodejs/node/v${nodeVersions.node}/LICENSE`,
  );
  await mkdir(join(output, "licenses/node-embedded"), { recursive: true });
  await writeFile(join(output, nodeLicensePath), nodeLicense);
  const nodeDependencies = [
    ["acorn", "Acorn", "deps/acorn", "MIT"],
    ["ada", "ada", "deps/ada", "MIT"],
    ["amaro", "amaro", "deps/amaro", "MIT"],
    ["ares", "c-ares", "deps/cares", "MIT"],
    ["brotli", "brotli", "deps/brotli", "MIT"],
    ["icu", "ICU", "deps/icu-small", "Unicode-3.0"],
    ["llhttp", "llhttp", "deps/llhttp", "MIT"],
    ["merve", "merve", "deps/merve", "MIT"],
    ["nghttp2", "nghttp2", "deps/nghttp2", "MIT"],
    ["openssl", "OpenSSL", "deps/openssl", "Apache-2.0"],
    ["simdjson", "simdjson", "deps/simdjson", "Apache-2.0"],
    ["simdutf", "simdutf", "deps/v8/third_party/simdutf", "MIT"],
    ["undici", "undici", "deps/undici", "MIT"],
    ["uv", "libuv", "deps/uv", "MIT"],
    ["uvwasi", "uvwasi", "deps/uvwasi", "MIT"],
    ["v8", "V8", "deps/v8", "BSD-3-Clause"],
    ["zlib", "zlib", "deps/zlib", "Zlib"],
    ["zstd", "zstd", "deps/zstd", "BSD-3-Clause"],
  ] as const;
  for (const [key, name, sourcePath, license] of nodeDependencies) {
    if (
      !nodeVersions[key] ||
      !nodeLicense.includes(`- ${name}, located at ${sourcePath},`)
    )
      throw new Error(
        `Node embedded dependency version/notice missing: ${name}`,
      );
    const id = `SPDXRef-node-embedded-${key}`;
    runtimeComponents.push({
      id,
      name,
      version: nodeVersions[key],
      license,
      source: `https://github.com/nodejs/node/tree/v${nodeVersions.node}/${sourcePath}`,
      purpose: "LIBRARY",
      notice: nodeLicensePath,
    });
    nativeRelationships.push({
      spdxElementId: "SPDXRef-runtime-node",
      relationshipType: "CONTAINS",
      relatedSpdxElement: id,
    });
  }
  for (const [key, sourcePath, license] of [
    ["nbytes", "deps/nbytes/LICENSE", "MIT"],
    ["ncrypto", "LICENSE", "MIT"],
    ["sqlite", "deps/sqlite/sqlite3.h", "LicenseRef-SQLite-Public-Domain"],
  ] as const) {
    const source = `https://raw.githubusercontent.com/nodejs/node/v${nodeVersions.node}/${sourcePath}`;
    const content = await preparedPublicFile(repo, source);
    if (!nodeVersions[key] || !content)
      throw new Error(`Node notice unavailable: ${key}`);
    const notice = `licenses/node-embedded/${key}.txt`;
    await writeFile(
      join(output, notice),
      key === "sqlite" ? content.split("*/", 1)[0] + "*/\n" : content,
    );
    const id = `SPDXRef-node-embedded-${key}`;
    runtimeComponents.push({
      id,
      name: key,
      version: nodeVersions[key],
      license,
      source,
      purpose: "LIBRARY",
      notice,
    });
    nativeRelationships.push({
      spdxElementId: "SPDXRef-runtime-node",
      relationshipType: "CONTAINS",
      relatedSpdxElement: id,
    });
  }
  const rustVersion = await readFile(
    join(output, "licenses/native/rust-source.txt"),
    "utf8",
  );
  const rustRelease = /^release: (.+)$/m.exec(rustVersion)?.[1];
  const rustCommit = /^commit-hash: (.+)$/m.exec(rustVersion)?.[1];
  if (!rustRelease || !rustCommit)
    throw new Error("Rust library provenance unavailable");
  for (const component of [
    {
      id: "SPDXRef-native-llvm-runtime",
      name: "LLVM runtime (libc++/compiler-rt/libunwind)",
      version: "20260922",
      license: "Apache-2.0 WITH LLVM-exception",
      source: "https://github.com/mstorsjo/llvm-mingw/releases/tag/20260922",
      purpose: "LIBRARY",
      notice: "licenses/native/LLVM-LICENSE.txt",
    },
    {
      id: "SPDXRef-native-mingw-runtime",
      name: "MinGW-w64 runtime",
      version: "57b595039040eaa15bece85b7cc71d952281b269",
      license: "LicenseRef-MinGW-Runtime",
      source:
        "https://github.com/mingw-w64/mingw-w64/tree/57b595039040eaa15bece85b7cc71d952281b269",
      purpose: "LIBRARY",
      notice: "licenses/native/mingw-runtime",
    },
    {
      id: "SPDXRef-native-rust-library",
      name: "Rust standard library",
      version: rustRelease,
      license: "MIT OR Apache-2.0",
      source: `https://github.com/rust-lang/rust/tree/${rustCommit}/library`,
      purpose: "LIBRARY",
      notice: "licenses/native/Rust-library-COPYRIGHT.html",
    },
  ] satisfies Component[]) {
    runtimeComponents.push(component);
    nativeRelationships.push({
      spdxElementId: kernelClosure.root,
      relationshipType: "DEPENDS_ON",
      relatedSpdxElement: component.id,
    });
  }
  await mkdir(join(output, "manifest"), { recursive: true });
  await writeFile(
    join(output, "manifest/components.json"),
    JSON.stringify(
      {
        application: inventory,
        kernel: kernelClosure.components,
        missing_cargo_notices: kernelClosure.missingNotices,
        runtimes: components.packs,
      },
      null,
      2,
    ) + "\n",
  );
  await writeFile(
    join(output, "licenses/THIRD_PARTY_NOTICES.md"),
    "# Third-party components\n\nApplication dependencies and runtime inventories are in manifest/components.json. Each runtime/<component>/manifest.json records the supplied pack's source, composite license and file hashes; its notices and build references are retained with that pack. Kernel toolchain notices are in licenses/native; Node embedded notices are bound to the shipped executable's version. Nous Wave-owned code is licensed under Apache-2.0.\n",
  );
  await writeFile(
    join(output, "manifest/sbom.spdx.json"),
    JSON.stringify(
      {
        spdxVersion: "SPDX-2.3",
        dataLicense: "CC0-1.0",
        SPDXID: "SPDXRef-DOCUMENT",
        name: "Nous-Wave-runtime-bundle",
        documentNamespace:
          "https://nous-wave.invalid/spdx/" + crypto.randomUUID(),
        creationInfo: {
          creators: ["Tool: Nous-Wave-assembler"],
          created: new Date().toISOString(),
        },
        hasExtractedLicensingInfos: [
          {
            licenseId: "LicenseRef-MinGW-Runtime",
            name: "MinGW-w64 runtime composite notices",
            extractedText: await readFile(
              join(
                output,
                "licenses/native/mingw-runtime/COPYING.MinGW-w64-runtime.txt",
              ),
              "utf8",
            ),
            seeAlsos: [
              "https://github.com/mingw-w64/mingw-w64/tree/57b595039040eaa15bece85b7cc71d952281b269",
            ],
          },
          {
            licenseId: "LicenseRef-SQLite-Public-Domain",
            name: "SQLite public domain dedication",
            extractedText:
              "The author disclaims copyright to this source code. See the retained SQLite header for its full dedication.",
            seeAlsos: [
              `https://raw.githubusercontent.com/nodejs/node/v${nodeVersions.node}/deps/sqlite/sqlite3.h`,
            ],
          },
        ],
        packages: [
          ...inventory.map((pkg, index) =>
            spdxPackage({
              id: `SPDXRef-npm-${index}`,
              ...pkg,
              source: `https://registry.npmjs.org/${pkg.name}/-/${pkg.name.split("/").at(-1)}-${pkg.version}.tgz`,
              purpose: "LIBRARY",
            }),
          ),
          ...kernelClosure.components.map(spdxPackage),
          ...runtimeComponents.map(spdxPackage),
        ],
        relationships: [
          ...kernelClosure.relationships,
          ...nativeRelationships,
          ...[
            kernelClosure.root,
            ...runtimeComponents.map((component) => component.id),
          ].map((id) => ({
            spdxElementId: "SPDXRef-DOCUMENT",
            relationshipType: "DESCRIBES",
            relatedSpdxElement: id,
          })),
        ],
      },
      null,
      2,
    ) + "\n",
  );
  const head = (
    await execute("git", ["rev-parse", "HEAD"], { cwd: repo })
  ).stdout.trim();
  const sourceFiles = (
    await execute(
      "git",
      ["ls-files", "--cached", "--others", "--exclude-standard", "-z"],
      { cwd: repo },
    )
  ).stdout
    .split("\0")
    .filter(Boolean)
    .sort();
  const sourceHash = createHash("sha256");
  for (const path of sourceFiles) {
    const contents = await readFile(join(repo, path)).catch(
      (error: NodeJS.ErrnoException) => {
        if (error.code === "ENOENT") return undefined;
        throw error;
      },
    );
    if (contents === undefined) continue;
    sourceHash
      .update(path)
      .update("\0")
      .update(createHash("sha256").update(contents).digest())
      .update("\0");
  }
  const sourceInputDigest = sourceHash.digest("hex");
  const sourceDirty = Boolean(
    (
      await execute("git", ["status", "--porcelain"], { cwd: repo })
    ).stdout.trim(),
  );
  await writeFile(
    join(output, "manifest/release.json"),
    JSON.stringify(
      {
        name: "Nous Wave",
        version: "0.1.0",
        platform: process.platform,
        arch: process.arch,
        source_head: head,
        source_input_sha256: sourceInputDigest,
        source_dirty: sourceDirty,
        source_tree: "working-tree",
        launcher: "bin/nous.cmd",
        application_license: "Apache-2.0",
      },
      null,
      2,
    ) + "\n",
  );
  const checksums: { path: string; sha256: string }[] = [];
  async function collect(directory: string) {
    for (const file of await readdir(directory, { withFileTypes: true })) {
      const path = join(directory, file.name);
      if (file.isDirectory()) await collect(path);
      else {
        const hash = createHash("sha256");
        for await (const bytes of createReadStream(path))
          hash.update(bytes as Buffer);
        checksums.push({
          path: path
            .slice(output.length + 1)
            .split(sep)
            .join("/"),
          sha256: hash.digest("hex"),
        });
      }
    }
  }
  await collect(output);
  await writeFile(
    join(output, "manifest/checksums.json"),
    JSON.stringify(checksums, null, 2) + "\n",
  );
}
function spdxPackage(component: Component) {
  return {
    SPDXID: component.id,
    name: component.name,
    versionInfo: component.version,
    downloadLocation: component.source,
    filesAnalyzed: false,
    licenseDeclared: component.license,
    licenseConcluded: "NOASSERTION",
    copyrightText: "NOASSERTION",
    primaryPackagePurpose: component.purpose,
    ...(component.notice
      ? {
          attributionTexts: [
            `License and attribution retained at ${component.notice}`,
          ],
        }
      : {}),
  };
}
