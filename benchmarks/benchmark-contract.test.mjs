import assert from "node:assert/strict";
import test from "node:test";
import { adapterContractId, outputHash, outputSizeClass } from "./benchmark-contract.mjs";

test("runtime adapter identity comes from the authoritative manifest", () => {
  assert.match(adapterContractId(), /^rtk:\d+\.\d+\.\d+:protocol-\d+$/);
});

test("output classes are bounded and deterministic", () => {
  assert.equal(outputSizeClass(0), "empty");
  assert.equal(outputSizeClass(1), "small");
  assert.equal(outputSizeClass(4 * 1024), "small");
  assert.equal(outputSizeClass((4 * 1024) + 1), "medium");
  assert.equal(outputSizeClass(64 * 1024), "medium");
  assert.equal(outputSizeClass((64 * 1024) + 1), "large");
  assert.throws(() => outputSizeClass(-1));
});

test("combined output hashing preserves stream boundaries by order", () => {
  assert.notEqual(outputHash(Buffer.from("ab"), Buffer.from("c")), outputHash(Buffer.from("a"), Buffer.from("bc")));
});
