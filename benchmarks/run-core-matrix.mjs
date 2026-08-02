import { spawn, spawnSync } from "node:child_process";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));

function option(name) {
  const index = process.argv.indexOf(name);
  if (index === -1 || !process.argv[index + 1]) throw new Error(`Missing required option ${name}`);
  return process.argv[index + 1];
}

const settings = {
  corpusRoot: resolve(option("--corpus-root")),
  nativeRtk: resolve(option("--native-rtk")),
  xuva: resolve(option("--xuva")),
  python: resolve(option("--python")),
  preflight: resolve(option("--preflight")),
  output: resolve(option("--output")),
  rounds: Number(process.argv.includes("--rounds") ? option("--rounds") : 10),
};
if (!Number.isInteger(settings.rounds) || settings.rounds < 5) {
  throw new Error("--rounds must be an integer of at least 5");
}

const manifestPath = resolve(here, "public-corpora.json");
const manifest = JSON.parse(readFileSync(manifestPath, "utf8"));
if (manifest?.schema_version !== 1 || !Array.isArray(manifest?.corpora) || manifest.corpora.length < 3) {
  throw new Error("public-corpora.json must contain at least three pinned corpora");
}

function git(repo, args) {
  const result = spawnSync(process.env.RTK_WAD_BENCH_GIT || "git.exe", ["-C", repo, ...args], {
    encoding: "utf8",
    windowsHide: true,
  });
  if (result.status !== 0) throw new Error(`Git corpus verification failed for ${repo}: ${result.stderr}`);
  return result.stdout.trim();
}

function normalizedRemote(remote) {
  return remote.trim().replace(/\\/g, "/").replace(/\.git\/?$/i, "").toLowerCase();
}

function runCore(args) {
  return new Promise((resolveRun, reject) => {
    const child = spawn(process.execPath, [resolve(here, "run-core-three-way.mjs"), ...args], {
      stdio: "inherit",
      shell: false,
      windowsHide: true,
    });
    child.on("error", reject);
    child.on("close", (code, signal) => {
      if (code !== 0 || signal !== null) reject(new Error(`core benchmark failed: exit=${code}; signal=${signal}`));
      else resolveRun();
    });
  });
}

const resultDirectory = resolve(dirname(settings.output), "core-matrix-artifacts");
mkdirSync(resultDirectory, { recursive: true });
const artifacts = [];
for (const corpus of manifest.corpora) {
  const benchmark = corpus.core_benchmark;
  if (!benchmark || !Array.isArray(benchmark.search_roots)
    || typeof benchmark.focused_pattern !== "string" || typeof benchmark.broad_pattern !== "string") {
    throw new Error(`${corpus.id} has no complete core_benchmark contract`);
  }
  const repo = resolve(settings.corpusRoot, corpus.id);
  const origin = git(repo, ["remote", "get-url", "origin"]);
  if (normalizedRemote(origin) !== normalizedRemote(corpus.repository)) {
    throw new Error(`${corpus.id} origin is ${origin}, expected ${corpus.repository}`);
  }
  const revision = git(repo, ["rev-parse", "HEAD"]);
  if (revision !== corpus.commit) throw new Error(`${corpus.id} is at ${revision}, expected ${corpus.commit}`);
  if (git(repo, ["status", "--porcelain"]) !== "") throw new Error(`${corpus.id} worktree is not clean`);

  const output = resolve(resultDirectory, `${corpus.id}.json`);
  await runCore([
    "--repo", repo,
    "--native-rtk", settings.nativeRtk,
    "--wad", settings.xuva,
    "--python", settings.python,
    "--preflight", settings.preflight,
    "--output", output,
    "--rounds", String(settings.rounds),
    "--search-roots", benchmark.search_roots.join(","),
    "--focused-pattern", benchmark.focused_pattern,
    "--broad-pattern", benchmark.broad_pattern,
  ]);
  const result = JSON.parse(readFileSync(output, "utf8"));
  if (result?.protocol !== "four-way-core-v3" || !result?.failure_contract) {
    throw new Error(`${corpus.id} did not produce a complete core-v3 artifact`);
  }
  artifacts.push({ id: corpus.id, commit: corpus.commit, output, result });
}

const outputClasses = [...new Set(artifacts.flatMap(({ result }) => result.summaries
  .map(({ variants }) => variants.raw.output_size_class)))].sort();
const coverageValid = artifacts.length === manifest.corpora.length
  && outputClasses.length >= 2
  && artifacts.every(({ result }) => result.failure_contract.raw.exit_code !== 0);
const aggregate = {
  schema_version: 1,
  protocol: "xuva-core-public-corpus-matrix-v1",
  rounds_per_variant: settings.rounds,
  corpus_manifest: manifestPath,
  corpora: artifacts.map(({ id, commit, output, result }) => ({
    id,
    commit,
    artifact: output,
    output_classes: [...new Set(result.summaries.map(({ variants }) => variants.raw.output_size_class))].sort(),
  })),
  observed_raw_output_classes: outputClasses,
  coverage_valid: coverageValid,
  claim_boundary: "Latency samples apply only to the recorded host, binaries, commits, command forms, state, and cache semantics.",
};
mkdirSync(dirname(settings.output), { recursive: true });
writeFileSync(settings.output, JSON.stringify(aggregate, null, 2));
if (!coverageValid) throw new Error("matrix lacks multi-corpus, multi-output-size, or failure-path evidence");
console.log(`Wrote ${settings.output}`);
