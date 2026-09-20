import assert from "node:assert/strict";
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { createManifest, readRelease, stageAssets, validateSignature } from "./release.mjs";

/** 构造包含四个平台、真实格式签名和独立临时目录的发布输入。 */
function fixture(t) {
  const root = mkdtempSync(join(tmpdir(), "ztshell-release-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  mkdirSync(join(root, "src-tauri"));
  mkdirSync(join(root, "release-notes"));
  const key = Buffer.alloc(42);
  key.write("Ed");
  key.fill(7, 2, 10);
  const signatureBytes = Buffer.alloc(74);
  signatureBytes.write("ED");
  key.copy(signatureBytes, 2, 2, 10);
  const pubkey = Buffer.from(`untrusted comment: 测试公钥\n${key.toString("base64")}\n`).toString("base64");
  const signature = Buffer.from(`untrusted comment: 测试签名\n${signatureBytes.toString("base64")}\ntrusted comment: 测试数据\n${Buffer.alloc(64).toString("base64")}\n`).toString("base64");
  writeFileSync(join(root, "package.json"), JSON.stringify({ version: "1.2.3" }));
  writeFileSync(join(root, "package-lock.json"), JSON.stringify({ version: "1.2.3", packages: { "": { version: "1.2.3" } } }));
  writeFileSync(join(root, "src-tauri/Cargo.toml"), '[package]\nname = "zt-shell"\nversion = "1.2.3"\n');
  writeFileSync(join(root, "src-tauri/Cargo.lock"), '[[package]]\nname = "zt-shell"\nversion = "1.2.3"\n');
  writeFileSync(join(root, "src-tauri/tauri.conf.json"), JSON.stringify({ version: "1.2.3", bundle: { createUpdaterArtifacts: true }, plugins: { updater: { pubkey } } }));
  writeFileSync(join(root, "release-notes/1.2.3.md"), "# 更新说明\n\n修复连接问题。\n");
  const bundles = {
    "x86_64-pc-windows-msvc": ["ZTShell_1.2.3_x64-setup.exe", "ZTShell_1.2.3_x64_en-US.msi"],
    "aarch64-apple-darwin": ["ZTShell_1.2.3_aarch64.dmg", "ZTShell.app.tar.gz"],
    "x86_64-apple-darwin": ["ZTShell_1.2.3_x64.dmg", "ZTShell.app.tar.gz"],
    "x86_64-unknown-linux-gnu": ["ZTShell_1.2.3_amd64.AppImage", "ZTShell_1.2.3_amd64.deb", "ZTShell-1.2.3-1.x86_64.rpm"],
  };
  for (const [target, names] of Object.entries(bundles)) {
    const directory = join(root, "src-tauri/target", target, "release/bundle");
    mkdirSync(directory, { recursive: true });
    for (const name of names) {
      writeFileSync(join(directory, name), `${target} 安装包`);
      if (!name.endsWith(".dmg")) writeFileSync(join(directory, `${name}.sig`), signature);
    }
  }
  const assets = join(root, "assets");
  for (const target of Object.keys(bundles)) stageAssets(root, target, assets);
  return { root, assets, pubkey, signature };
}

test("汇总完整平台，保留安装器类型、固定版本地址和同一份说明", (t) => {
  const { root, assets } = fixture(t);
  const manifest = createManifest(root, assets, "owner/repository", "v1.2.3", new Date("2026-01-01T00:00:00Z"));
  assert.deepEqual(Object.keys(manifest.platforms).sort(), ["windows-x86_64", "windows-x86_64-nsis", "windows-x86_64-msi", "darwin-aarch64", "darwin-x86_64", "linux-x86_64", "linux-x86_64-appimage", "linux-x86_64-deb", "linux-x86_64-rpm"].sort());
  assert.match(manifest.platforms["windows-x86_64-msi"].url, /\/v1\.2\.3\/.*\.msi$/);
  assert.match(manifest.platforms["linux-x86_64-deb"].url, /\.deb$/);
  assert.notEqual(manifest.platforms["darwin-aarch64"].url, manifest.platforms["darwin-x86_64"].url);
  assert.equal(manifest.notes, readRelease(root).notes);
  assert.equal(JSON.parse(readFileSync(join(assets, "latest.json"), "utf8")).version, "1.2.3");
});

test("任何平台缺少附件或签名时禁止生成正式更新清单", (t) => {
  const { root, assets } = fixture(t);
  rmSync(join(assets, "ZTShell_1.2.3_darwin-aarch64.app.tar.gz.sig"));
  assert.throws(() => createManifest(root, assets, "owner/repo", "v1.2.3"));
});

test("标签、版本、更新说明必须对应", (t) => {
  const { root } = fixture(t);
  assert.throws(() => readRelease(root, "v1.2.4"), /标签/);
  writeFileSync(join(root, "release-notes/1.2.3.md"), "# 仅有标题");
  assert.throws(() => readRelease(root), /更新说明/);
  writeFileSync(join(root, "src-tauri/Cargo.toml"), 'version = "1.2.4"');
  assert.throws(() => readRelease(root), /版本号不一致/);
});

test("错误私钥和损坏签名在发布前被拒绝", (t) => {
  const { pubkey, signature } = fixture(t);
  validateSignature(signature, pubkey);
  assert.throws(() => validateSignature("损坏的签名", pubkey), /格式/);
  const lines = Buffer.from(pubkey, "base64").toString().split("\n");
  const key = Buffer.from(lines[1], "base64");
  key[2] ^= 1;
  lines[1] = key.toString("base64");
  assert.throws(() => validateSignature(signature, Buffer.from(lines.join("\n")).toString("base64")), /不匹配/);
});
