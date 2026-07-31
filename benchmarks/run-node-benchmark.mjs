import { spawnSync } from "node:child_process";
import { performance } from "node:perf_hooks";

const gitExe = "C:\\Program Files\\Git\\cmd\\git.exe";
const rtkExe = "C:\\Users\\90174228\\.cargo\\bin\\rtk.exe";
const xuvaExe = "C:\\Users\\90174228\\AppData\\Local\\Programs\\XUVA\\xuva.exe";
const rgExe = "C:\\Users\\90174228\\.cargo\\bin\\rg.exe";
const repoDir = "d:\\luthfi\\project\\rtk-wad";

const workloads = [
  { name: "git status", rawCmd: gitExe, rawArgs: ["status", "--short", "--branch"], rtkArgs: ["git", "status", "--short", "--branch"], xuvaArgs: ["git", "status", "--short", "--branch"] },
  { name: "git log 100", rawCmd: gitExe, rawArgs: ["log", "--oneline", "-100"], rtkArgs: ["git", "log", "--oneline", "-100"], xuvaArgs: ["git", "log", "--oneline", "-100"] },
  { name: "ripgrep focused", rawCmd: rgExe, rawArgs: ["-n", "struct", "src"], rtkArgs: ["rg", "-n", "struct", "src"], xuvaArgs: ["rg", "-n", "struct", "src"] },
  { name: "ripgrep broad", rawCmd: rgExe, rawArgs: ["-n", "pub|fn|struct", "src"], rtkArgs: ["rg", "-n", "pub|fn|struct", "src"], xuvaArgs: ["rg", "-n", "pub|fn|struct", "src"] }
];

console.log("=== HIGH-PRECISION NODE.JS BENCHMARK (spawnSync / performance.now) ===");
console.log(`XUVA Version : ${spawnSync(xuvaExe, ["--version"]).stdout.toString().trim()}`);
console.log(`RTK Version  : ${spawnSync(rtkExe, ["--version"]).stdout.toString().trim()}`);
console.log("Rounds per workload: 10 (1 warmup + 10 measured)\n");

function measure(file, args) {
  // Warmup
  spawnSync(file, args, { cwd: repoDir, windowsHide: true });
  
  const times = [];
  for (let i = 0; i < 10; i++) {
    const t0 = performance.now();
    spawnSync(file, args, { cwd: repoDir, windowsHide: true });
    const t1 = performance.now();
    times.push(t1 - t0);
  }
  times.sort((a, b) => a - b);
  // Median of 10 samples
  return (times[4] + times[5]) / 2;
}

for (const w of workloads) {
  console.log(`Workload: ${w.name}`);
  const rawMs = measure(w.rawCmd, w.rawArgs);
  const rtkMs = measure(rtkExe, w.rtkArgs);
  const xuvaMs = measure(xuvaExe, w.xuvaArgs);

  const diffRaw = xuvaMs - rawMs;
  const diffRtk = xuvaMs - rtkMs;

  console.log(`  Raw Windows Median Latency : ${rawMs.toFixed(2)} ms`);
  console.log(`  Stock RTK Median Latency   : ${rtkMs.toFixed(2)} ms`);
  console.log(`  XUVA Median Latency        : ${xuvaMs.toFixed(2)} ms`);
  console.log(`  Overhead vs Raw            : ${diffRaw >= 0 ? "+" : ""}${diffRaw.toFixed(2)} ms`);
  console.log(`  Overhead vs Stock RTK      : ${diffRtk >= 0 ? "+" : ""}${diffRtk.toFixed(2)} ms\n`);
}
