import { createHash } from "node:crypto";
import { spawn, spawnSync } from "node:child_process";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, isAbsolute, resolve } from "node:path";
import { performance } from "node:perf_hooks";
import { fileURLToPath } from "node:url";
import { adapterContractId } from "./benchmark-contract.mjs";
import { isolatedBenchmarkState } from "./isolated-state.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const option = (name) => {
  const index = process.argv.indexOf(name);
  if (index === -1 || !process.argv[index + 1]) throw new Error(`Missing required option ${name}`);
  return process.argv[index + 1];
};
const delimiter = process.argv.indexOf("--");
if (delimiter === -1 || delimiter === process.argv.length - 1) throw new Error("Pass measured tool arguments after --");

const settings = {
  tool: option("--tool"),
  repo: resolve(option("--repo")),
  nativeRtk: resolve(option("--native-rtk")),
  wad: resolve(option("--wad")),
  python: resolve(option("--python")),
  preflight: resolve(option("--preflight")),
  output: resolve(option("--output")),
  rounds: Number(process.argv.includes("--rounds") ? option("--rounds") : 5),
  timeoutMs: Number(process.argv.includes("--timeout-ms") ? option("--timeout-ms") : 60000),
  normalizer: process.argv.includes("--normalizer") ? option("--normalizer") : "none",
  skipWarmup: process.argv.includes("--skip-warmup"),
  policyKey: process.argv.includes("--policy-key") ? option("--policy-key") : null,
  withoutNative: process.argv.includes("--without-native"),
  args: process.argv.slice(delimiter + 1),
};
if (!Number.isInteger(settings.rounds) || settings.rounds < 5) throw new Error("--rounds must be an integer of at least 5");
if (!Number.isInteger(settings.timeoutMs) || settings.timeoutMs < 1000) throw new Error("--timeout-ms must be an integer of at least 1000");
if (!['none', 'cargo-check-duration', 'dart-format-duration', 'flutter-analysis-duration'].includes(settings.normalizer)) throw new Error("unsupported output normalizer");
if (settings.policyKey && settings.withoutNative) throw new Error("--policy-key requires a stock native RTK comparison");

const preflight = JSON.parse(readFileSync(settings.preflight, "utf8"));
const matchingNative = (preflight?.Windows?.RtkEvidence || []).some((provider) => provider?.HelpExitCode === 0
  && provider?.VersionExitCode === 0
  && resolve(provider.Path).toLowerCase() === settings.nativeRtk.toLowerCase());
if (preflight?.BenchmarkPreflight?.WindowsNativeRtkReady !== true || !matchingNative) {
  throw new Error("P18 preflight does not verify the selected stock Windows RTK provider");
}

function tokenizerPackage() {
  const result = spawnSync(settings.python, ['-c', 'import tiktoken; print(tiktoken.__version__)'], { encoding: 'utf8' });
  if (result.status !== 0 || !result.stdout.trim()) throw new Error(`Pinned tokenizer is unavailable: ${result.stderr}`);
  return `tiktoken==${result.stdout.trim()}`;
}

const rawExecutable = process.env.RTK_WAD_BENCH_TOOL || ({ cargo: 'cargo.exe', dart: 'dart.bat', flutter: 'flutter.bat', go: 'go.exe', dotnet: 'dotnet.exe' }[settings.tool] || settings.tool);
const rawVariant = /\.(cmd|bat)$/i.test(rawExecutable)
  ? { file: process.env.ComSpec || 'cmd.exe', args: ['/d', '/s', '/c', rawExecutable, ...settings.args] }
  : { file: rawExecutable, args: settings.args };
const rawToolPath = isAbsolute(rawExecutable) ? `${dirname(rawExecutable)};${process.env.Path || ''}` : (process.env.Path || '');
const state = isolatedBenchmarkState(settings.output);
const expectedAdapterContract = adapterContractId();
const wadEnvironment = { XUVA_STATE_DIR: state, XUVA_NATIVE_RTK_PATH: settings.nativeRtk, Path: rawToolPath };

function execute(variant) {
  return new Promise((done, fail) => {
    const started = performance.now();
    let timedOut = false;
    const child = spawn(variant.file, variant.args, {
      cwd: settings.repo,
      env: { ...process.env, XUVA_NATIVE_RTK_PATH: settings.nativeRtk, ...variant.environment },
      shell: false,
      windowsHide: true,
    });
    const stdout = [];
    const stderr = [];
    child.stdout.on('data', (chunk) => stdout.push(chunk));
    child.stderr.on('data', (chunk) => stderr.push(chunk));
    const timeout = setTimeout(() => {
      timedOut = true;
      spawn('taskkill.exe', ['/pid', String(child.pid), '/t', '/f'], { windowsHide: true });
    }, settings.timeoutMs);
    child.on('error', fail);
    child.on('close', (exitCode, signal) => {
      clearTimeout(timeout);
      done({ elapsed_ms: Number((performance.now() - started).toFixed(3)), exit_code: exitCode, signal, timed_out: timedOut, stdout: Buffer.concat(stdout), stderr: Buffer.concat(stderr) });
    });
  });
}
function requireSuccess(sample, label) {
  if (sample.exit_code !== 0 || sample.signal !== null || sample.timed_out) throw new Error(`${label} failed: exit=${sample.exit_code}; signal=${sample.signal}; timed_out=${sample.timed_out}`);
}
function exactTokens(buffer) {
  const result = spawnSync(settings.python, [resolve(here, 'token-count.py')], { input: buffer, encoding: 'utf8', maxBuffer: 32 * 1024 * 1024 });
  if (result.status !== 0) throw new Error(`o200k_base counter failed: ${result.stderr}`);
  return Number(result.stdout.trim());
}
function normalized(sample) {
  const text = Buffer.concat([sample.stdout, sample.stderr]).toString('utf8');
  if (settings.normalizer === 'cargo-check-duration') return text.replace(/target\(s\) in \d+(?:\.\d+)?s/g, 'target(s) in <duration>');
  if (settings.normalizer === 'dart-format-duration') return text.replace(/in \d+(?:\.\d+)? seconds\./g, 'in <duration> seconds.');
  if (settings.normalizer === 'flutter-analysis-duration') return text.replace(/\(ran in [^)]+\)/g, '(ran in <duration>)');
  return text;
}
function percentile(values, fraction) { return values[Math.min(values.length - 1, Math.ceil(values.length * fraction) - 1)]; }
function summarize(samples, rawTokens) {
  const elapsed = samples.map((sample) => sample.elapsed_ms).sort((a, b) => a - b);
  const output = Buffer.concat([samples[0].stdout, samples[0].stderr]);
  const outputHashes = [...new Set(samples.map((sample) => createHash('sha256').update(sample.stdout).update(sample.stderr).digest('hex')))];
  const semanticHashes = [...new Set(samples.map((sample) => createHash('sha256').update(normalized(sample)).digest('hex')))];
  const tokens = exactTokens(output);
  return { runs: samples.length, median_ms: percentile(elapsed, 0.5), p95_ms: percentile(elapsed, 0.95), min_ms: elapsed[0], max_ms: elapsed.at(-1), exit_codes: [...new Set(samples.map((sample) => sample.exit_code))], signals: [...new Set(samples.map((sample) => sample.signal))], timed_out: samples.some((sample) => sample.timed_out), output_bytes: output.length, o200k_tokens: tokens, tokens_saved_vs_raw: rawTokens - tokens, token_savings_percent: rawTokens === 0 ? 0 : Number((((rawTokens - tokens) / rawTokens) * 100).toFixed(1)), output_hashes: outputHashes, semantic_output_hashes: semanticHashes };
}
async function runRotatingRounds(variants, samples) {
  if (!settings.skipWarmup) for (const [name, variant] of Object.entries(variants)) requireSuccess(await execute(variant), `${name} warm-up`);
  const entries = Object.entries(variants);
  for (let round = 0; round < settings.rounds; round += 1) {
    const rotated = entries.slice(round % entries.length).concat(entries.slice(0, round % entries.length));
    for (const [name, variant] of rotated) {
      const sample = await execute(variant);
      requireSuccess(sample, `${name} round ${round + 1}`);
      samples.push({ variant: name, round: round + 1, ...sample });
    }
  }
}
async function runSingleVariant(name, variant, samples) {
  if (!settings.skipWarmup) requireSuccess(await execute(variant), `${name} warm-up`);
  for (let round = 0; round < settings.rounds; round += 1) {
    const sample = await execute(variant);
    requireSuccess(sample, `${name} round ${round + 1}`);
    samples.push({ variant: name, round: round + 1, ...sample });
  }
}

const baseline = { raw: { ...rawVariant, environment: {} } };
if (!settings.withoutNative) baseline.native_rtk = { file: settings.nativeRtk, args: [settings.tool, ...settings.args], environment: { Path: rawToolPath } };
baseline[settings.policyKey ? 'rtk_wad_native_candidate' : 'rtk_wad_auto'] = {
  file: settings.wad,
  args: settings.policyKey ? ['--route', 'native-rtk', settings.tool, ...settings.args] : [settings.tool, ...settings.args],
  environment: wadEnvironment,
};

const samples = [];
await runRotatingRounds(baseline, samples);

let policy = null;
if (settings.policyKey) {
  const rawTokens = exactTokens(Buffer.concat([samples.find((sample) => sample.variant === 'raw').stdout, samples.find((sample) => sample.variant === 'raw').stderr]));
  const candidateSummary = summarize(samples.filter((sample) => sample.variant === 'rtk_wad_native_candidate'), rawTokens);
  const context = await execute({ file: settings.wad, args: ['policy', 'context'], environment: wadEnvironment });
  requireSuccess(context, 'policy context');
  const parsed = JSON.parse(context.stdout.toString('utf8'));
  if (parsed?.schema_version !== 2 || parsed?.manifest_version !== expectedAdapterContract || typeof parsed?.context_signature !== 'string' || parsed.context_signature.length !== 16) throw new Error('XUVA policy context does not match the authoritative adapter manifest');
  const keyResult = await execute({ file: settings.wad, args: ['policy', 'key', settings.tool, ...settings.args], environment: wadEnvironment });
  requireSuccess(keyResult, 'policy key');
  const keyMatch = /^key=([a-z0-9:._-]{1,128})\r?\n?$/.exec(keyResult.stdout.toString('utf8'));
  if (!keyMatch || keyMatch[1] !== settings.policyKey) throw new Error('--policy-key must exactly match `xuva policy key` for this workload shape');
  policy = { schema_version: 2, manifest_version: parsed.manifest_version, context_signature: parsed.context_signature, evidence: [{ key: keyMatch[1], raw_median_ms: summarize(samples.filter((sample) => sample.variant === 'raw'), rawTokens).median_ms, candidate_median_ms: candidateSummary.median_ms, token_savings_percent: candidateSummary.token_savings_percent, sample_count: settings.rounds }] };
  const policyPath = settings.output.replace(/\.json$/i, '.route-policy.json');
  mkdirSync(dirname(settings.output), { recursive: true });
  writeFileSync(policyPath, JSON.stringify(policy, null, 2));
  const imported = await execute({ file: settings.wad, args: ['policy', 'import', policyPath], environment: wadEnvironment });
  requireSuccess(imported, 'isolated policy import');
  await runSingleVariant('rtk_wad_auto_after_policy', { file: settings.wad, args: [settings.tool, ...settings.args], environment: wadEnvironment }, samples);
}

const rawTokens = exactTokens(Buffer.concat([samples.find((sample) => sample.variant === 'raw').stdout, samples.find((sample) => sample.variant === 'raw').stderr]));
const summary = Object.fromEntries([...new Set(samples.map((sample) => sample.variant))].map((name) => [name, summarize(samples.filter((sample) => sample.variant === name), rawTokens)]));
const finalWad = summary.rtk_wad_auto_after_policy || summary.rtk_wad_auto;
const rawWadEquivalent = summary.raw.semantic_output_hashes.length === 1 && finalWad.semantic_output_hashes.length === 1 && summary.raw.semantic_output_hashes[0] === finalWad.semantic_output_hashes[0];

mkdirSync(dirname(settings.output), { recursive: true });
writeFileSync(settings.output, JSON.stringify({
  schema_version: 2,
  protocol: settings.policyKey ? 'four-way-toolchain-policy-v2' : (settings.withoutNative ? 'two-way-toolchain-v2' : 'three-way-toolchain-v2'),
  tokenizer: 'o200k_base', tokenizer_package: tokenizerPackage(), preflight: settings.preflight, tool: settings.tool, args: settings.args, policy_key: settings.policyKey, output_normalizer: settings.normalizer, rounds: settings.rounds, timeout_ms: settings.timeoutMs, warmup: settings.skipWarmup ? 'external' : 'runner', corpus: settings.repo, native_rtk: settings.withoutNative ? null : settings.nativeRtk, rtk_wad: settings.wad, isolated_wad_state: state, policy, raw_wad_output_equivalent: rawWadEquivalent, coverage_valid: rawWadEquivalent, summary,
  samples: samples.map(({ stdout, stderr, ...sample }) => ({ ...sample, stdout_sha256: createHash('sha256').update(stdout).digest('hex'), stderr_sha256: createHash('sha256').update(stderr).digest('hex'), semantic_output_sha256: createHash('sha256').update(normalized({ stdout, stderr })).digest('hex'), stdout_bytes: stdout.length, stderr_bytes: stderr.length })),
}, null, 2));
console.log(`Wrote ${settings.output}`);
if (policy) console.log(`Wrote ${settings.output.replace(/\.json$/i, '.route-policy.json')}`);
if (!rawWadEquivalent) throw new Error('Raw and final WAD semantic outputs differ; coverage is not valid');
