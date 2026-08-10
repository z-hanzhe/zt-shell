// @ts-nocheck

import assert from "node:assert/strict";
import test from "node:test";

import {
  confirmSessionClose,
  countUnfinishedTransferTasks,
  isLiveSessionStatus,
} from "./sessionClose.ts";

/** 构造关闭保护所需的最小传输任务 */
function transfer(overrides = {}) {
  return {
    id: "task-default",
    parentId: null,
    sessionId: "session-live",
    status: "running",
    ...overrides,
  };
}

test("连接状态只包含连接生命周期中的三种状态", () => {
  assert.equal(isLiveSessionStatus("connecting"), true);
  assert.equal(isLiveSessionStatus("verifying"), true);
  assert.equal(isLiveSessionStatus("connected"), true);
  assert.equal(isLiveSessionStatus("disconnected"), false);
  assert.equal(isLiveSessionStatus("error"), false);
});

test("关闭保护严格按连接、未保存编辑、未完成传输的顺序执行", async () => {
  const calls = [];
  const result = await confirmSessionClose({
    getLiveSessionIds: () => ["session-live"],
    confirmLiveSessions: async () => {
      calls.push("连接确认");
      return true;
    },
    queryUnsavedDocuments: async () => {
      calls.push("检测编辑");
      return 2;
    },
    confirmUnsavedDocuments: async () => {
      calls.push("编辑确认");
      return true;
    },
    queryTransfers: async () => {
      calls.push("检测传输");
      return [transfer()];
    },
    confirmUnfinishedTransfers: async () => {
      calls.push("传输确认");
      return true;
    },
  });

  assert.equal(result, true);
  assert.deepEqual(calls, ["连接确认", "检测编辑", "编辑确认", "检测传输", "传输确认"]);
});

test("取消未保存编辑确认后不再检测传输", async () => {
  let queriedTransfers = false;
  const result = await confirmSessionClose({
    getLiveSessionIds: () => ["session-live"],
    confirmLiveSessions: async () => true,
    queryUnsavedDocuments: async () => 1,
    confirmUnsavedDocuments: async () => false,
    queryTransfers: async () => {
      queriedTransfers = true;
      return [];
    },
    confirmUnfinishedTransfers: async () => true,
  });

  assert.equal(result, false);
  assert.equal(queriedTransfers, false);
});

test("取消现有连接确认后不执行资源查询", async () => {
  let queriedUnsaved = false;
  const result = await confirmSessionClose({
    getLiveSessionIds: () => ["session-live"],
    confirmLiveSessions: async () => false,
    queryUnsavedDocuments: async () => {
      queriedUnsaved = true;
      return 1;
    },
    confirmUnsavedDocuments: async () => true,
    queryTransfers: async () => [],
    confirmUnfinishedTransfers: async () => true,
  });

  assert.equal(result, false);
  assert.equal(queriedUnsaved, false);
});

test("取消传输确认会中止关闭流程", async () => {
  const result = await confirmSessionClose({
    getLiveSessionIds: () => ["session-live"],
    confirmLiveSessions: async () => true,
    queryUnsavedDocuments: async () => 0,
    confirmUnsavedDocuments: async () => true,
    queryTransfers: async () => [transfer()],
    confirmUnfinishedTransfers: async () => false,
  });

  assert.equal(result, false);
});

test("已断开会话不触发任何关闭提示或状态查询", async () => {
  let called = false;
  const unexpected = async () => {
    called = true;
    return true;
  };
  const result = await confirmSessionClose({
    getLiveSessionIds: () => [],
    confirmLiveSessions: unexpected,
    queryUnsavedDocuments: unexpected,
    confirmUnsavedDocuments: unexpected,
    queryTransfers: async () => {
      called = true;
      return [];
    },
    confirmUnfinishedTransfers: unexpected,
  });

  assert.equal(result, true);
  assert.equal(called, false);
});

test("首次确认期间断线时跳过后续风险提示", async () => {
  let liveCheck = 0;
  let queriedUnsaved = false;
  const result = await confirmSessionClose({
    getLiveSessionIds: () => (++liveCheck === 1 ? ["session-live"] : []),
    confirmLiveSessions: async () => true,
    queryUnsavedDocuments: async () => {
      queriedUnsaved = true;
      return 1;
    },
    confirmUnsavedDocuments: async () => true,
    queryTransfers: async () => [],
    confirmUnfinishedTransfers: async () => true,
  });

  assert.equal(result, true);
  assert.equal(queriedUnsaved, false);
});

test("未完成传输包含失败和暂停任务，并排除完成、取消、子任务与其他会话", () => {
  const tasks = [
    transfer({ id: "pending", status: "pending" }),
    transfer({ id: "running", status: "running" }),
    transfer({ id: "packing", status: "packing" }),
    transfer({ id: "paused", status: "paused" }),
    transfer({ id: "failed", status: "failed" }),
    transfer({ id: "completed", status: "completed" }),
    transfer({ id: "cancelled", status: "cancelled" }),
    transfer({ id: "child", parentId: "running", status: "running" }),
    transfer({ id: "other", sessionId: "session-other", status: "running" }),
  ];

  assert.equal(countUnfinishedTransferTasks(tasks, ["session-live"]), 5);
});
