import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));

export function adapterContractId(manifestPath = resolve(here, "command-manifest.json")) {
  const manifest = JSON.parse(readFileSync(manifestPath, "utf8"));
  const adapter = manifest?.adapter;
  if (!Number.isInteger(manifest?.schema_version) || manifest.schema_version < 3
    || typeof adapter?.name !== "string" || !/^[a-z0-9_-]+$/.test(adapter.name)
    || typeof adapter?.version !== "string" || !/^\d+\.\d+\.\d+$/.test(adapter.version)
    || !Number.isInteger(adapter?.protocol_version) || adapter.protocol_version < 1
    || !Array.isArray(adapter?.compatible_versions)
    || !adapter.compatible_versions.includes(adapter.version)) {
    throw new Error("command-manifest.json does not define a valid runtime adapter contract");
  }
  return `${adapter.name}:${adapter.version}:protocol-${adapter.protocol_version}`;
}

export function outputSizeClass(bytes) {
  if (!Number.isInteger(bytes) || bytes < 0) throw new Error("output size must be a non-negative integer");
  if (bytes === 0) return "empty";
  if (bytes <= 4 * 1024) return "small";
  if (bytes <= 64 * 1024) return "medium";
  return "large";
}

export function outputHash(stdout, stderr) {
  const stdoutLength = Buffer.allocUnsafe(8);
  stdoutLength.writeBigUInt64BE(BigInt(stdout.length));
  return createHash("sha256").update(stdoutLength).update(stdout).update(stderr).digest("hex");
}
