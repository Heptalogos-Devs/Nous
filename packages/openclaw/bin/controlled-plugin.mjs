// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

// One controlled-runtime installation record; this grants only the two named native state methods.
import { createHash } from "node:crypto";
import {
  readFileSync,
  readdirSync,
  lstatSync,
  realpathSync,
  mkdirSync,
  writeFileSync,
  unlinkSync,
  existsSync,
} from "node:fs";
import { dirname, join, resolve, relative, sep } from "node:path";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";
/** @typedef {{pluginId:string,packageName:string,version:string,integrity:string,files:Record<string,string>,capabilities:string[],entryPaths:string[]}} ArtifactIdentity */
/** @typedef {ArtifactIdentity & {artifact:string}} ControlledPluginRecord */
/** @typedef {{schemaVersion:number,plugins:ControlledPluginRecord[]}} ControlledPluginRegistry */
/** @typedef {{nousManagedRuntime?:number,dependencies?:Record<string,string>}} RuntimePackage */
/** @typedef {{packages?:Record<string,{resolved?:string,integrity?:string,version?:string,dependencies?:Record<string,string>}>}} RuntimeLock */
/** @typedef {{name?:string,version?:string,nousNativeState?:unknown,openclaw?:{extensions?:unknown}}} PluginPackage */
/** @typedef {NodeJS.ReadableStream & {type:string,path:string,size:number,resume():void}} TarEntry */
/** @typedef {{t(options:{file:string,sync:true,strict:true,onReadEntry(entry:TarEntry):void}):void}} TarReader */
const nativeStateCapabilities = [
  "openChannelIngressQueue",
  "openKeyedStore",
];
const installedLeases = new Set();
/** @param {Uint8Array} bytes */
const hash = (bytes) => createHash("sha256").update(bytes).digest("hex");
/** @param {string} path @returns {unknown} */
const json = (path) =>
  /** @type {unknown} */ (JSON.parse(readFileSync(path, "utf8")));
/** @param {string} path */
const privateFile = (path) => {
  const stat = lstatSync(path);
  if (!stat.isFile() || stat.isSymbolicLink() || stat.mode & 0o077)
    throw Error(
      "Controlled installation record must be a protected regular file",
    );
};
/** @param {string} nativeRoot @param {string} path @returns {ArtifactIdentity} */
export function inspectPluginArtifact(nativeRoot, path) {
  const stat = lstatSync(path);
  if (!stat.isFile() || stat.isSymbolicLink() || stat.size > 64 * 1024 * 1024)
    throw Error("Local plugin artifact is not a bounded regular tarball");
  const integrity =
    "sha512-" +
    createHash("sha512").update(readFileSync(path)).digest("base64");
  const tar = /** @type {TarReader} */ (
    createRequire(join(nativeRoot, "package.json"))("tar")
  );
  /** @type {Record<string,string>} */
  const files = {};
  /** @type {Record<string,unknown>} */
  const metadata = {};
  let total = 0,
    count = 0;
  tar.t({
    file: path,
    sync: true,
    strict: true,
    onReadEntry(entry) {
      if (entry.type === "Directory") {
        entry.resume();
        return;
      }
      if (entry.type !== "File" || !entry.path.startsWith("package/"))
        throw Error("Local plugin tarball contains unsupported entries");
      const name = entry.path.slice(8);
      if (
        !name ||
        name.startsWith("/") ||
        name.includes("\\") ||
        name
          .split("/")
          .some((part) => !part || part === "." || part === "..") ||
        name.startsWith("node_modules/") ||
        files[name]
      )
        throw Error("Local plugin tarball path is invalid");
      if (++count > 10000 || (total += entry.size) > 256 * 1024 * 1024)
        throw Error("Local plugin expansion budget exceeded");
      const digest = createHash("sha256");
      /** @type {Buffer[]} */
      const chunks = [];
      const capture =
        name === "package.json" || name === "openclaw.plugin.json";
      if (capture && entry.size > 1024 * 1024)
        throw Error("Local plugin metadata is oversized");
      entry.on("data", (/** @type {Buffer} */ chunk) => {
        digest.update(chunk);
        if (capture) chunks.push(chunk);
      });
      entry.on("end", () => {
        files[name] = digest.digest("hex");
        if (capture)
          metadata[name] = /** @type {unknown} */ (
            JSON.parse(Buffer.concat(chunks).toString("utf8"))
          );
      });
      entry.resume();
    },
  });
  const pkg = /** @type {PluginPackage | undefined} */ (
      metadata["package.json"]
    ),
    manifest = /** @type {{id?:string} | undefined} */ (
      metadata["openclaw.plugin.json"]
    );
  if (
    !pkg ||
    !manifest ||
    !/^(?:@[a-z0-9][a-z0-9._-]*\/)?[a-z0-9][a-z0-9._-]*$/u.test(
      pkg.name ?? "",
    ) ||
    typeof pkg.name !== "string" ||
    pkg.name.length > 214 ||
    ["node_modules", "favicon.ico"].includes(pkg.name) ||
    typeof manifest.id !== "string" ||
    !/^[a-z0-9][a-z0-9_-]{0,127}$/u.test(manifest.id) ||
    typeof pkg.version !== "string"
  )
    throw Error("Local plugin package identity is invalid");
  if (["openclaw", "@nous-wave/openclaw"].includes(pkg.name))
    throw Error("Core runtime packages cannot be installed as local plugins");
  const capabilities = pkg.nousNativeState;
  if (
    !Array.isArray(capabilities) ||
    !capabilities.length ||
    new Set(capabilities).size !== capabilities.length ||
    capabilities.some(
      (method) =>
        typeof method !== "string" || !nativeStateCapabilities.includes(method),
    )
  )
    throw Error(
      "Local plugin must declare only supported native state capabilities",
    );
  if (
    !Array.isArray(pkg.openclaw?.extensions) ||
    !pkg.openclaw.extensions.length ||
    pkg.openclaw.extensions.some(
      (entryPath) =>
        typeof entryPath !== "string" ||
        !files[entryPath.replace(/^\.\//u, "")],
    )
  )
    throw Error("Local plugin entry must be contained in its artifact");
  return {
    pluginId: manifest.id,
    packageName: pkg.name,
    version: pkg.version,
    integrity,
    files,
    capabilities: /** @type {string[]} */ (capabilities),
    entryPaths: /** @type {string[]} */ (pkg.openclaw.extensions).map((entry) =>
      entry.replace(/^\.\//u, ""),
    ),
  };
}
/** @param {string} prefix @param {ControlledPluginRecord} record */
export function verifyInstalledPlugin(prefix, record) {
  const nativeRoot = join(prefix, "node_modules/openclaw");
  const root = /** @type {RuntimePackage} */ (
    json(join(prefix, "package.json"))
  );
  if (root.nousManagedRuntime !== 1)
    throw Error("Runtime is not owned by the Nous installer");
  if (
    typeof record.artifact !== "string" ||
    !/^artifacts\/local-[a-f0-9]{64}\.tgz$/u.test(record.artifact)
  )
    throw Error("Local plugin artifact location is invalid");
  const artifactPath = join(prefix, record.artifact);
  if (realpathSync(artifactPath) !== resolve(artifactPath))
    throw Error("Local plugin artifact must not be redirected");
  const actual = inspectPluginArtifact(nativeRoot, artifactPath);
  for (const key of /** @type {const} */ ([
    "pluginId",
    "packageName",
    "version",
    "integrity",
  ]))
    if (actual[key] !== record[key])
      throw Error("Local plugin artifact identity changed");
  if (
    JSON.stringify(actual.capabilities) !==
      JSON.stringify(record.capabilities) ||
    JSON.stringify(actual.files) !== JSON.stringify(record.files)
  )
    throw Error("Local plugin installation record differs from its artifact");
  const spec = `file:${record.artifact}`;
  const lock = /** @type {RuntimeLock} */ (
    json(join(prefix, "package-lock.json"))
  );
  const locked = lock.packages?.[`node_modules/${actual.packageName}`];
  if (
    root.dependencies?.[actual.packageName] !== spec ||
    lock.packages?.[""]?.dependencies?.[actual.packageName] !== spec ||
    locked?.resolved !== spec ||
    locked.integrity !== actual.integrity ||
    locked.version !== actual.version
  )
    throw Error("Local plugin is not the recorded npm installation");
  const packageRoot = join(prefix, "node_modules", actual.packageName);
  if (realpathSync(packageRoot) !== resolve(packageRoot))
    throw Error("Local plugin package must not be redirected");
  /** @type {Record<string,string>} */
  const retained = {};
  /** @param {string} directory */
  function visit(directory) {
    for (const name of readdirSync(directory)) {
      if (directory === packageRoot && name === "node_modules") continue;
      const path = join(directory, name),
        stat = lstatSync(path);
      if (stat.isSymbolicLink())
        throw Error("Local plugin installed files must not be redirected");
      if (stat.isDirectory()) visit(path);
      else if (stat.isFile())
        retained[relative(packageRoot, path).split(sep).join("/")] = hash(
          readFileSync(path),
        );
      else throw Error("Local plugin contains unsupported installed files");
    }
  }
  visit(packageRoot);
  if (
    Object.keys(retained).length !== Object.keys(actual.files).length ||
    Object.entries(actual.files).some(
      ([path, digest]) => retained[path] !== digest,
    )
  )
    throw Error("Local plugin installed content differs from its artifact");
  return { ...actual, packageRoot };
}
/** @param {string} prefix */
export function assertNoActivePluginState(prefix) {
  const directory = join(prefix, ".nous-native-state");
  if (!existsSync(directory)) return;
  if (
    !lstatSync(directory).isDirectory() ||
    realpathSync(directory) !== resolve(directory)
  )
    throw Error("Native state lease directory is invalid");
  for (const name of readdirSync(directory)) {
    if (!/^[1-9][0-9]*\.json$/u.test(name))
      throw Error(
        "Unknown native state lease; stop the controlled runtime before updating",
      );
    const path = join(directory, name);
    privateFile(path);
    const lease = /** @type {{pid:number}} */ (json(path));
    if (lease.pid !== Number(name.slice(0, -5)))
      throw Error("Invalid native state lease");
    try {
      process.kill(lease.pid, 0);
    } catch (error) {
      if (error instanceof Error && "code" in error && error.code === "ESRCH") {
        unlinkSync(path);
        continue;
      }
      throw Error("Cannot confirm that the controlled runtime is stopped", {
        cause: error,
      });
    }
    throw Error(
      "Stop the controlled runtime before updating its local plugins",
    );
  }
}
/** @param {{pluginId:string,pluginSource?:string,methodName:string,nativeRoot?:string}} options */
export function allowControlledPluginState({
  pluginId,
  pluginSource,
  methodName,
  nativeRoot,
}) {
  if (
    !nativeStateCapabilities.includes(methodName) ||
    typeof pluginSource !== "string"
  )
    return false;
  try {
    const native = realpathSync(
      nativeRoot ?? resolve(dirname(fileURLToPath(import.meta.url)), ".."),
    );
    const prefix = dirname(dirname(native));
    if (realpathSync(join(prefix, "node_modules/openclaw")) !== native)
      return false;
    const installLock = join(prefix, ".nous-install-lock");
    if (existsSync(installLock)) return false;
    const path = join(prefix, "nous-local-plugins.json");
    privateFile(path);
    const registry = /** @type {ControlledPluginRegistry} */ (json(path));
    if (registry.schemaVersion !== 1 || !Array.isArray(registry.plugins))
      return false;
    const record = registry.plugins.find((item) => item.pluginId === pluginId);
    if (!record || !record.capabilities.includes(methodName)) return false;
    const actual = verifyInstalledPlugin(prefix, record);
    const source = realpathSync(pluginSource);
    if (
      !source.startsWith(actual.packageRoot + sep) ||
      !actual.entryPaths.includes(
        relative(actual.packageRoot, source).split(sep).join("/"),
      )
    )
      return false;
    const directory = join(prefix, ".nous-native-state");
    mkdirSync(directory, { recursive: true, mode: 0o700 });
    if (realpathSync(directory) !== resolve(directory)) return false;
    const lease = join(directory, `${process.pid}.json`);
    writeFileSync(
      lease,
      JSON.stringify({ pid: process.pid, runtimeRoot: prefix }) + "\n",
      { mode: 0o600 },
    );
    if (existsSync(installLock)) {
      if (!installedLeases.has(lease)) unlinkSync(lease);
      return false;
    }
    if (!installedLeases.has(lease)) {
      installedLeases.add(lease);
      process.once("exit", () => {
        try {
          unlinkSync(lease);
        } catch {}
      });
    }
    return true;
  } catch {
    return false;
  }
}
