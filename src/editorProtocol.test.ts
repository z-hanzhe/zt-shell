// @ts-nocheck

import assert from "node:assert/strict";
import test from "node:test";

import {
  DEFAULT_EDITOR_FONT_SIZE,
  MAX_EDITOR_FONT_SIZE,
  MIN_EDITOR_FONT_SIZE,
  normalizeEditorFontSize,
} from "./editorProtocol.ts";

test("编辑器字号对空值和无效输入使用默认值", () => {
  assert.equal(normalizeEditorFontSize(undefined), DEFAULT_EDITOR_FONT_SIZE);
  assert.equal(normalizeEditorFontSize(null), DEFAULT_EDITOR_FONT_SIZE);
  assert.equal(normalizeEditorFontSize(""), DEFAULT_EDITOR_FONT_SIZE);
  assert.equal(normalizeEditorFontSize("invalid"), DEFAULT_EDITOR_FONT_SIZE);
});

test("编辑器字号取整并限制在设置允许范围内", () => {
  assert.equal(normalizeEditorFontSize(4), MIN_EDITOR_FONT_SIZE);
  assert.equal(normalizeEditorFontSize(14.6), 15);
  assert.equal(normalizeEditorFontSize("18"), 18);
  assert.equal(normalizeEditorFontSize(80), MAX_EDITOR_FONT_SIZE);
});
