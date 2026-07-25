import { createHash } from "node:crypto";
import { spawn, spawnSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
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
  wad: resolve(option("--wad")),
  python: resolve(option("--python")),
  preflight: resolve(option("--preflight")),
  output: resolve(option("--output")),
  wsl1Distro: option("--wsl1-distro"),
  wsl1Rtk: option("--wsl1-rtk"),
  wsl2Distro: option("--wsl2-distro"),
  wsl2Rtk: option("--wsl2-rtk"),
  rounds: Number(process.argv.includes("--rounds") ? option("--rounds") : 5),
};
if (!Number.isInteger(settings.rounds) || settings.rounds < 5) {
  throw new Error("--rounds must be an integer of at least 5 for bridge evidence");
}

const preflight = JSON.parse(readFileSync(settings.preflight, "utf8"));
if (preflight?.BenchmarkPreflight?.Wsl1RtkReady !== true
  || preflight?.BenchmarkPreflight?.Wsl2RtkReady !== true) {
  throw new Error("P18 preflight does not verify both WSL1 and WSL2 RTK providers");
}

function requirePreflightProvider(distro, version, rtkPath) {
  const provider = (preflight?.Wsl || []).find((candidate) =>
    candidate?.Distro === distro
    && candidate?.WslVersion === version
    && candidate?.Rtk?.Path === rtkPath
    && candidate?.Rtk?.VersionExitCode === 0,
  );
  const coverage = (preflight?.Manifest?.Coverage || []).find((candidate) =>
    candidate?.Distro === distro && candidate?.WslVersion === version && candidate?.ExactMatch === true,
  );
  if (!provider || !coverage) {
    throw new Error(`P18 preflight does not verify ${distro} as WSL${version} with the selected RTK path`);
  }
}

requirePreflightProvider(settings.wsl1Distro, 1, settings.wsl1Rtk);
requirePreflightProvider(settings.wsl2Distro, 2, settings.wsl2Rtk);

const rawGit = process.env.RTK_WAD_BENCH_GIT || "git.exe";
const rawRg = process.env.RTK_WAD_BENCH_RG || "rg.exe";
const isolatedWadState = resolve(dirname(settings.output), "wad-bridge-state");
const searchRoots = ["src", "tests", "test", "docs"]
  .filter((candidate) => existsSync(resolve(settings.repo, candidate)));
if (searchRoots.length === 0) {
  throw new Error("The benchmark corpus has no existing src, tests, test, or docs directory for ripgrep workloads");
}
const workloads = [
  { id: "git-status", raw: [rawGit, ["status", "--short", "--branch"]], rtk: ["git", "status", "--short", "--branch"] },
  { id: "git-log-100", raw: [rawGit, ["log", "--oneline", "-100"]], rtk: ["git", "log", "--oneline", "-100"] },
  { id: "rg-focused", raw: [rawRg, ["-n", "graphVersion", ...searchRoots]], rtk: ["rg", "-n", "graphVersion", ...searchRoots] },
  { id: "rg-broad", raw: [rawRg, ["-n", "function|const|class|require|module", ...searchRoots]], rtk: ["rg", "-n", "function|const|class|require|module", ...searchRoots] },
];

function execute(file, args, environment = {}) {
  return new Promise((resolveExecution, reject) => {
    const started = performance.now();
    const child = spawn(file, args, {
      cwd: settings.repo,
      env: { ...process.env, ...environment },
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
  if (result.status !== 0) throw new Error(`o200k_base counter failed: ${result.stderr}`);
  return Number(result.stdout.trim());
}

function tokenizerVersion() {
  const result = spawnSync(settings.python, ["-c", "import tiktoken; print(tiktoken.__version__)"], { encoding: "utf8" });
  if (result.status !== 0) throw new Error(`tiktoken version probe failed: ${result.stderr}`);
  return result.stdout.trim();
}

function percentile(sorted, fraction) {
  return sorted[Math.min(sorted.length - 1, Math.ceil(sorted.length * fraction) - 1)];
}

function summarize(samples, rawTokens) {
  const elapsed = samples.map((sample) => sample.elapsed_ms).sort((left, right) => left - right);
  const output = Buffer.concat([samples[0].stdout, samples[0].stderr]);
  const tokens = exactTokens(output);
  return {
    runs: samples.length,
    median_ms: percentile(elapsed, 0.5),
    p95_ms: percentile(elapsed, 0.95),
    exit_codes: [...new Set(samples.map((sample) => sample.exit_code))],
    signals: [...new Set(samples.map((sample) => sample.signal))],
    output_bytes: output.length,
    o200k_tokens: tokens,
    tokens_saved_vs_raw: rawTokens - tokens,
    token_savings_percent: rawTokens === 0 ? 0 : Number((((rawTokens - tokens) / rawTokens) * 100).toFixed(1)),
    output_hashes: [...new Set(samples.map((sample) => createHash("sha256").update(sample.stdout).update(sample.stderr).digest("hex")))],
  };
}

function wslVariant(route, distro, rtkPath, args) {
  return {
    file: settings.wad,
    args: ["--route", route, ...args],
    environment: {
      RTK_WSL_BACKEND: route,
      RTK_WSL_DISTRO: distro,
      RTK_WSL_RTK_PATH: rtkPath,
      RTK_WAD_STATE_DIR: isolatedWadState,
    },
  };
}

function variants(workload) {
  return {
    raw_windows: { file: workload.raw[0], args: workload.raw[1], environment: {} },
    wad_wsl1: wslVariant("wsl1", settings.wsl1Distro, settings.wsl1Rtk, workload.rtk),
    wad_wsl2: wslVariant("wsl2", settings.wsl2Distro, settings.wsl2Rtk, workload.rtk),
  };
}

const samples = [];
for (const workload of workloads) {
  const entries = Object.entries(variants(workload));
  for (const [, variant] of entries) {
    requireSuccessful(await execute(variant.file, variant.args, variant.environment), `${workload.id} warm-up`);
  }
  for (let round = 0; round < settings.rounds; round += 1) {
    const rotated = entries.slice(round % entries.length).concat(entries.slice(0, round % entries.length));
    for (const [name, variant] of rotated) {
      const sample = await execute(variant.file, variant.args, variant.environment);
      requireSuccessful(sample, `${workload.id} ${name} round ${round + 1}`);
      samples.push({ workload: workload.id, variant: name, round: round + 1, ...sample });
    }
  }
}

const summaries = workloads.map((workload) => {
  const perVariant = {};
  const rawSamples = samples.filter((sample) => sample.workload === workload.id && sample.variant === "raw_windows");
  const rawTokens = exactTokens(Buffer.concat([rawSamples[0].stdout, rawSamples[0].stderr]));
  for (const name of Object.keys(variants(workload))) {
    perVariant[name] = summarize(samples.filter((sample) => sample.workload === workload.id && sample.variant === name), rawTokens);
  }
  return { workload: workload.id, variants: perVariant };
});

mkdirSync(dirname(settings.output), { recursive: true });
writeFileSync(settings.output, JSON.stringify({
  schema_version: 1,
  protocol: "wsl-bridge-core-v1",
  tokenizer: "o200k_base",
  tokenizer_package: `tiktoken==${tokenizerVersion()}`,
  rounds: settings.rounds,
  corpus: settings.repo,
  search_roots: searchRoots,
  rtk_wad: settings.wad,
  wsl1: { distro: settings.wsl1Distro, rtk: settings.wsl1Rtk },
  wsl2: { distro: settings.wsl2Distro, rtk: settings.wsl2Rtk },
  isolated_wad_state: isolatedWadState,
  summaries,
  samples: samples.map(({ stdout, stderr, ...sample }) => ({
    ...sample,
    stdout_sha256: createHash("sha256").update(stdout).digest("hex"),
    stderr_sha256: createHash("sha256").update(stderr).digest("hex"),
    stdout_bytes: stdout.length,
    stderr_bytes: stderr.length,
  })),
}, null, 2));

console.log(`Wrote ${settings.output}`);
