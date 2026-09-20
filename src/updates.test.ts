// @ts-nocheck

import assert from "node:assert/strict";
import { registerHooks } from "node:module";
import test from "node:test";
import { createPinia, setActivePinia } from "pinia";
import { mockIPC } from "@tauri-apps/api/mocks";

registerHooks({
  /** 兼容 Vite 使用的无扩展名相对导入。 */
  resolve(specifier, context, nextResolve) {
    try { return nextResolve(specifier, context); }
    catch (error) {
      if (error.code === "ERR_MODULE_NOT_FOUND" && specifier.startsWith(".")) return nextResolve(`${specifier}.ts`, context);
      throw error;
    }
  },
});
const { useUpdatesStore, normalizeUpdatePreferences } = await import("./stores/updates.ts");

/** 创建隔离的更新环境，不发出网络请求或运行安装器。 */
function setup(saved = new Map(), handler = () => undefined, secrets = new Map()) {
  setActivePinia(createPinia());
  globalThis.window = { crypto: globalThis.crypto };
  globalThis.isTauri = true;
  const calls = [];
  const remote = { value: { version: "0.2.8", currentVersion: "0.2.7", notes: "更新说明", date: null } };
  mockIPC(async (command, args) => {
    calls.push({ command, args });
    const custom = handler(command, args);
    if (custom !== undefined) return custom;
    if (command === "updater_environment") return { version: "0.2.7", canInstall: true };
    if (command === "updater_check") return remote.value;
    if (command === "plugin:store|load") return 1;
    if (command === "plugin:store|get") return [saved.get(args.key), saved.has(args.key)];
    if (command === "plugin:store|set") saved.set(args.key, structuredClone(args.value));
    if (command === "credentials_set_many") {
      for (const change of args.changes) secrets.set(change.id, change.value);
    }
    if (command === "credentials_delete_many") {
      for (const key of args.keys) secrets.delete(key.id);
    }
  });
  return { store: useUpdatesStore(), saved, calls, remote, secrets };
}

test("启动并发初始化只检查一次，并使用保存的代理", async () => {
  const saved = new Map([["preferences", { source: "github", useProxy: true, proxyUrl: "http://127.0.0.1:8080" }]]);
  const { store, calls } = setup(saved);
  await Promise.all([store.init(), store.init(), store.init()]);
  const checks = calls.filter((call) => call.command === "updater_check");
  assert.equal(checks.length, 1);
  assert.deepEqual(checks[0].args.proxy, { url: "http://127.0.0.1:8080", username: "", credentialId: null });
  assert.equal(store.hasUpdate, true);
  assert.equal(calls.some((call) => call.command === "updater_download"), false);
});

test("跳过记录跨重启保留，仅下一版本恢复红点", async () => {
  const first = setup();
  await first.store.init();
  await first.store.skip();
  assert.equal(first.store.hasUpdate, false);
  const second = setup(first.saved);
  await second.store.init();
  assert.equal(second.store.hasUpdate, false);
  await second.store.check();
  assert.equal(second.store.hasUpdate, false);
  second.remote.value = { ...second.remote.value, version: "0.2.9" };
  await second.store.check();
  assert.equal(second.store.hasUpdate, true);
  second.remote.value = null;
  await second.store.check();
  assert.equal(second.store.hasUpdate, false);
});

test("检查失败保留已有提醒，启动检查失败只记录错误", async () => {
  let fail = false;
  const { store } = setup(new Map(), (command) => {
    if (command === "updater_check" && fail) return Promise.reject(new Error("网络不可用"));
  });
  await store.init();
  fail = true;
  await store.check();
  assert.equal(store.hasUpdate, true);
  assert.match(store.error, /网络不可用/);
  assert.equal(store.phase, "idle");
  const startup = setup(new Map(), (command) => command === "updater_check" ? Promise.reject(new Error("离线")) : undefined);
  await startup.store.init();
  assert.match(startup.store.error, /离线/);
  assert.equal(startup.store.hasUpdate, false);
});

test("保存网络设置不会检查更新，手动检查时使用新配置", async () => {
  const { store, calls } = setup();
  await store.init();
  assert.equal(calls.find((call) => call.command === "updater_check").args.proxy, null);
  await store.savePreferences({ source: "github", useProxy: true, proxyUrl: " http://localhost:1234 " });
  assert.equal(calls.filter((call) => call.command === "updater_check").length, 1);
  assert.equal(store.downloadReady, false);
  assert.equal(store.lastChecked, "");
  await store.check();
  assert.deepEqual(calls.filter((call) => call.command === "updater_check").at(-1).args.proxy, {
    url: "http://localhost:1234", username: "", credentialId: null,
  });
});

test("切换代理后检查失败保留红点，但禁止使用旧代理下载", async () => {
  let fail = false;
  const { store, calls } = setup(new Map(), (command) => {
    if (command === "updater_check" && fail) return Promise.reject(new Error("代理不可用"));
  });
  await store.init();
  fail = true;
  await store.savePreferences({ source: "github", useProxy: true, proxyUrl: "http://localhost:1234" });
  await store.check();
  assert.equal(store.hasUpdate, true);
  assert.equal(store.downloadReady, false);
  await store.download();
  assert.equal(calls.some((call) => call.command === "updater_download"), false);
  fail = false;
  await store.check();
  assert.equal(store.downloadReady, true);
});

test("下载期间合并重复操作，校验成功后等待用户安装", async () => {
  let finish;
  const deferred = new Promise((resolve) => { finish = resolve; });
  const { store, calls } = setup(new Map(), (command) => command === "updater_download" ? deferred : undefined);
  await store.init();
  const download = store.download();
  assert.equal(store.phase, "downloading");
  await store.download();
  await store.check();
  await store.skip();
  assert.equal(calls.filter((call) => call.command === "updater_download").length, 1);
  assert.equal(calls.filter((call) => call.command === "updater_check").length, 1);
  finish();
  await download;
  assert.equal(store.phase, "ready");
  assert.equal(calls.some((call) => call.command === "updater_install"), false);
  await store.skip();
  assert.equal(store.hasUpdate, false);
  await store.install();
  assert.equal(store.phase, "installing");
});

test("签名校验失败不能进入安装状态，安装失败仍可重试", async () => {
  let failDownload = true;
  const { store } = setup(new Map(), (command) => {
    if (command === "updater_download" && failDownload) return Promise.reject(new Error("签名不匹配"));
    if (command === "updater_install") return Promise.reject(new Error("权限不足"));
  });
  await store.init();
  await store.download();
  assert.equal(store.phase, "idle");
  await assert.rejects(store.install(), /请先下载更新/);
  failDownload = false;
  await store.download();
  await assert.rejects(store.install(), /权限不足/);
  assert.equal(store.phase, "ready");
});

test("跳过记录保存失败不撤去红点", async () => {
  const { store } = setup(new Map(), (command) => command === "plugin:store|save" ? Promise.reject(new Error("磁盘不可写")) : undefined);
  await store.init();
  await store.skip();
  assert.equal(store.hasUpdate, true);
  assert.match(store.error, /磁盘不可写/);
});

test("代理密码只写入凭据库，重新启动检查只传密码引用", async () => {
  const first = setup();
  await first.store.init();
  await first.store.savePreferences({
    ...first.store.preferences, useProxy: true, proxyUsername: "代理用户@/%", password: "不应进入偏好",
  }, { mode: "set", value: "测试密码: @/%" });
  const id = first.store.preferences.proxyCredentialId;
  assert.match(id, /^updater-proxy:/);
  assert.equal(first.secrets.get(id), "测试密码: @/%");
  assert.equal(JSON.stringify([...first.saved]).includes("测试密码"), false);
  assert.equal(JSON.stringify([...first.saved]).includes("不应进入偏好"), false);
  const second = setup(first.saved, () => undefined, first.secrets);
  await second.store.init();
  assert.equal(second.store.preferences.proxyCredentialId, id);
  const check = second.calls.find((call) => call.command === "updater_check");
  assert.equal(check.args.proxy.username, "代理用户@/%");
  assert.equal(check.args.proxy.credentialId, id);
  assert.equal(JSON.stringify(check.args).includes("测试密码"), false);
  await second.store.savePreferences({ ...second.store.preferences, useProxy: false });
  await second.store.check();
  assert.equal(second.calls.filter((call) => call.command === "updater_check").at(-1).args.proxy, null);
  assert.equal(second.secrets.get(id), "测试密码: @/%");
});

test("密码替换失败保留旧凭据和旧偏好，清除成功后删除凭据", async () => {
  let fail = false;
  const { store, saved, secrets } = setup(new Map(), (command) => {
    if (command === "plugin:store|save" && fail) return Promise.reject(new Error("磁盘不可写"));
  });
  await store.init();
  await store.savePreferences({ ...store.preferences, proxyUsername: "用户" }, { mode: "set", value: "原密码" });
  const previous = { ...store.preferences };
  fail = true;
  await assert.rejects(store.savePreferences({ ...store.preferences, proxyUrl: "http://localhost:8000" }, { mode: "set", value: "新密码" }), /磁盘不可写/);
  assert.deepEqual(store.preferences, previous);
  assert.deepEqual(saved.get("preferences"), previous);
  assert.deepEqual([...secrets], [[previous.proxyCredentialId, "原密码"]]);
  fail = false;
  await store.savePreferences({ ...store.preferences, proxyUsername: "" }, { mode: "clear" });
  assert.equal(store.preferences.proxyCredentialId, null);
  assert.equal(saved.get("preferences").proxyCredentialId, null);
  assert.equal(secrets.size, 0);
});

test("代理密码必须关联用户名，更新引用不能由表单伪造", async () => {
  const { store, secrets } = setup();
  await store.init();
  await assert.rejects(store.savePreferences(store.preferences, { mode: "set", value: "密码" }), /代理用户名/);
  assert.equal(secrets.size, 0);
  await store.savePreferences({ ...store.preferences, proxyUsername: "用户名", proxyCredentialId: "外部凭据" });
  assert.equal(store.preferences.proxyCredentialId, null);
});

test("拒绝含密码、路径或不受支持协议的代理地址", () => {
  for (const proxyUrl of ["localhost:8080", "http://user:secret@host", "http://host/path", "http://host?token=secret", "file:///tmp", ""]) {
    assert.throws(() => normalizeUpdatePreferences({ source: "github", useProxy: true, proxyUrl }));
  }
});
