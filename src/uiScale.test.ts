// @ts-nocheck

import assert from "node:assert/strict";
import test from "node:test";

import { DEFAULT_UI_SCALE, normalizeUiScale } from "./uiScale.ts";

test("界面缩放对无效输入使用默认值", () => {
  assert.equal(normalizeUiScale(undefined), DEFAULT_UI_SCALE);
  assert.equal(normalizeUiScale(null), DEFAULT_UI_SCALE);
  assert.equal(normalizeUiScale(""), DEFAULT_UI_SCALE);
  assert.equal(normalizeUiScale("invalid"), DEFAULT_UI_SCALE);
  assert.equal(normalizeUiScale(Number.NaN), DEFAULT_UI_SCALE);
});

test("界面缩放限制在支持范围内并保留百分比精度", () => {
  assert.equal(normalizeUiScale(0.5), 0.8);
  assert.equal(normalizeUiScale(1.249), 1.25);
  assert.equal(normalizeUiScale("1.5"), 1.5);
  assert.equal(normalizeUiScale(3), 2);
});
