import { createHash } from "node:crypto";
import { spawn, spawnSync } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { performance } from "node:perf_hooks";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));

function option(name) {
  const index = process.argv.indexOf(name);
  if (index === -1 || !process.argv[index + 1]) {
    throw new Error(`Missing required option ${name}`);
  }
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
if (!Number.isInteger(settings.rounds) || settings.rounds < 1) {
  throw new Error("--rounds must be a positive integer");
}

const rawGit = process.env.RTK_WAD_BENCH_GIT || "git.exe";
const rawRg = process.env.RTK_WAD_BENCH_RG || "rg.exe";
const workloads = [
  {
    id: "git-status",
    raw: [rawGit, ["status", "--short", "--branch"]],
    rtk: ["git", "status", "--short", "--branch"],
  },
  {
    id: "git-log-100",
    raw: [rawGit, ["log", "--oneline", "-100"]],
    rtk: ["git", "log", "--oneline", "-100"],
  },
  {
    id: "rg-focused",
    raw: [rawRg, ["-n", "graphVersion", "src", "test", "docs"]],
    rtk: ["rg", "-n", "graphVersion", "src", "test", "docs"],
  },
  {
    id: "rg-broad",
    raw: [rawRg, ["-n", "function|const|class|require|module", "src", "test", "docs"]],
    rtk: ["rg", "-n", "function|const|class|require|module", "src", "test", "docs"],
  },
];

function execute(file, args, extraEnvironment = {}) {
  return new Promise((resolveExecution, reject) => {
    const started = performance.now();
    const child = spawn(file, args, {
      cwd: settings.repo,
      env: { ...process.env, ...extraEnvironment },
      shell: false,
      windowsHide: true,
    });
    const stdout = [];
    const stderr = [];
    child.stdout.on("data", (chunk) => stdout.push(chunk));
    child.stderr.on("data", (chunk) => stderr.push(chunk));
    child.on("error", reject);
    child.on("close", (exitCode, signal) => {
      resolveExecution({
        elapsed_ms: Number((performance.now() - started).toFixed(3)),
        exit_code: exitCode,
        signal,
        stdout: Buffer.concat(stdout),
        stderr: Buffer.concat(stderr),
      });
    });
  });
}

function exactTokens(buffer) {
  const result = spawnSync(settings.python, [resolve(here, "token-count.py")], {
    input: buffer,
    encoding: "utf8",
    maxBuffer: 32 * 1024 * 1024,
  });
  if (result.status !== 0) {
    throw new Error(`o200k_base counter failed: ${result.stderr}`);
  }
  return Number(result.stdout.trim());
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

const variants = (workload) => ({
  raw: { file: workload.raw[0], args: workload.raw[1], environment: {} },
  native_rtk: { file: settings.nativeRtk, args: workload.rtk, environment: {} },
  rtk_wad_auto: {
    file: settings.wad,
    args: workload.rtk,
    environment: {
      RTK_WAD_NATIVE_RTK_PATH: settings.nativeRtk,
      LOCALAPPDATA: resolve(dirname(settings.output), "wad-local-app-data"),
    },
  },
});

const allSamples = [];
for (const workload of workloads) {
  const entries = Object.entries(variants(workload));
  for (const [, variant] of entries) {
    await execute(variant.file, variant.args, variant.environment);
  }
  for (let round = 0; round < settings.rounds; round += 1) {
    const rotated = entries.slice(round % entries.length).concat(entries.slice(0, round % entries.length));
    for (const [name, variant] of rotated) {
      allSamples.push({ workload: workload.id, variant: name, round: round + 1, ...(await execute(variant.file, variant.args, variant.environment)) });
    }
  }
}

const summaries = workloads.map((workload) => {
  const perVariant = {};
  const rawSamples = allSamples.filter((sample) => sample.workload === workload.id && sample.variant === "raw");
  const rawTokens = exactTokens(Buffer.concat([rawSamples[0].stdout, rawSamples[0].stderr]));
  for (const name of Object.keys(variants(workload))) {
    const samples = allSamples.filter((sample) => sample.workload === workload.id && sample.variant === name);
    perVariant[name] = summarize(samples, rawTokens);
  }
  return { workload: workload.id, variants: perVariant };
});

mkdirSync(dirname(settings.output), { recursive: true });
const policyKey = {
  "git-status": "git:status",
  "git-log-100": "git:log",
  "rg-focused": "rg",
  "rg-broad": "rg",
};
const policyEvidence = Object.values(summaries.reduce((grouped, { workload, variants }) => {
  const key = policyKey[workload];
  const evidence = {
    key,
    raw_median_ms: variants.raw.median_ms,
    candidate_median_ms: variants.rtk_wad_auto.median_ms,
    token_savings_percent: variants.native_rtk.token_savings_percent,
    sample_count: variants.raw.runs,
  };
  const previous = grouped[key];
  if (!previous) return { ...grouped, [key]: evidence };
  const total = previous.sample_count + evidence.sample_count;
  return {
    ...grouped,
    [key]: {
      key,
      raw_median_ms: ((previous.raw_median_ms * previous.sample_count) + (evidence.raw_median_ms * evidence.sample_count)) / total,
      candidate_median_ms: ((previous.candidate_median_ms * previous.sample_count) + (evidence.candidate_median_ms * evidence.sample_count)) / total,
      token_savings_percent: ((previous.token_savings_percent * previous.sample_count) + (evidence.token_savings_percent * evidence.sample_count)) / total,
      sample_count: total,
    },
  };
}, {}));
writeFileSync(settings.output, JSON.stringify({
  schema_version: 1,
  protocol: "three-way-core-v1",
  tokenizer: "o200k_base",
  rounds: settings.rounds,
  corpus: settings.repo,
  native_rtk: settings.nativeRtk,
  rtk_wad: settings.wad,
  summaries,
  samples: allSamples.map(({ stdout, stderr, ...sample }) => ({
    ...sample,
    stdout_sha256: createHash("sha256").update(stdout).digest("hex"),
    stderr_sha256: createHash("sha256").update(stderr).digest("hex"),
    stdout_bytes: stdout.length,
    stderr_bytes: stderr.length,
  })),
}, null, 2));
const policyOutput = settings.output.replace(/\.json$/i, ".route-policy.json");
writeFileSync(policyOutput, JSON.stringify({ schema_version: 1, evidence: policyEvidence }, null, 2));

console.log(`Wrote ${settings.output}`);
console.log(`Wrote ${policyOutput}`);
if (settings.installPolicy) {
  const imported = await execute(settings.wad, ["policy", "import", policyOutput]);
  if (imported.exit_code !== 0) throw new Error(`Policy import failed: ${imported.stderr}`);
  console.log("Installed the generated local route policy.");
}
