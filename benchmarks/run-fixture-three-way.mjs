import { createHash } from "node:crypto";
import { spawn, spawnSync } from "node:child_process";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { performance } from "node:perf_hooks";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const option = (name) => {
  const index = process.argv.indexOf(name);
  if (index === -1 || !process.argv[index + 1]) throw new Error(`Missing required option ${name}`);
  return process.argv[index + 1];
};
const settings = {
  nativeRtk: resolve(option('--native-rtk')),
  wad: resolve(option('--wad')),
  windowsBin: resolve(option('--windows-bin')),
  linuxBin: option('--linux-bin'),
  wsl1Distro: option('--wsl1-distro'),
  wsl1Rtk: option('--wsl1-rtk'),
  python: resolve(option('--python')),
  preflight: resolve(option('--preflight')),
  output: resolve(option('--output')),
  rounds: Number(process.argv.includes('--rounds') ? option('--rounds') : 5),
  commands: process.argv.includes('--commands') ? option('--commands').split(',').map((value) => value.trim()).filter(Boolean) : null,
};
if (!Number.isInteger(settings.rounds) || settings.rounds < 5) throw new Error('--rounds must be an integer of at least 5');

const preflight = JSON.parse(readFileSync(settings.preflight, 'utf8'));
const nativeReady = preflight?.BenchmarkPreflight?.WindowsNativeRtkReady === true
  && (preflight?.Windows?.RtkEvidence || []).some((provider) => provider?.HelpExitCode === 0
    && provider?.VersionExitCode === 0
    && resolve(provider.Path).toLowerCase() === settings.nativeRtk.toLowerCase());
const wsl1Ready = preflight?.BenchmarkPreflight?.Wsl1RtkReady === true
  && (preflight?.Wsl || []).some((provider) => provider?.Distro === settings.wsl1Distro
    && provider?.WslVersion === 1
    && provider?.Rtk?.Path === settings.wsl1Rtk)
  && (preflight?.Manifest?.Coverage || []).some((coverage) => coverage?.Distro === settings.wsl1Distro
    && coverage?.WslVersion === 1
    && coverage?.ExactMatch === true);
if (!nativeReady || !wsl1Ready) throw new Error('P18 preflight does not verify the selected Windows RTK and WSL1 RTK providers');

function tokenizerPackage() {
  const result = spawnSync(settings.python, ['-c', 'import tiktoken; print(tiktoken.__version__)'], { encoding: 'utf8' });
  if (result.status !== 0 || !result.stdout.trim()) throw new Error(`Pinned tokenizer is unavailable: ${result.stderr}`);
  return `tiktoken==${result.stdout.trim()}`;
}

const cases = [
  ['aws', ['sts', 'get-caller-identity']], ['curl', ['https://fixture.invalid/api']], ['docker', ['ps']],
  ['gh', ['repo', 'view']], ['glab', ['repo', 'view']], ['kubectl', ['get', 'pods']], ['oc', ['get', 'pods']],
  ['psql', ['-c', 'select 1']], ['wget', ['https://fixture.invalid/archive']],
];
const selectedCases = settings.commands === null ? cases : cases.filter(([command]) => settings.commands.includes(command));
if (selectedCases.length === 0 || (settings.commands && selectedCases.length !== settings.commands.length)) {
  throw new Error('--commands must name one or more supported fixture commands');
}
const windowsPath = `${settings.windowsBin};${process.env.Path || ''}`;
function execute(variant) {
  return new Promise((done, fail) => {
    const started = performance.now();
    const child = spawn(variant.file, variant.args, { shell: false, windowsHide: true, env: { ...process.env, ...variant.environment } });
    const stdout = []; const stderr = [];
    child.stdout.on('data', (chunk) => stdout.push(chunk));
    child.stderr.on('data', (chunk) => stderr.push(chunk));
    child.on('error', fail);
    child.on('close', (exitCode, signal) => done({ elapsed_ms: Number((performance.now() - started).toFixed(3)), exit_code: exitCode, signal, stdout: Buffer.concat(stdout), stderr: Buffer.concat(stderr) }));
  });
}
function requireSuccess(sample, label) { if (sample.exit_code !== 0 || sample.signal !== null) throw new Error(`${label} failed: exit=${sample.exit_code}; signal=${sample.signal}`); }
function exactTokens(buffer) {
  const result = spawnSync(settings.python, [resolve(here, 'token-count.py')], { input: buffer, encoding: 'utf8', maxBuffer: 32 * 1024 * 1024 });
  if (result.status !== 0) throw new Error(`o200k_base counter failed: ${result.stderr}`);
  return Number(result.stdout.trim());
}
function percentile(values, fraction) { return values[Math.min(values.length - 1, Math.ceil(values.length * fraction) - 1)]; }
function normalized(sample) { return Buffer.concat([sample.stdout, sample.stderr]).toString('utf8').replace(/\r\n/g, '\n'); }
function hashNormalized(sample) { return createHash('sha256').update(normalized(sample)).digest('hex'); }
function expectedRaw(command, args) { return `fixture=${command};argc=${args.length};argv=${args.join('|')}\n`; }
function summarize(samples, rawTokens) {
  const elapsed = samples.map((sample) => sample.elapsed_ms).sort((a, b) => a - b);
  const output = Buffer.concat([samples[0].stdout, samples[0].stderr]);
  const counted = exactTokens(output);
  return { runs: samples.length, median_ms: percentile(elapsed, 0.5), p95_ms: percentile(elapsed, 0.95), min_ms: elapsed[0], max_ms: elapsed.at(-1), exit_codes: [...new Set(samples.map((sample) => sample.exit_code))], signals: [...new Set(samples.map((sample) => sample.signal))], output_bytes: output.length, o200k_tokens: counted, tokens_saved_vs_raw: rawTokens - counted, token_savings_percent: rawTokens === 0 ? 0 : Number((((rawTokens - counted) / rawTokens) * 100).toFixed(1)), output_hashes: [...new Set(samples.map((sample) => createHash('sha256').update(sample.stdout).update(sample.stderr).digest('hex')))] };
}

const samples = [];
for (const [command, args] of selectedCases) {
  const variants = {
    raw: { file: resolve(settings.windowsBin, `${command}.exe`), args, environment: { Path: windowsPath } },
    native_rtk: { file: settings.nativeRtk, args: [command, ...args], environment: { Path: windowsPath } },
    rtk_wad_wsl1: { file: settings.wad, args: ['--route', 'wsl1', command, ...args], environment: { RTK_WSL_BACKEND: 'wsl1', RTK_WSL_DISTRO: settings.wsl1Distro, RTK_WSL_RTK_PATH: settings.wsl1Rtk, RTK_WSL_EXTRA_PATH: settings.linuxBin, RTK_WAD_NATIVE_RTK_PATH: settings.nativeRtk, Path: windowsPath } },
  };
  for (const [name, variant] of Object.entries(variants)) requireSuccess(await execute(variant), `${command} ${name} warm-up`);
  const entries = Object.entries(variants);
  for (let round = 0; round < settings.rounds; round += 1) {
    const rotated = entries.slice(round % entries.length).concat(entries.slice(0, round % entries.length));
    for (const [name, variant] of rotated) {
      const sample = await execute(variant);
      requireSuccess(sample, `${command} ${name} round ${round + 1}`);
      samples.push({ command, variant: name, round: round + 1, ...sample });
    }
  }
}

const summary = selectedCases.map(([command, args]) => {
  const perVariant = {};
  const rawSamples = samples.filter((sample) => sample.command === command && sample.variant === 'raw');
  const rawTokens = exactTokens(Buffer.concat([rawSamples[0].stdout, rawSamples[0].stderr]));
  for (const name of ['raw', 'native_rtk', 'rtk_wad_wsl1']) perVariant[name] = summarize(samples.filter((sample) => sample.command === command && sample.variant === name), rawTokens);
  const rawFixtureContractPassed = rawSamples.every((sample) => normalized(sample) === expectedRaw(command, args));
  const adapterSamples = samples.filter((sample) => sample.command === command && sample.variant !== 'raw');
  const nativeWsl1ContractPassed = new Set(adapterSamples.map(hashNormalized)).size === 1;
  return { command, raw_fixture_contract_passed: rawFixtureContractPassed, native_wsl1_contract_passed: nativeWsl1ContractPassed, all_variants_succeeded: Object.values(perVariant).every((variant) => variant.exit_codes.length === 1 && variant.exit_codes[0] === 0 && variant.signals.length === 1 && variant.signals[0] === null), variants: perVariant };
});
const allContractsPassed = summary.every((item) => item.raw_fixture_contract_passed && item.native_wsl1_contract_passed && item.all_variants_succeeded);
mkdirSync(dirname(settings.output), { recursive: true });
writeFileSync(settings.output, JSON.stringify({ schema_version: 2, protocol: 'fixture-compatibility-v3', tokenizer: 'o200k_base', tokenizer_package: tokenizerPackage(), preflight: settings.preflight, windows_fixture_bin: settings.windowsBin, wsl1_fixture_bin: settings.linuxBin, wsl1_distro: settings.wsl1Distro, wsl1_rtk: settings.wsl1Rtk, rounds: settings.rounds, fixture_commands: selectedCases.map(([command]) => command), adaptive_policy_eligible: false, adaptive_policy_reason: 'External command fixtures prove structured argv and cross-host adapter compatibility, not safe real-corpus token or latency evidence.', fixture_contracts_passed: allContractsPassed, summary, samples: samples.map(({ stdout, stderr, ...sample }) => ({ ...sample, stdout_sha256: createHash('sha256').update(stdout).digest('hex'), stderr_sha256: createHash('sha256').update(stderr).digest('hex'), stdout_bytes: stdout.length, stderr_bytes: stderr.length })) }, null, 2));
console.log(`Wrote ${settings.output}`);
if (!allContractsPassed) throw new Error('Fixture argv or cross-host adapter contract failed; coverage is not valid');
