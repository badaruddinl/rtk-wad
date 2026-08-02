import { createHash } from "node:crypto";
import { spawn, spawnSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { performance } from "node:perf_hooks";
import { fileURLToPath } from "node:url";
import { isolatedBenchmarkState } from "./isolated-state.mjs";

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
  preflight: resolve(option("--preflight")),
  output: resolve(option("--output")),
  rounds: Number(process.argv.includes("--rounds") ? option("--rounds") : 10),
  installPolicy: process.argv.includes("--install-policy"),
  workloads: process.argv.includes("--workloads") ? option("--workloads").split(",").map((value) => value.trim()).filter(Boolean) : null,
  searchRoots: process.argv.includes("--search-roots") ? option("--search-roots").split(",").map((value) => value.trim()).filter(Boolean) : null,
  focusedPattern: process.argv.includes("--focused-pattern") ? option("--focused-pattern") : "graphVersion",
  broadPattern: process.argv.includes("--broad-pattern") ? option("--broad-pattern") : "function|const|class|require|module",
};
if (!Number.isInteger(settings.rounds) || settings.rounds < 1) {
    throw new Error("--rounds must be a positive integer");
}

const preflight = JSON.parse(readFileSync(settings.preflight, "utf8"));
if (preflight?.BenchmarkPreflight?.WindowsNativeRtkReady !== true) {
  throw new Error("P18 preflight does not verify a native Windows RTK provider");
}
const configuredNativeRtk = settings.nativeRtk.toLowerCase();
const matchingNativeRtk = (preflight?.Windows?.RtkEvidence || []).some((provider) =>
  provider?.HelpExitCode === 0
  && provider?.VersionExitCode === 0
  && resolve(provider.Path).toLowerCase() === configuredNativeRtk,
);
if (!matchingNativeRtk) {
  throw new Error("P18 preflight does not contain the selected native RTK path");
}

const rawGit = process.env.RTK_WAD_BENCH_GIT || "git.exe";
const rawRg = process.env.RTK_WAD_BENCH_RG || "rg.exe";
const isolatedWadState = isolatedBenchmarkState(settings.output);
const searchRoots = (settings.searchRoots || ["src", "tests", "test", "docs"])
  .filter((candidate) => existsSync(resolve(settings.repo, candidate)));
if (searchRoots.length === 0) {
  throw new Error("The benchmark corpus has no existing src, tests, test, or docs directory for ripgrep workloads");
}
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
    raw: [rawRg, ["-n", settings.focusedPattern, ...searchRoots]],
    rtk: ["rg", "-n", settings.focusedPattern, ...searchRoots],
  },
  {
    id: "rg-broad",
    raw: [rawRg, ["-n", settings.broadPattern, ...searchRoots]],
    rtk: ["rg", "-n", settings.broadPattern, ...searchRoots],
  },
];
const selectedWorkloads = settings.workloads === null ? workloads : workloads.filter((workload) => settings.workloads.includes(workload.id));
if (selectedWorkloads.length === 0 || (settings.workloads && selectedWorkloads.length !== settings.workloads.length)) {
  throw new Error("--workloads must name one or more supported core workloads");
}

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

function requireSuccessful(sample, label) {
  if (sample.exit_code !== 0 || sample.signal !== null) {
    throw new Error(`${label} failed: exit=${sample.exit_code}; signal=${sample.signal}`);
  }
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

function tokenizerVersion() {
  const result = spawnSync(settings.python, ["-c", "import tiktoken; print(tiktoken.__version__)"], {
    encoding: "utf8",
  });
  if (result.status !== 0) {
    throw new Error(`tiktoken version probe failed: ${result.stderr}`);
  }
  return result.stdout.trim();
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
  rtk_wad_native_candidate: {
    file: settings.wad,
    args: ["--route", "native-rtk", ...workload.rtk],
    environment: {
      XUVA_NATIVE_RTK_PATH: settings.nativeRtk,
      XUVA_STATE_DIR: isolatedWadState,
    },
  },
});

const allSamples = [];
for (const workload of selectedWorkloads) {
  const entries = Object.entries(variants(workload));
  for (const [, variant] of entries) {
    requireSuccessful(
      await execute(variant.file, variant.args, variant.environment),
      `${workload.id} warm-up`,
    );
  }
  for (let round = 0; round < settings.rounds; round += 1) {
    const rotated = entries.slice(round % entries.length).concat(entries.slice(0, round % entries.length));
    for (const [name, variant] of rotated) {
      const sample = await execute(variant.file, variant.args, variant.environment);
      requireSuccessful(sample, `${workload.id} ${name} round ${round + 1}`);
      allSamples.push({ workload: workload.id, variant: name, round: round + 1, ...sample });
    }
  }
}

const candidateSummaries = selectedWorkloads.map((workload) => {
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
const policyKey = {};
for (const workload of selectedWorkloads) {
  const keyResult = await execute(settings.wad, ["policy", "key", ...workload.rtk], {
    XUVA_NATIVE_RTK_PATH: settings.nativeRtk,
    XUVA_STATE_DIR: isolatedWadState,
  });
  requireSuccessful(keyResult, `${workload.id} policy key`);
  const match = /^key=([a-z0-9:._-]{1,128})\r?\n?$/.exec(keyResult.stdout.toString("utf8"));
  if (!match) throw new Error(`${workload.id} returned an invalid policy key`);
  policyKey[workload.id] = match[1];
}
const policyEvidence = Object.values(candidateSummaries.reduce((grouped, { workload, variants }) => {
  const key = policyKey[workload];
  const evidence = {
    key,
    raw_median_ms: variants.raw.median_ms,
    candidate_median_ms: variants.rtk_wad_native_candidate.median_ms,
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
const policyContext = await execute(settings.wad, ["policy", "context"], {
  XUVA_NATIVE_RTK_PATH: settings.nativeRtk,
  XUVA_STATE_DIR: isolatedWadState,
});
requireSuccessful(policyContext, "policy context");
const parsedPolicyContext = JSON.parse(policyContext.stdout.toString("utf8"));
if (parsedPolicyContext?.schema_version !== 2
  || parsedPolicyContext?.manifest_version !== "rtk:0.43.0:protocol-1"
  || typeof parsedPolicyContext?.context_signature !== "string"
  || parsedPolicyContext.context_signature.length !== 16) {
  throw new Error("WAD policy context is not compatible with the P16 policy contract");
}
const policyOutput = settings.output.replace(/\.json$/i, ".route-policy.json");
writeFileSync(policyOutput, JSON.stringify({
  schema_version: 2,
  manifest_version: parsedPolicyContext.manifest_version,
  context_signature: parsedPolicyContext.context_signature,
  evidence: policyEvidence,
}, null, 2));

const isolatedImport = await execute(settings.wad, ["policy", "import", policyOutput], {
  XUVA_NATIVE_RTK_PATH: settings.nativeRtk,
  XUVA_STATE_DIR: isolatedWadState,
});
requireSuccessful(isolatedImport, "isolated policy import");

for (const workload of selectedWorkloads) {
  const auto = {
    file: settings.wad,
    args: workload.rtk,
    environment: {
      XUVA_NATIVE_RTK_PATH: settings.nativeRtk,
      XUVA_STATE_DIR: isolatedWadState,
    },
  };
  requireSuccessful(
    await execute(auto.file, auto.args, auto.environment),
    `${workload.id} auto-policy warm-up`,
  );
  for (let round = 0; round < settings.rounds; round += 1) {
    const sample = await execute(auto.file, auto.args, auto.environment);
    requireSuccessful(sample, `${workload.id} auto-policy round ${round + 1}`);
    allSamples.push({
      workload: workload.id,
      variant: "rtk_wad_auto_after_policy",
      round: round + 1,
      ...sample,
    });
  }
}

const summaries = selectedWorkloads.map((workload) => {
  const perVariant = {};
  const rawSamples = allSamples.filter((sample) => sample.workload === workload.id && sample.variant === "raw");
  const rawTokens = exactTokens(Buffer.concat([rawSamples[0].stdout, rawSamples[0].stderr]));
  for (const name of [...Object.keys(variants(workload)), "rtk_wad_auto_after_policy"]) {
    const samples = allSamples.filter((sample) => sample.workload === workload.id && sample.variant === name);
    perVariant[name] = summarize(samples, rawTokens);
  }
  return { workload: workload.id, variants: perVariant };
});

writeFileSync(settings.output, JSON.stringify({
  schema_version: 2,
  protocol: "four-way-core-v2",
  tokenizer: "o200k_base",
  tokenizer_package: `tiktoken==${tokenizerVersion()}`,
  rounds: settings.rounds,
  workloads: selectedWorkloads.map((workload) => workload.id),
  corpus: settings.repo,
  search_roots: searchRoots,
  focused_pattern: settings.focusedPattern,
  broad_pattern: settings.broadPattern,
  native_rtk: settings.nativeRtk,
  rtk_wad: settings.wad,
  isolated_wad_state: isolatedWadState,
  route_policy: policyOutput,
  summaries,
  samples: allSamples.map(({ stdout, stderr, ...sample }) => ({
    ...sample,
    stdout_sha256: createHash("sha256").update(stdout).digest("hex"),
    stderr_sha256: createHash("sha256").update(stderr).digest("hex"),
    stdout_bytes: stdout.length,
    stderr_bytes: stderr.length,
  })),
}, null, 2));

console.log(`Wrote ${settings.output}`);
console.log(`Wrote ${policyOutput}`);
if (settings.installPolicy) {
  const imported = await execute(settings.wad, ["policy", "import", policyOutput], {
    XUVA_NATIVE_RTK_PATH: settings.nativeRtk,
  });
  if (imported.exit_code !== 0) throw new Error(`Policy import failed: ${imported.stderr}`);
  console.log("Installed the generated local route policy.");
}
