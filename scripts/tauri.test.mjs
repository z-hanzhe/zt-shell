import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import test from "node:test";
import { createTauriEnvironment } from "./tauri.mjs";

const markers = {
  ZTSHELL_PRIVATE_KEY: "本机测试私钥占位",
  ZTSHELL_PRIVATE_KEY_PASSWORD: "本机测试口令占位",
  ZTSHELL_PRIVATE_KEY_PATH: "本机测试私钥路径占位",
  TAURI_SIGNING_PRIVATE_KEY: "官方测试私钥占位",
  TAURI_SIGNING_PRIVATE_KEY_PASSWORD: "官方测试口令占位",
  TAURI_SIGNING_PRIVATE_KEY_PATH: "官方测试私钥路径占位",
};

/** 使用假凭据执行真实 CLI，只检查输出是否包含这些测试值。 */
function runCli(args) {
  return spawnSync(process.execPath, [fileURLToPath(new URL("./tauri.mjs", import.meta.url)), ...args], {
    encoding: "utf8",
    env: { ...process.env, ...markers },
    timeout: 15000,
  });
}

for (const args of [
  ["signer", "sign", "--help"],
  ["help", "signer", "sign"],
  ["signer", "help", "sign"],
  ["-v", "help", "signer", "sign"],
  ["signer", "sign", "-h"],
  ["signer", "sign", "-vh"],
  ["signer", "sign", "-hv"],
]) {
  test(`CLI 帮助不输出签名凭据：${args.join(" ")}`, () => {
    const result = runCli(args);
    assert.equal(result.status, 0);
    assert.match(result.stdout, /TAURI_SIGNING_PRIVATE_KEY/);
    const output = `${result.stdout}${result.stderr}`;
    for (const marker of Object.values(markers)) assert.equal(output.includes(marker), false);
  });
}

for (const args of [["signer", "sign"], ["signer", "sign", "--unknown-option"], ["signer", "sign", "--help=true"]]) {
  test(`CLI 参数错误不输出签名凭据：${args.join(" ")}`, () => {
    const result = runCli(args);
    assert.notEqual(result.status, 0);
    const output = `${result.stdout}${result.stderr}`;
    for (const marker of Object.values(markers)) assert.equal(output.includes(marker), false);
  });
}

test("帮助、版本、开发、信息查询和跳过签名的构建不继承任何签名变量", () => {
  for (const args of [[], ["--version"], ["-vV"], ["--help"], ["dev"], ["info"], ["completions"], ["signer", "generate", "--help"], ["build", "--no-sign"], ["bundle", "--no-sign"]]) {
    const environment = createTauriEnvironment(args, { ...markers, PUBLIC_SETTING: "保留普通配置" });
    for (const name of Object.keys(markers)) assert.equal(environment[name], undefined);
    assert.equal(environment.PUBLIC_SETTING, "保留普通配置");
  }
});

test("构建和签名命令映射本机凭据但不向子进程传递本机变量别名", () => {
  const source = {
    ZTSHELL_PRIVATE_KEY: markers.ZTSHELL_PRIVATE_KEY,
    ZTSHELL_PRIVATE_KEY_PASSWORD: markers.ZTSHELL_PRIVATE_KEY_PASSWORD,
    PUBLIC_SETTING: "保留普通配置",
  };
  for (const args of [["build"], ["bundle"], ["-vv", "build"], ["signer", "sign", "安装包.exe"]]) {
    const environment = createTauriEnvironment(args, source);
    assert.equal(environment.TAURI_SIGNING_PRIVATE_KEY, source.ZTSHELL_PRIVATE_KEY);
    assert.equal(environment.TAURI_SIGNING_PRIVATE_KEY_PASSWORD, source.ZTSHELL_PRIVATE_KEY_PASSWORD);
    assert.equal(environment.ZTSHELL_PRIVATE_KEY, undefined);
    assert.equal(environment.ZTSHELL_PRIVATE_KEY_PASSWORD, undefined);
    assert.equal(environment.PUBLIC_SETTING, source.PUBLIC_SETTING);
  }
  assert.equal(source.ZTSHELL_PRIVATE_KEY, markers.ZTSHELL_PRIVATE_KEY);
});

test("显式提供的官方签名配置优先，本机环境不会被修改", () => {
  const environment = createTauriEnvironment(["signer", "sign", "安装包.exe"], markers);
  assert.equal(environment.TAURI_SIGNING_PRIVATE_KEY, markers.TAURI_SIGNING_PRIVATE_KEY);
  assert.equal(environment.TAURI_SIGNING_PRIVATE_KEY_PASSWORD, markers.TAURI_SIGNING_PRIVATE_KEY_PASSWORD);
  assert.equal(environment.TAURI_SIGNING_PRIVATE_KEY_PATH, markers.TAURI_SIGNING_PRIVATE_KEY_PATH);
  assert.equal(markers.ZTSHELL_PRIVATE_KEY, "本机测试私钥占位");
});
