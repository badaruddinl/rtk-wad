import { spawn, spawnSync } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { performance } from "node:perf_hooks";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const value = (name) => { const index = process.argv.indexOf(name); if (index < 0 || !process.argv[index + 1]) throw new Error(`Missing ${name}`); return process.argv[index + 1]; };
const settings = { repo: resolve(value("--repo")), nativeRtk: resolve(value("--native-rtk")), wad: resolve(value("--wad")), python: resolve(value("--python")), target: resolve(value("--target-dir")), output: resolve(value("--output")), rounds: Number(process.argv.includes("--rounds") ? value("--rounds") : 5) };
const rawCargo = process.env.RTK_WAD_BENCH_CARGO || "cargo.exe";
function run(file, args) { return new Promise((done, fail) => { const start = performance.now(); const child = spawn(file, args, { cwd: settings.repo, shell: false, windowsHide: true, env: { ...process.env, CARGO_TARGET_DIR: settings.target, RTK_WAD_NATIVE_RTK_PATH: settings.nativeRtk } }); const stdout = []; const stderr = []; child.stdout.on("data", x => stdout.push(x)); child.stderr.on("data", x => stderr.push(x)); child.on("error", fail); child.on("close", (exit_code) => done({ elapsed_ms: performance.now() - start, exit_code, output: Buffer.concat([...stdout, ...stderr]) })); }); }
function tokens(output) { const result = spawnSync(settings.python, [resolve(here, "token-count.py")], { input: output, encoding: "utf8" }); if (result.status !== 0) throw new Error(result.stderr); return Number(result.stdout.trim()); }
const variants = { raw: [rawCargo, ["check"]], native_rtk: [settings.nativeRtk, ["cargo", "check"]], rtk_wad_auto: [settings.wad, ["cargo", "check"]] };
const samples = [];
for (const [name, [file, args]] of Object.entries(variants)) await run(file, args);
for (let round = 0; round < settings.rounds; round += 1) for (const [name, [file, args]] of Object.entries(variants)) samples.push({ variant: name, round: round + 1, ...(await run(file, args)) });
const summary = Object.fromEntries(Object.keys(variants).map(name => { const group = samples.filter(sample => sample.variant === name); const output = group[0].output; return [name, { median_ms: group.map(sample => sample.elapsed_ms).sort((a, b) => a - b)[Math.floor(group.length / 2)], exit_codes: [...new Set(group.map(sample => sample.exit_code))], o200k_tokens: tokens(output), runs: group.length }]; }));
const policy = { schema_version: 1, evidence: [{ key: "cargo:check", raw_median_ms: summary.raw.median_ms, candidate_median_ms: summary.rtk_wad_auto.median_ms, token_savings_percent: summary.raw.o200k_tokens === 0 ? 0 : ((summary.raw.o200k_tokens - summary.rtk_wad_auto.o200k_tokens) / summary.raw.o200k_tokens) * 100, sample_count: summary.raw.runs }] };
mkdirSync(dirname(settings.output), { recursive: true }); writeFileSync(settings.output, JSON.stringify({ schema_version: 1, protocol: "three-way-cargo-check-v1", summary, samples: samples.map(({ output, ...sample }) => ({ ...sample, output_bytes: output.length })) }, null, 2)); writeFileSync(settings.output.replace(/\.json$/i, ".route-policy.json"), JSON.stringify(policy, null, 2)); console.log(`Wrote ${settings.output}`);
