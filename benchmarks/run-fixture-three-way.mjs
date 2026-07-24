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
  nativeRtk: resolve(option("--native-rtk")),
  wad: resolve(option("--wad")),
  windowsBin: resolve(option("--windows-bin")),
  linuxBin: option("--linux-bin"),
  python: resolve(option("--python")),
  output: resolve(option("--output")),
  rounds: Number(process.argv.includes("--rounds") ? option("--rounds") : 5),
};
if (!Number.isInteger(settings.rounds) || settings.rounds < 5) {
  throw new Error("--rounds must be an integer of at least 5 for coverage evidence");
}

const cases = [
  ["aws", ["sts", "get-caller-identity"]],
  ["curl", ["https://fixture.invalid/api"]],
  ["docker", ["ps"]],
  ["gh", ["repo", "view"]],
  ["glab", ["repo", "view"]],
  ["kubectl", ["get", "pods"]],
  ["oc", ["get", "pods"]],
  ["psql", ["-c", "select 1"]],
  ["wget", ["https://fixture.invalid/archive"]],
];

function execute(variant) {
  return new Promise((resolveExecution, reject) => {
    const started = performance.now();
    const child = spawn(variant.file, variant.args, {
      shell: false,
      windowsHide: true,
      env: { ...process.env, ...variant.environment },
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

function normalizedOutput(sample) {
  return Buffer.concat([sample.stdout, sample.stderr]).toString("utf8").replace(/\r\n/g, "\n");
}

function normalizedOutputHash(sample) {
  return createHash("sha256").update(normalizedOutput(sample)).digest("hex");
}

function expectedRawFixtureOutput(command, args) {
  return `fixture=${command};argc=${args.length};argv=${args.join("|")}\n`;
}

function summarize(samples, rawTokens) {
  const elapsed = samples.map((sample) => sample.elapsed_ms).sort((a, b) => a - b);
  const output = Buffer.concat([samples[0].stdout, samples[0].stderr]);
  const hashes = [...new Set(samples.map((sample) => createHash("sha256").update(sample.stdout).update(sample.stderr).digest("hex")))];
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
    output_hashes: hashes,
  };
}

const samples = [];
for (const [command, args] of cases) {
  const variants = {
    raw: { file: resolve(settings.windowsBin, `${command}.exe`), args, environment: {} },
    native_rtk: {
      file: settings.nativeRtk,
      args: [command, ...args],
      environment: { Path: `${settings.windowsBin};${process.env.Path}` },
    },
    rtk_wad_auto: {
      file: settings.wad,
      args: [command, ...args],
      environment: {
        RTK_WSL_EXTRA_PATH: settings.linuxBin,
        RTK_WAD_NATIVE_RTK_PATH: settings.nativeRtk,
        LOCALAPPDATA: resolve(dirname(settings.output), "wad-local-app-data"),
      },
    },
  };
  for (const variant of Object.values(variants)) await execute(variant);
  const entries = Object.entries(variants);
  for (let round = 0; round < settings.rounds; round += 1) {
    const rotated = entries.slice(round % entries.length).concat(entries.slice(0, round % entries.length));
    for (const [name, variant] of rotated) {
      samples.push({ command, variant: name, round: round + 1, ...(await execute(variant)) });
    }
  }
}

const summary = cases.map(([command]) => {
  const perVariant = {};
  const rawSamples = samples.filter((sample) => sample.command === command && sample.variant === "raw");
  const rawTokens = exactTokens(Buffer.concat([rawSamples[0].stdout, rawSamples[0].stderr]));
  for (const name of ["raw", "native_rtk", "rtk_wad_auto"]) {
    perVariant[name] = summarize(samples.filter((sample) => sample.command === command && sample.variant === name), rawTokens);
  }
  const rawFixtureContractPassed = rawSamples.every((sample) => normalizedOutput(sample) === expectedRawFixtureOutput(command, cases.find(([name]) => name === command)[1]));
  const nativeWadSamples = samples.filter((sample) => sample.command === command && sample.variant !== "raw");
  const nativeWadContractPassed = new Set(nativeWadSamples.map(normalizedOutputHash)).size === 1;
  return {
    command,
    raw_fixture_contract_passed: rawFixtureContractPassed,
    native_wad_contract_passed: nativeWadContractPassed,
    all_variants_succeeded: Object.values(perVariant).every((variant) => variant.exit_codes.length === 1 && variant.exit_codes[0] === 0 && variant.signals.length === 1 && variant.signals[0] === null),
    variants: perVariant,
  };
});
const allContractsPassed = summary.every((item) => item.raw_fixture_contract_passed && item.native_wad_contract_passed && item.all_variants_succeeded);
mkdirSync(dirname(settings.output), { recursive: true });
writeFileSync(settings.output, JSON.stringify({
  schema_version: 1,
  protocol: "three-way-fixture-v2",
  tokenizer: "o200k_base",
  rounds: settings.rounds,
  fixture_contracts_passed: allContractsPassed,
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
if (!allContractsPassed) throw new Error("Fixture argv or process contract failed; coverage is not valid");
