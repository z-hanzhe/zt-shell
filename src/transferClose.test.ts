// @ts-nocheck

import assert from "node:assert/strict";
import test from "node:test";

import { prepareTransferClose, runTransferCreation } from "./transferClose.ts";

/** 创建可由测试控制完成时机的 Promise */
function deferred() {
  let resolve;
  let reject;
  const promise = new Promise((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
}

test("关闭准备会等待已进入 API 边界的传输创建请求", async () => {
  const pending = deferred();
  const creation = runTransferCreation("session-wait", () => pending.promise);
  let prepared = false;
  const preparationTask = prepareTransferClose(["session-wait"]).then((value) => {
    prepared = true;
    return value;
  });

  await Promise.resolve();
  assert.equal(prepared, false);
  pending.resolve("created");
  assert.equal(await creation, "created");

  const preparation = await preparationTask;
  assert.equal(prepared, true);
  await preparation.release();
});

test("关闭准备期间拒绝新传输，释放后恢复创建", async () => {
  const preparation = await prepareTransferClose(["session-blocked"]);
  let invoked = false;

  await assert.rejects(
    runTransferCreation("session-blocked", async () => {
      invoked = true;
    }),
    /会话正在关闭/
  );
  assert.equal(invoked, false);

  await preparation.release();
  assert.equal(
    await runTransferCreation("session-blocked", async () => "created"),
    "created"
  );
});

test("重叠关闭准备按引用计数释放传输创建门闩", async () => {
  const first = await prepareTransferClose(["session-overlap"]);
  const second = await prepareTransferClose(["session-overlap"]);

  await first.release();
  await assert.rejects(
    runTransferCreation("session-overlap", async () => undefined),
    /会话正在关闭/
  );

  await second.release();
  await runTransferCreation("session-overlap", async () => undefined);
});
