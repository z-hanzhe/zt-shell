/** 将签名凭据限定在需要它们的官方 CLI 命令中，不读取或生成私钥文件。 */
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";

const SIGNING_VARIABLES = [
  "ZTSHELL_PRIVATE_KEY",
  "ZTSHELL_PRIVATE_KEY_PASSWORD",
  "ZTSHELL_PRIVATE_KEY_PATH",
  "TAURI_SIGNING_PRIVATE_KEY",
  "TAURI_SIGNING_PRIVATE_KEY_PASSWORD",
  "TAURI_SIGNING_PRIVATE_KEY_PATH",
];

/** 创建命令专用环境，帮助查询与非签名命令均不继承签名凭据。 */
export function createTauriEnvironment(args, source) {
  const environment = { ...source };
  for (const name of SIGNING_VARIABLES) delete environment[name];

  // Clap 同时支持 help 子命令和 -vh 等组合短参数，不能只检查 --help。
  const informationOnly = args.some((arg) =>
    arg === "help" || /^--(?:help|version)(?:=|$)/.test(arg) || /^-[vVh]*[hV][vVh]*$/.test(arg)
  );
  const commands = args.filter((arg) => !arg.startsWith("-"));
  const signingCommand =
    (["build", "bundle"].includes(commands[0]) && !args.includes("--no-sign")) ||
    (commands[0] === "signer" && commands[1] === "sign");
  if (informationOnly || !signingCommand) return environment;

  for (const suffix of ["PRIVATE_KEY", "PRIVATE_KEY_PASSWORD"]) {
    const target = `TAURI_SIGNING_${suffix}`;
    const value = source[target] ?? source[`ZTSHELL_${suffix}`];
    if (value !== undefined) environment[target] = value;
  }
  if (source.TAURI_SIGNING_PRIVATE_KEY_PATH !== undefined) {
    environment.TAURI_SIGNING_PRIVATE_KEY_PATH = source.TAURI_SIGNING_PRIVATE_KEY_PATH;
  }
  return environment;
}

/** 在加载原生 CLI 前收紧环境，并对返回到包装层的错误消息隐藏凭据。 */
async function main() {
  const args = process.argv.slice(2);
  const sensitiveValues = [...new Set(SIGNING_VARIABLES.map((name) => process.env[name]).filter(Boolean))]
    .sort((left, right) => right.length - left.length);
  const environment = createTauriEnvironment(args, process.env);
  for (const name of SIGNING_VARIABLES) {
    if (environment[name] === undefined) delete process.env[name];
    else process.env[name] = environment[name];
  }

  try {
    const { default: cli } = await import("@tauri-apps/cli");
    await cli.run(args, "npm run tauri");
  } catch (error) {
    let message = error instanceof Error ? error.message : String(error);
    for (const value of sensitiveValues) message = message.replaceAll(value, "[签名凭据已隐藏]");
    console.error(message);
    process.exitCode = 1;
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) await main();
