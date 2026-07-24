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

const settings = {
  repo: resolve(option("--repo")),
  nativeRtk: resolve(option("--native-rtk")),
  wad: resolve(option("--wad")),
  python: resolve(option("--python")),
  output: resolve(option("--output")),
  rounds: Number(process.argv.includes("--rounds") ? option("--rounds") : 10),
  installPolicy: process.argv.includes("--install-policy"),
};
if (!Number.isInteger(settings.rounds) || settings.rounds < 5) {
  throw new Error("--rounds must be an integer of at least 5 for route-policy evidence");
}

const rawNpm = process.env.RTK_WAD_BENCH_NPM || "npm.cmd";
const variants = {
  // Node cannot spawn a Windows .cmd file with shell:false. cmd.exe is the
  // normal Windows launcher for this fixed, argument-free npm list operation.
  // Keep its command and argument distinct so Node supplies Windows quoting.
  raw: { file: process.env.ComSpec || "cmd.exe", args: ["/d", "/s", "/c", rawNpm, "run"], environment: {} },
  native_rtk: { file: settings.nativeRtk, args: ["npm", "run"], environment: {} },
  rtk_wad_auto: {
    file: settings.wad,
    args: ["npm", "run"],
    environment: {
      RTK_WAD_NATIVE_RTK_PATH: settings.nativeRtk,
      LOCALAPPDATA: resolve(dirname(settings.output), "wad-local-app-data"),
    },
  },
};

function execute(variant) {
  return new Promise((resolveExecution, reject) => {
    const started = performance.now();
    const child = spawn(variant.file, variant.args, {
      cwd: settings.repo,
      env: { ...process.env, ...variant.environment },
      shell: false,
      windowsHide: true,
    });
    const stdout = [];
    const stderr = [];
    child.stdout.on("data", (chunk) => stdout.push(chunk));
    child.stderr.on("data", (chunk) => stderr.push(chunk));
    child.on("error", reject);
    child.on("close", (exitCode, signal) => resolveExecution({
      elapsed_ms: Number((performance.now() - started).toFixed(3)),
      exit_code: exitCode,
      signal,
      stdout: Buffer.concat(stdout),
      stderr: Buffer.concat(stderr),
    }));
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
    output_bytes: output.length,
    o200k_tokens: tokens,
    tokens_saved_vs_raw: rawTokens - tokens,
    token_savings_percent: rawTokens === 0 ? 0 : Number((((rawTokens - tokens) / rawTokens) * 100).toFixed(1)),
    output_hashes: [...new Set(samples.map((sample) => createHash("sha256").update(sample.stdout).update(sample.stderr).digest("hex")))],
  };
}

for (const variant of Object.values(variants)) await execute(variant);
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
const allSuccessful = samples.every((sample) => sample.exit_code === 0 && sample.signal === null);
mkdirSync(dirname(settings.output), { recursive: true });
writeFileSync(settings.output, JSON.stringify({
  schema_version: 1,
  protocol: "three-way-npm-run-list-v1",
  tokenizer: "o200k_base",
  rounds: settings.rounds,
  corpus: settings.repo,
  native_rtk: settings.nativeRtk,
  rtk_wad: settings.wad,
  all_successful: allSuccessful,
  summary,
  samples: samples.map(({ stdout, stderr, ...sample }) => ({
    ...sample,
    stdout_sha256: createHash("sha256").update(stdout).digest("hex"),
    stderr_sha256: createHash("sha256").update(stderr).digest("hex"),
    stdout_bytes: stdout.length,
    stderr_bytes: stderr.length,
  })),
}, null, 2));
console.log(`Wrote ${settings.output}`);
if (!allSuccessful) throw new Error("One or more measured variants failed; no route policy was emitted");

const policyPath = settings.output.replace(/\.json$/i, ".route-policy.json");
const policy = {
  schema_version: 1,
  evidence: [{
    key: "npm:run-list",
    raw_median_ms: summary.raw.median_ms,
    candidate_median_ms: summary.rtk_wad_auto.median_ms,
    token_savings_percent: summary.native_rtk.token_savings_percent,
    sample_count: summary.raw.runs,
  }],
};
writeFileSync(policyPath, JSON.stringify(policy, null, 2));
console.log(`Wrote ${policyPath}`);
if (settings.installPolicy) {
  const imported = await execute({ file: settings.wad, args: ["policy", "import", policyPath], environment: variants.rtk_wad_auto.environment });
  if (imported.exit_code !== 0) throw new Error(`Policy import failed: ${imported.stderr}`);
  console.log("Installed the generated local route policy.");
}
