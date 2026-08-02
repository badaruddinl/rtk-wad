import { createHash } from "node:crypto";
import { basename, join, resolve } from "node:path";

export function isolatedBenchmarkState(output, localAppData = process.env.LOCALAPPDATA) {
  if (!localAppData) {
    throw new Error("LOCALAPPDATA is required for ACL-protected benchmark state");
  }

  const resolvedOutput = resolve(output);
  const artifactName = basename(resolvedOutput, ".json").replace(/[^a-zA-Z0-9_.-]/g, "-");
  const artifactId = createHash("sha256").update(resolvedOutput).digest("hex").slice(0, 16);
  return join(resolve(localAppData), "xuva", "benchmark-state", `${artifactName}-${artifactId}`);
}
