import { createHash } from "node:crypto";
import { spawn, spawnSync } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { performance } from "node:perf_hooks";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));

function option(name) {
  const index = process.argv.indexOf(name);
  if (index === -1 || !process.argv[index + 1]) throw new Error(`Missing required option ${name}`);
  return process.argv[index + 1];
}

const delimiter = process.argv.indexOf("--");
if (delimiter === -1 || delimiter === process.argv.length - 1) {
  throw new Error("Pass the measured tool arguments after --");
}
const settings = {
  tool: option("--tool"),
  repo: resolve(option("--repo")),
  nativeRtk: resolve(option("--native-rtk")),
  wad: resolve(option("--wad")),
  python: resolve(option("--python")),
  output: resolve(option("--output")),
  rounds: Number(process.argv.includes("--rounds") ? option("--rounds") : 5),
  timeoutMs: Number(process.argv.includes("--timeout-ms") ? option("--timeout-ms") : 60000),
  normalizer: process.argv.includes("--normalizer") ? option("--normalizer") : "none",
  skipWarmup: process.argv.includes("--skip-warmup"),
  policyKey: process.argv.includes("--policy-key") ? option("--policy-key") : null,
  withoutNative: process.argv.includes("--without-native"),
  args: process.argv.slice(delimiter + 1),
};
if (!Number.isInteger(settings.rounds) || settings.rounds < 5) {
  throw new Error("--rounds must be an integer of at least 5");
}
if (!Number.isInteger(settings.timeoutMs) || settings.timeoutMs < 1000) {
  throw new Error("--timeout-ms must be an integer of at least 1000");
}
if (!["none", "dart-format-duration", "flutter-analysis-duration"].includes(settings.normalizer)) {
  throw new Error("--normalizer must be none, dart-format-duration, or flutter-analysis-duration");
}
if (settings.policyKey && settings.withoutNative) {
  throw new Error("--policy-key requires a stock native RTK comparison");
}

const rawExecutable = process.env.RTK_WAD_BENCH_TOOL || ({
  dart: "dart.bat",
  flutter: "flutter.bat",
  go: "go.exe",
  dotnet: "dotnet.exe",
}[settings.tool] || settings.tool);

function rawVariant() {
  // Node cannot directly spawn .cmd/.bat launchers with shell:false.  Let the
  // Windows command processor launch only the fixed executable and its argv;
  // individual arguments remain separate items so Node performs their quoting.
  if (/\.(cmd|bat)$/i.test(rawExecutable)) {
    return { file: process.env.ComSpec || "cmd.exe", args: ["/d", "/s", "/c", rawExecutable, ...settings.args] };
  }
  return { file: rawExecutable, args: settings.args };
}

const variants = {
  raw: { ...rawVariant(), environment: {} },
  rtk_wad_auto: {
    file: settings.wad,
    args: [settings.tool, ...settings.args],
    environment: { RTK_WAD_STATE_DIR: resolve(dirname(settings.output), "wad-state") },
  },
};
if (!settings.withoutNative) variants.native_rtk = { file: settings.nativeRtk, args: [settings.tool, ...settings.args], environment: {} };
if (settings.policyKey) {
  variants.rtk_wad_candidate = {
    file: settings.wad,
    args: ["--route", "native-rtk", settings.tool, ...settings.args],
    environment: { RTK_WAD_STATE_DIR: resolve(dirname(settings.output), "wad-state") },
  };
}

function execute(variant) {
  return new Promise((resolveExecution, reject) => {
    const started = performance.now();
    let timedOut = false;
    const child = spawn(variant.file, variant.args, {
      cwd: settings.repo,
      env: {
        ...process.env,
        RTK_WAD_NATIVE_RTK_PATH: settings.nativeRtk,
        ...variant.environment,
      },
      shell: false,
      windowsHide: true,
    });
    const stdout = [];
    const stderr = [];
    child.stdout.on("data", (chunk) => stdout.push(chunk));
    child.stderr.on("data", (chunk) => stderr.push(chunk));
    const timeout = setTimeout(() => {
      timedOut = true;
      // A Windows launcher can outlive its immediate .cmd or adapter parent.
      // Terminate only this benchmark process tree, never a name-based set.
      spawn("taskkill.exe", ["/pid", String(child.pid), "/t", "/f"], { windowsHide: true });
    }, settings.timeoutMs);
    child.on("error", reject);
    child.on("close", (exitCode, signal) => {
      clearTimeout(timeout);
      resolveExecution({
        elapsed_ms: Number((performance.now() - started).toFixed(3)),
        exit_code: exitCode,
        signal,
        timed_out: timedOut,
        stdout: Buffer.concat(stdout),
        stderr: Buffer.concat(stderr),
      });
    });
  });
}

function exactTokens(buffer) {
  const counted = spawnSync(settings.python, [resolve(here, "token-count.py")], {
    input: buffer,
    encoding: "utf8",
    maxBuffer: 32 * 1024 * 1024,
  });
  if (counted.status !== 0) throw new Error(`o200k_base counter failed: ${counted.stderr}`);
  return Number(counted.stdout.trim());
}

function normalizedOutput(stdout, stderr) {
  const output = Buffer.concat([stdout, stderr]).toString("utf8");
  if (settings.normalizer === "dart-format-duration") {
    return output.replace(/in \d+(?:\.\d+)? seconds\./g, "in <duration> seconds.");
  }
  if (settings.normalizer === "flutter-analysis-duration") {
    return output.replace(/\(ran in [^)]+\)/g, "(ran in <duration>)");
  }
  return output;
}

function percentile(sorted, fraction) {
  return sorted[Math.min(sorted.length - 1, Math.ceil(sorted.length * fraction) - 1)];
}

function summarize(samples, rawTokens) {
  const elapsed = samples.map((sample) => sample.elapsed_ms).sort((a, b) => a - b);
  const output = Buffer.concat([samples[0].stdout, samples[0].stderr]);
  const tokens = exactTokens(output);
  return {
    runs: samples.length,
    median_ms: percentile(elapsed, 0.5),
    p95_ms: percentile(elapsed, 0.95),
    min_ms: elapsed[0],
    max_ms: elapsed.at(-1),
    exit_codes: [...new Set(samples.map((sample) => sample.exit_code))],
    signals: [...new Set(samples.map((sample) => sample.signal))],
    timed_out: samples.some((sample) => sample.timed_out),
    output_bytes: output.length,
    o200k_tokens: tokens,
    tokens_saved_vs_raw: rawTokens - tokens,
    token_savings_percent: rawTokens === 0 ? 0 : Number((((rawTokens - tokens) / rawTokens) * 100).toFixed(1)),
    output_hashes: [...new Set(samples.map((sample) => createHash("sha256").update(sample.stdout).update(sample.stderr).digest("hex")))],
    semantic_output_hashes: [...new Set(samples.map((sample) => createHash("sha256").update(normalizedOutput(sample.stdout, sample.stderr)).digest("hex")))],
  };
}

if (!settings.skipWarmup) for (const variant of Object.values(variants)) await execute(variant);
const samples = [];
const entries = Object.entries(variants);
for (let round = 0; round < settings.rounds; round += 1) {
  const rotated = entries.slice(round % entries.length).concat(entries.slice(0, round % entries.length));
  for (const [name, variant] of rotated) samples.push({ variant: name, round: round + 1, ...(await execute(variant)) });
}

const rawSamples = samples.filter((sample) => sample.variant === "raw");
const rawTokens = exactTokens(Buffer.concat([rawSamples[0].stdout, rawSamples[0].stderr]));
const summary = Object.fromEntries(Object.keys(variants).map((name) => [
  name,
  summarize(samples.filter((sample) => sample.variant === name), rawTokens),
]));
const allSuccessful = samples.every((sample) => sample.exit_code === 0 && sample.signal === null && !sample.timed_out);
const rawWadEquivalent = summary.raw.semantic_output_hashes.length === 1
  && summary.rtk_wad_auto.semantic_output_hashes.length === 1
  && summary.raw.semantic_output_hashes[0] === summary.rtk_wad_auto.semantic_output_hashes[0];
mkdirSync(dirname(settings.output), { recursive: true });
writeFileSync(settings.output, JSON.stringify({
  schema_version: 1,
  protocol: settings.policyKey
    ? "four-way-toolchain-policy-v1"
    : (settings.withoutNative ? "two-way-toolchain-v1" : "three-way-toolchain-v1"),
  tokenizer: "o200k_base",
  tool: settings.tool,
  args: settings.args,
  policy_key: settings.policyKey,
  output_normalizer: settings.normalizer,
  rounds: settings.rounds,
  timeout_ms: settings.timeoutMs,
  warmup: settings.skipWarmup ? "external" : "runner",
  corpus: settings.repo,
  native_rtk: settings.withoutNative ? null : settings.nativeRtk,
  rtk_wad: settings.wad,
  all_successful: allSuccessful,
  raw_wad_output_equivalent: rawWadEquivalent,
  summary,
  samples: samples.map(({ stdout, stderr, ...sample }) => ({
    ...sample,
    stdout_sha256: createHash("sha256").update(stdout).digest("hex"),
    stderr_sha256: createHash("sha256").update(stderr).digest("hex"),
    semantic_output_sha256: createHash("sha256").update(normalizedOutput(stdout, stderr)).digest("hex"),
    stdout_bytes: stdout.length,
    stderr_bytes: stderr.length,
  })),
}, null, 2));
console.log(`Wrote ${settings.output}`);
if (!allSuccessful) throw new Error("One or more measured variants failed");
if (!rawWadEquivalent) throw new Error("Raw and WAD semantic outputs differ; do not treat the fallback as transparent");
if (settings.policyKey) {
  const policyPath = settings.output.replace(/\.json$/i, ".route-policy.json");
  const policy = {
    schema_version: 1,
    evidence: [{
      key: settings.policyKey,
      raw_median_ms: summary.raw.median_ms,
      candidate_median_ms: summary.rtk_wad_candidate.median_ms,
      token_savings_percent: summary.rtk_wad_candidate.token_savings_percent,
      sample_count: summary.raw.runs,
    }],
  };
  writeFileSync(policyPath, JSON.stringify(policy, null, 2));
  console.log(`Wrote ${policyPath}`);
}
