import { createHash } from "node:crypto";
import { spawn, spawnSync } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { performance } from "node:perf_hooks";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
function required(name) {
  const index = process.argv.indexOf(name);
  if (index < 0 || !process.argv[index + 1]) throw new Error(`Missing ${name}`);
  return process.argv[index + 1];
}
const settings = {
  nativeRtk: resolve(required("--native-rtk")), wad: resolve(required("--wad")),
  windowsBin: resolve(required("--windows-bin")), linuxBin: required("--linux-bin"),
  python: resolve(required("--python")), output: resolve(required("--output")),
  rounds: Number(process.argv.includes("--rounds") ? required("--rounds") : 5),
};
const cases = [
  ["aws", ["sts", "get-caller-identity"]], ["curl", ["https://fixture.invalid/api"]],
  ["docker", ["ps"]], ["gh", ["repo", "view"]], ["glab", ["repo", "view"]],
  ["kubectl", ["get", "pods"]], ["oc", ["get", "pods"]], ["psql", ["-c", "select 1"]],
  ["wget", ["https://fixture.invalid/archive"]],
];
function execute(file, args, environment) {
  return new Promise((resolveRun, reject) => {
    const start = performance.now(); const child = spawn(file, args, { shell: false, windowsHide: true, env: environment });
    const stdout = []; const stderr = []; child.stdout.on("data", x => stdout.push(x)); child.stderr.on("data", x => stderr.push(x));
    child.on("error", reject); child.on("close", (exit_code, signal) => resolveRun({ elapsed_ms: Number((performance.now() - start).toFixed(3)), exit_code, signal, stdout: Buffer.concat(stdout), stderr: Buffer.concat(stderr) }));
  });
}
function tokens(output) {
  const result = spawnSync(settings.python, [resolve(here, "token-count.py")], { input: output, encoding: "utf8" });
  if (result.status !== 0) throw new Error(result.stderr); return Number(result.stdout.trim());
}
const samples = [];
for (const [command, args] of cases) {
  const variants = {
    raw: [resolve(settings.windowsBin, `${command}.exe`), args, { ...process.env }],
    native_rtk: [settings.nativeRtk, [command, ...args], { ...process.env, Path: `${settings.windowsBin};${process.env.Path}` }],
    rtk_wad_auto: [settings.wad, [command, ...args], { ...process.env, RTK_WSL_EXTRA_PATH: settings.linuxBin, RTK_WAD_NATIVE_RTK_PATH: settings.nativeRtk }],
  };
  for (const [, [file, argv, env]] of Object.entries(variants)) await execute(file, argv, env);
  for (let round = 0; round < settings.rounds; round += 1) for (const name of Object.keys(variants)) {
    const [file, argv, env] = variants[name]; samples.push({ command, variant: name, round: round + 1, ...(await execute(file, argv, env)) });
  }
}
const summary = cases.map(([command]) => {
  const variants = Object.fromEntries(Object.keys({ raw: 0, native_rtk: 0, rtk_wad_auto: 0 }).map(variant => {
  const group = samples.filter(x => x.command === command && x.variant === variant); const representative = Buffer.concat([group[0].stdout, group[0].stderr]);
  return [variant, { exit_codes: [...new Set(group.map(x => x.exit_code))], median_ms: group.map(x => x.elapsed_ms).sort((a, b) => a - b)[Math.floor(group.length / 2)], o200k_tokens: tokens(representative), output_hashes: [...new Set(group.map(x => createHash("sha256").update(x.stdout).update(x.stderr).digest("hex")))] }];
  }));
  return { command, variants };
});
mkdirSync(dirname(settings.output), { recursive: true });
writeFileSync(settings.output, JSON.stringify({ schema_version: 1, protocol: "three-way-fixture-v1", rounds: settings.rounds, summary, samples: samples.map(({ stdout, stderr, ...item }) => ({ ...item, stdout_sha256: createHash("sha256").update(stdout).digest("hex"), stderr_sha256: createHash("sha256").update(stderr).digest("hex") })) }, null, 2));
console.log(`Wrote ${settings.output}`);
