/** 发布入口：版本与说明校验、安装包归档、官方更新清单生成。 */
import { copyFileSync, existsSync, mkdirSync, readFileSync, readdirSync, statSync, writeFileSync } from "node:fs";
import { basename, join, resolve } from "node:path";
import { pathToFileURL } from "node:url";

/** 各目标所需的安装包和更新平台键，优先匹配原安装器类型。 */
export const TARGETS = {
  "x86_64-pc-windows-msvc": { platform: "windows-x86_64", bundles: [
    { suffix: "-setup.exe", keys: ["windows-x86_64", "windows-x86_64-nsis"] },
    { suffix: ".msi", keys: ["windows-x86_64-msi"] },
  ] },
  "aarch64-apple-darwin": { platform: "darwin-aarch64", bundles: [
    { suffix: ".dmg", keys: [] }, { suffix: ".app.tar.gz", keys: ["darwin-aarch64"] },
  ] },
  "x86_64-apple-darwin": { platform: "darwin-x86_64", bundles: [
    { suffix: ".dmg", keys: [] }, { suffix: ".app.tar.gz", keys: ["darwin-x86_64"] },
  ] },
  "x86_64-unknown-linux-gnu": { platform: "linux-x86_64", bundles: [
    { suffix: ".AppImage", keys: ["linux-x86_64", "linux-x86_64-appimage"] },
    { suffix: ".deb", keys: ["linux-x86_64-deb"] }, { suffix: ".rpm", keys: ["linux-x86_64-rpm"] },
  ] },
};

/** 读取并校验本次版本、标签、更新公钥和随代码维护的说明。 */
export function readRelease(root, tag) {
  const readJson = (file) => JSON.parse(readFileSync(join(root, file), "utf8"));
  const pkg = readJson("package.json");
  const lock = readJson("package-lock.json");
  const config = readJson("src-tauri/tauri.conf.json");
  const cargo = readFileSync(join(root, "src-tauri/Cargo.toml"), "utf8").match(/^version\s*=\s*"([^"]+)"/m)?.[1];
  const cargoLock = readFileSync(join(root, "src-tauri/Cargo.lock"), "utf8").match(/\[\[package\]\]\s+name = "zt-shell"\s+version = "([^"]+)"/)?.[1];
  const versions = [pkg.version, lock.version, lock.packages[""].version, config.version, cargo, cargoLock];
  if (versions.some((version) => version !== pkg.version)) throw new Error(`版本号不一致：${versions.join("、")}`);
  if (!/^\d+\.\d+\.\d+$/.test(pkg.version)) throw new Error("正式更新源只发布稳定版本");
  if (tag !== undefined && tag !== pkg.version && tag !== `v${pkg.version}`) throw new Error(`标签须为 ${pkg.version} 或 v${pkg.version}`);
  const notesPath = join(root, "release-notes", `${pkg.version}.md`);
  const notes = readFileSync(notesPath, "utf8").trim();
  if (!notes || !notes.split("\n").some((line) => line.trim() && !line.startsWith("#"))) throw new Error("当前版本缺少有效的更新说明");
  if (config.bundle.createUpdaterArtifacts !== true || !config.plugins?.updater?.pubkey) throw new Error("未启用签名更新产物或未配置公钥");
  return { version: pkg.version, notes, notesPath, pubkey: config.plugins.updater.pubkey };
}

/** 递归收集构建文件，不进入符号链接或将应用目录当成附件。 */
function listFiles(directory) {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const path = join(directory, entry.name);
    return entry.isDirectory() ? listFiles(path) : entry.isFile() ? [path] : [];
  });
}

/** 校验签名格式和签名密钥标识；实际安装前由官方插件验证包内容。 */
export function validateSignature(signature, pubkey) {
  const signatureLines = Buffer.from(signature.trim(), "base64").toString("utf8").trim().split(/\r?\n/);
  const keyLines = Buffer.from(pubkey.trim(), "base64").toString("utf8").trim().split(/\r?\n/);
  const signatureBytes = Buffer.from(signatureLines[1] ?? "", "base64");
  const keyBytes = Buffer.from(keyLines[1] ?? "", "base64");
  if (signatureBytes.length !== 74 || keyBytes.length !== 42 || signatureLines.length !== 4 || Buffer.from(signatureLines[3] ?? "", "base64").length !== 64) {
    throw new Error("更新签名或公钥格式不正确");
  }
  if (!signatureBytes.subarray(2, 10).equals(keyBytes.subarray(2, 10))) throw new Error("更新签名使用的私钥与应用内公钥不匹配");
}

/** 统一附件名称，避免两个 macOS 架构的应用归档相互覆盖。 */
function assetName(version, platform, suffix) {
  return `ZTShell_${version}_${platform}${suffix}`;
}

/** 只归档本目标需要的安装包和签名，缺失或重复时终止发布。 */
export function stageAssets(root, target, destination) {
  const spec = TARGETS[target];
  if (!spec) throw new Error(`不支持的构建目标：${target}`);
  const release = readRelease(root);
  const files = listFiles(join(root, "src-tauri", "target", target, "release", "bundle"));
  mkdirSync(destination, { recursive: true });
  for (const bundle of spec.bundles) {
    const matches = files.filter((file) => file.endsWith(bundle.suffix));
    if (matches.length !== 1) throw new Error(`${target} 的 ${bundle.suffix} 安装包数量应为 1，实际为 ${matches.length}`);
    const source = matches[0];
    if (statSync(source).size === 0) throw new Error(`安装包为空：${basename(source)}`);
    const output = join(destination, assetName(release.version, spec.platform, bundle.suffix));
    copyFileSync(source, output);
    if (bundle.keys.length) {
      const signature = readFileSync(`${source}.sig`, "utf8");
      validateSignature(signature, release.pubkey);
      copyFileSync(`${source}.sig`, `${output}.sig`);
    }
  }
}

/** 汇总所有平台的签名与固定标签下载地址，生成 Tauri 静态清单。 */
export function createManifest(root, directory, repository, tag, now = new Date()) {
  const release = readRelease(root, tag);
  if (!/^[\w.-]+\/[\w.-]+$/.test(repository)) throw new Error("GitHub 仓库名称格式不正确");
  const platforms = {};
  for (const spec of Object.values(TARGETS)) {
    for (const bundle of spec.bundles) {
      const file = assetName(release.version, spec.platform, bundle.suffix);
      const path = join(directory, file);
      if (!existsSync(path) || statSync(path).size === 0) throw new Error(`发布附件缺失或为空：${file}`);
      if (!bundle.keys.length) continue;
      const signature = readFileSync(`${path}.sig`, "utf8").trim();
      validateSignature(signature, release.pubkey);
      const entry = { signature, url: `https://github.com/${repository}/releases/download/${encodeURIComponent(tag)}/${encodeURIComponent(file)}` };
      for (const key of bundle.keys) platforms[key] = entry;
    }
  }
  const manifest = { version: release.version, notes: release.notes, pub_date: now.toISOString(), platforms };
  writeFileSync(join(directory, "latest.json"), `${JSON.stringify(manifest, null, 2)}\n`);
  return manifest;
}

/** 执行发布工作流指定的阶段，任何缺失都在公开 Release 前报错。 */
function main() {
  const [command, ...args] = process.argv.slice(2);
  const root = process.cwd();
  if (command === "validate") {
    const release = readRelease(root, args[0]);
    console.log(`版本与更新说明校验通过：${release.version}`);
  } else if (command === "stage") {
    stageAssets(root, args[0], resolve(args[1]));
  } else if (command === "manifest") {
    createManifest(root, resolve(args[0]), args[1], args[2]);
  } else {
    throw new Error("请指定 validate、stage 或 manifest 发布阶段");
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) main();
