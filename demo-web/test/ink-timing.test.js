import assert from "node:assert/strict";
import test from "node:test";
import {
  ANSWER_HOLD_MS,
  IDLE_SUBMIT_MS,
  characterDelay,
  isDismissSwipe
} from "../public/ink-timing.js";

test("uses the approved idle and reading durations", () => {
  assert.equal(IDLE_SUBMIT_MS, 3000);
  assert.equal(ANSWER_HOLD_MS, 12000);
});

test("pauses longer at punctuation than ordinary characters", () => {
  assert.ok(characterDelay("。") > characterDelay("，"));
  assert.ok(characterDelay("，") > characterDelay("纸"));
  assert.ok(characterDelay("纸") > characterDelay(" "));
});

test("dismisses only a deliberate swipe", () => {
  assert.equal(isDismissSwipe({ x: 10, y: 10 }, { x: 25, y: 22 }), false);
  assert.equal(isDismissSwipe({ x: 10, y: 10 }, { x: 70, y: 14 }), true);
});
