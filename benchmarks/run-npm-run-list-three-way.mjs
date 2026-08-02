import { createHash } from "node:crypto";
import { spawn, spawnSync } from "node:child_process";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, isAbsolute, resolve } from "node:path";
import { performance } from "node:perf_hooks";
import { fileURLToPath } from "node:url";
import { isolatedBenchmarkState } from "./isolated-state.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const option = (name) => {
  const index = process.argv.indexOf(name);
  if (index === -1 || !process.argv[index + 1]) throw new Error(`Missing required option ${name}`);
  return process.argv[index + 1];
};
const settings = {
  repo: resolve(option('--repo')),
  nativeRtk: resolve(option('--native-rtk')),
  wad: resolve(option('--wad')),
  python: resolve(option('--python')),
  preflight: resolve(option('--preflight')),
  output: resolve(option('--output')),
  rounds: Number(process.argv.includes('--rounds') ? option('--rounds') : 5),
};
if (!Number.isInteger(settings.rounds) || settings.rounds < 5) throw new Error('--rounds must be an integer of at least 5');

const preflight = JSON.parse(readFileSync(settings.preflight, 'utf8'));
const exactNative = (preflight?.Windows?.RtkEvidence || []).some((provider) => provider?.HelpExitCode === 0
  && provider?.VersionExitCode === 0
  && resolve(provider.Path).toLowerCase() === settings.nativeRtk.toLowerCase());
if (preflight?.BenchmarkPreflight?.WindowsNativeRtkReady !== true || !exactNative) throw new Error('P18 preflight does not verify the selected stock Windows RTK provider');

function tokenizerPackage() {
  const result = spawnSync(settings.python, ['-c', 'import tiktoken; print(tiktoken.__version__)'], { encoding: 'utf8' });
  if (result.status !== 0 || !result.stdout.trim()) throw new Error(`Pinned tokenizer is unavailable: ${result.stderr}`);
  return `tiktoken==${result.stdout.trim()}`;
}

const rawNpm = process.env.RTK_WAD_BENCH_NPM || 'npm.cmd';
const npmPath = isAbsolute(rawNpm) ? `${dirname(rawNpm)};${process.env.Path || ''}` : (process.env.Path || '');
const state = isolatedBenchmarkState(settings.output);
const wadEnvironment = { XUVA_NATIVE_RTK_PATH: settings.nativeRtk, XUVA_STATE_DIR: state, Path: npmPath };
function execute(variant) {
  return new Promise((done, fail) => {
    const started = performance.now();
    const child = spawn(variant.file, variant.args, { cwd: settings.repo, env: { ...process.env, ...variant.environment }, shell: false, windowsHide: true });
    const stdout = [];
    const stderr = [];
    child.stdout.on('data', (chunk) => stdout.push(chunk));
    child.stderr.on('data', (chunk) => stderr.push(chunk));
    child.on('error', fail);
    child.on('close', (exitCode, signal) => done({ elapsed_ms: Number((performance.now() - started).toFixed(3)), exit_code: exitCode, signal, stdout: Buffer.concat(stdout), stderr: Buffer.concat(stderr) }));
  });
}
function requireSuccess(sample, label) { if (sample.exit_code !== 0 || sample.signal !== null) throw new Error(`${label} failed: exit=${sample.exit_code}; signal=${sample.signal}`); }
function tokens(buffer) {
  const result = spawnSync(settings.python, [resolve(here, 'token-count.py')], { input: buffer, encoding: 'utf8', maxBuffer: 32 * 1024 * 1024 });
  if (result.status !== 0) throw new Error(`o200k_base counter failed: ${result.stderr}`);
  return Number(result.stdout.trim());
}
function percentile(values, fraction) { return values[Math.min(values.length - 1, Math.ceil(values.length * fraction) - 1)]; }
function summarize(samples, rawTokens) {
  const elapsed = samples.map((sample) => sample.elapsed_ms).sort((a, b) => a - b);
  const output = Buffer.concat([samples[0].stdout, samples[0].stderr]);
  const outputHashes = [...new Set(samples.map((sample) => createHash('sha256').update(sample.stdout).update(sample.stderr).digest('hex')))];
  const counted = tokens(output);
  return { runs: samples.length, median_ms: percentile(elapsed, 0.5), p95_ms: percentile(elapsed, 0.95), min_ms: elapsed[0], max_ms: elapsed.at(-1), exit_codes: [...new Set(samples.map((sample) => sample.exit_code))], signals: [...new Set(samples.map((sample) => sample.signal))], output_bytes: output.length, o200k_tokens: counted, tokens_saved_vs_raw: rawTokens - counted, token_savings_percent: rawTokens === 0 ? 0 : Number((((rawTokens - counted) / rawTokens) * 100).toFixed(1)), output_hashes: outputHashes };
}
async function runRotating(variants, samples) {
  for (const [name, variant] of Object.entries(variants)) requireSuccess(await execute(variant), `${name} warm-up`);
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
async function runAuto(samples) {
  const variant = { file: settings.wad, args: ['npm', 'run'], environment: wadEnvironment };
  requireSuccess(await execute(variant), 'rtk_wad_auto_after_policy warm-up');
  for (let round = 0; round < settings.rounds; round += 1) {
    const sample = await execute(variant);
    requireSuccess(sample, `rtk_wad_auto_after_policy round ${round + 1}`);
    samples.push({ variant: 'rtk_wad_auto_after_policy', round: round + 1, ...sample });
  }
}

const baseline = {
  raw: { file: process.env.ComSpec || 'cmd.exe', args: ['/d', '/s', '/c', rawNpm, 'run'], environment: { Path: npmPath } },
  native_rtk: { file: settings.nativeRtk, args: ['npm', 'run'], environment: { Path: npmPath } },
  rtk_wad_native_candidate: { file: settings.wad, args: ['--route', 'native-rtk', 'npm', 'run'], environment: wadEnvironment },
};
const samples = [];
await runRotating(baseline, samples);
const firstRaw = samples.find((sample) => sample.variant === 'raw');
const rawTokens = tokens(Buffer.concat([firstRaw.stdout, firstRaw.stderr]));
const context = await execute({ file: settings.wad, args: ['policy', 'context'], environment: wadEnvironment });
requireSuccess(context, 'policy context');
const parsedContext = JSON.parse(context.stdout.toString('utf8'));
if (parsedContext?.schema_version !== 2 || parsedContext?.manifest_version !== '0.43.0' || typeof parsedContext?.context_signature !== 'string' || parsedContext.context_signature.length !== 16) throw new Error('WAD policy context is not compatible with P16');
const candidate = summarize(samples.filter((sample) => sample.variant === 'rtk_wad_native_candidate'), rawTokens);
const policy = { schema_version: 2, manifest_version: parsedContext.manifest_version, context_signature: parsedContext.context_signature, evidence: [{ key: 'npm:run-list', raw_median_ms: summarize(samples.filter((sample) => sample.variant === 'raw'), rawTokens).median_ms, candidate_median_ms: candidate.median_ms, token_savings_percent: candidate.token_savings_percent, sample_count: settings.rounds }] };
const policyPath = settings.output.replace(/\.json$/i, '.route-policy.json');
mkdirSync(dirname(settings.output), { recursive: true });
writeFileSync(policyPath, JSON.stringify(policy, null, 2));
requireSuccess(await execute({ file: settings.wad, args: ['policy', 'import', policyPath], environment: wadEnvironment }), 'isolated policy import');
await runAuto(samples);

const summary = Object.fromEntries([...new Set(samples.map((sample) => sample.variant))].map((name) => [name, summarize(samples.filter((sample) => sample.variant === name), rawTokens)]));
const allSuccessful = samples.every((sample) => sample.exit_code === 0 && sample.signal === null);
const rawAutoEquivalent = summary.raw.output_hashes.length === 1
  && summary.rtk_wad_auto_after_policy.output_hashes.length === 1
  && summary.raw.output_hashes[0] === summary.rtk_wad_auto_after_policy.output_hashes[0];
mkdirSync(dirname(settings.output), { recursive: true });
writeFileSync(settings.output, JSON.stringify({ schema_version: 2, protocol: 'four-way-npm-run-list-v2', tokenizer: 'o200k_base', tokenizer_package: tokenizerPackage(), preflight: settings.preflight, rounds: settings.rounds, corpus: settings.repo, native_rtk: settings.nativeRtk, rtk_wad: settings.wad, isolated_wad_state: state, policy, all_successful: allSuccessful, raw_wad_output_equivalent: rawAutoEquivalent, coverage_valid: allSuccessful && rawAutoEquivalent, summary, samples: samples.map(({ stdout, stderr, ...sample }) => ({ ...sample, stdout_sha256: createHash('sha256').update(stdout).digest('hex'), stderr_sha256: createHash('sha256').update(stderr).digest('hex'), stdout_bytes: stdout.length, stderr_bytes: stderr.length })) }, null, 2));
console.log(`Wrote ${settings.output}`);
console.log(`Wrote ${policyPath}`);
if (!rawAutoEquivalent) throw new Error('Raw and final WAD output differ; coverage is not valid');
