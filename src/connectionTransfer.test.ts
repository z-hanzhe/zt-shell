// @ts-nocheck

import assert from "node:assert/strict";
import test from "node:test";

import {
  CONNECTION_EXPORT_FORMAT,
  CONNECTION_EXPORT_VERSION,
  buildConnectionExport,
  createProxyConfigFingerprint,
  findAvailableConnectionName,
  parseConnectionExport,
  planConnectionImport,
  serializeConnectionExport,
  validateConnectionExport,
} from "./connectionTransfer.ts";

/** 构造测试连接 */
function connection(overrides = {}) {
  return {
    id: "connection-default",
    name: "默认连接",
    host: "host.example.com",
    port: 22,
    username: "root",
    authType: "password",
    password: "connection-password",
    privateKeyPath: undefined,
    passphrase: undefined,
    proxyId: null,
    remark: "测试备注",
    tunnels: [],
    parentId: null,
    order: undefined,
    ...overrides,
  };
}

/** 构造测试文件夹 */
function folder(overrides = {}) {
  return {
    id: "folder-default",
    name: "默认文件夹",
    parentId: null,
    order: undefined,
    ...overrides,
  };
}

/** 构造测试代理 */
function proxy(overrides = {}) {
  return {
    id: "proxy-default",
    name: "默认代理",
    proxyType: "socks5",
    host: "proxy.example.com",
    port: 1080,
    username: "proxy-user",
    password: "proxy-password",
    ...overrides,
  };
}

/** 构造 V1 测试连接 */
function fileConnection(overrides = {}) {
  return {
    ref: "connection-ref-default",
    name: "导入连接",
    host: "import.example.com",
    port: 22,
    username: "root",
    authType: "password",
    password: "import-password",
    privateKeyPath: null,
    passphrase: null,
    proxyRef: null,
    remark: null,
    tunnels: [],
    parentRef: null,
    order: 0,
    ...overrides,
  };
}

/** 构造 V1 测试代理 */
function fileProxy(overrides = {}) {
  return {
    ref: "proxy-ref-default",
    name: "导入代理",
    proxyType: "socks5",
    host: "proxy.example.com",
    port: 1080,
    username: "proxy-user",
    password: "proxy-password",
    ...overrides,
  };
}

/** 构造合法的 V1 测试文件 */
function exportFile(overrides = {}) {
  return {
    format: CONNECTION_EXPORT_FORMAT,
    version: CONNECTION_EXPORT_VERSION,
    exportedAt: "2026-08-09T01:02:03.000Z",
    folders: [],
    connections: [],
    proxies: [],
    ...overrides,
  };
}

/** 创建可预测且不会重复的测试 id 工厂 */
function sequentialIdFactory(prefix = "generated") {
  let sequence = 0;
  return () => `${prefix}-${++sequence}`;
}

/** 按给定顺序返回测试 id */
function listedIdFactory(ids) {
  let index = 0;
  return () => ids[index++];
}

test("完整导出保留树形混合顺序、凭据字段和空文件夹，并排除未使用代理", () => {
  const snapshot = {
    folders: [
      folder({ id: "folder-root", name: "项目", parentId: null, order: 1 }),
      folder({ id: "folder-child", name: "子项目", parentId: "folder-root", order: 0 }),
      folder({ id: "folder-empty", name: "空文件夹", parentId: "folder-root", order: 2 }),
    ],
    connections: [
      connection({
        id: "connection-root",
        name: "根连接",
        authType: "privateKey",
        privateKeyPath: "C:\\Keys\\id_ed25519",
        passphrase: "key-passphrase",
        proxyId: "proxy-used",
        parentId: null,
        order: 0,
        tunnels: [
          {
            id: "tunnel-source-id",
            name: "数据库",
            tunnelType: "local",
            enabled: true,
            localOnly: true,
            listenPort: 3306,
            targetHost: "db.internal",
            targetPort: 3306,
          },
        ],
      }),
      connection({
        id: "connection-child",
        name: "项目连接",
        proxyId: "proxy-used",
        parentId: "folder-root",
        order: 1,
      }),
      connection({
        id: "connection-deep",
        name: "深层连接",
        parentId: "folder-child",
        order: 0,
      }),
    ],
    proxies: [
      proxy({ id: "proxy-used", name: "已使用代理" }),
      proxy({ id: "proxy-unused", name: "未使用代理", host: "unused.example.com" }),
    ],
  };

  const file = buildConnectionExport({ kind: "all" }, snapshot, {
    now: () => new Date("2026-08-09T01:02:03.000Z"),
  });

  assert.equal(file.connections.find((item) => item.name === "根连接").order, 0);
  assert.deepEqual(
    file.folders.map((item) => [item.ref, item.parentRef, item.order]),
    [
      ["folder-root", null, 1],
      ["folder-child", "folder-root", 0],
      ["folder-empty", "folder-root", 2],
    ]
  );
  assert.equal(file.connections.find((item) => item.name === "项目连接").order, 1);
  assert.equal(file.connections.find((item) => item.name === "根连接").privateKeyPath, "C:\\Keys\\id_ed25519");
  assert.equal(file.connections.find((item) => item.name === "根连接").password, null);
  assert.equal(file.connections.find((item) => item.name === "根连接").passphrase, null);
  assert.equal(file.connections.find((item) => item.name === "根连接").ref, "connection-root");
  assert.equal("id" in file.connections[0], false);
  assert.equal("id" in file.connections.find((item) => item.name === "根连接").tunnels[0], false);
  assert.deepEqual(file.proxies.map((item) => item.ref), ["proxy-used"]);
  assert.equal(file.proxies[0].password, null);
  assert.deepEqual(parseConnectionExport(serializeConnectionExport(file)), file);
});

test("单连接导出脱离原文件夹，文件夹导出将所选子树提升为根", () => {
  const snapshot = {
    folders: [
      folder({ id: "folder-parent", name: "父级", parentId: null, order: 0 }),
      folder({ id: "folder-target", name: "目标", parentId: "folder-parent", order: 0 }),
      folder({ id: "folder-child", name: "后代", parentId: "folder-target", order: 0 }),
      folder({ id: "folder-outside", name: "外部", parentId: null, order: 1 }),
    ],
    connections: [
      connection({ id: "connection-target", name: "目标连接", parentId: "folder-target", order: 1 }),
      connection({ id: "connection-child", name: "后代连接", parentId: "folder-child", order: 0 }),
      connection({ id: "connection-outside", name: "外部连接", parentId: "folder-outside", order: 0 }),
    ],
    proxies: [],
  };

  const single = buildConnectionExport({ kind: "connection", id: "connection-target" }, snapshot);
  assert.equal(single.folders.length, 0);
  assert.equal(single.connections[0].parentRef, null);
  assert.equal(single.connections[0].order, 0);

  const subtree = buildConnectionExport({ kind: "folder", id: "folder-target" }, snapshot);
  assert.deepEqual(subtree.folders.map((item) => item.ref), ["folder-target", "folder-child"]);
  assert.equal(subtree.folders[0].parentRef, null);
  assert.equal(subtree.folders[0].order, 0);
  assert.deepEqual(subtree.connections.map((item) => item.name), ["后代连接", "目标连接"]);
  assert.equal(subtree.connections.some((item) => item.name === "外部连接"), false);
});

test("导出允许代理密码等待 Rust 从系统凭据库注入", () => {
  const file = buildConnectionExport(
    { kind: "all" },
    {
      folders: [],
      connections: [connection({ id: "connection-secure", proxyId: "proxy-secure", order: 0 })],
      proxies: [
        proxy({ id: "proxy-secure", username: "proxy-user", password: undefined, hasPassword: true }),
      ],
    }
  );
  assert.equal(file.proxies[0].username, "proxy-user");
  assert.equal(file.proxies[0].password, null);
});

test("V1 校验拒绝未知字段、循环层级、悬空代理和重复混合排序", () => {
  assert.throws(
    () => validateConnectionExport({ ...exportFile(), unexpected: true }),
    /不支持的字段/
  );
  assert.throws(
    () =>
      validateConnectionExport(
        exportFile({
          folders: [
            { ref: "folder-a", name: "A", parentRef: "folder-b", order: 0 },
            { ref: "folder-b", name: "B", parentRef: "folder-a", order: 0 },
          ],
        })
      ),
    /循环引用/
  );
  assert.throws(
    () =>
      validateConnectionExport(
        exportFile({ connections: [fileConnection({ proxyRef: "missing-proxy" })] })
      ),
    /不存在的代理/
  );
  assert.throws(
    () =>
      validateConnectionExport(
        exportFile({
          folders: [{ ref: "folder-a", name: "A", parentRef: null, order: 0 }],
          connections: [fileConnection({ order: 0 })],
        })
      ),
    /相同排序值/
  );
  assert.throws(
    () =>
      validateConnectionExport(
        exportFile({
          connections: [
            fileConnection({ ref: "duplicate-ref", name: "A", order: 0 }),
            fileConnection({ ref: "duplicate-ref", name: "B", order: 1 }),
          ],
        })
      ),
    /连接引用.*重复/
  );
});

test("导入重生成连接内部 id、复用同层同名文件夹并按同层连接名递增", () => {
  const existingProxy = proxy({
    id: "proxy-existing",
    name: "本地代理名称",
    host: "PROXY.EXAMPLE.COM",
  });
  const snapshot = {
    folders: [folder({ id: "folder-existing", name: "同名文件夹", order: 0 })],
    connections: [
      connection({
        id: "connection-internal-existing",
        name: "内部连接",
        parentId: "folder-existing",
        order: 0,
      }),
      connection({ id: "connection-existing", name: "服务器", order: 1 }),
      connection({ id: "connection-existing-2", name: "服务器 (2)", order: 2 }),
    ],
    proxies: [existingProxy],
  };
  const file = exportFile({
    folders: [{ ref: "folder-source", name: "同名文件夹", parentRef: null, order: 0 }],
    connections: [
      fileConnection({
        ref: "connection-internal-first",
        name: "内部连接",
        authType: "privateKey",
        privateKeyPath: "/home/me/.ssh/id_ed25519",
        passphrase: "key-passphrase",
        proxyRef: "proxy-same",
        parentRef: "folder-source",
        order: 0,
        tunnels: [
          {
            name: "动态代理",
            tunnelType: "dynamic",
            enabled: true,
            localOnly: true,
            listenPort: 1081,
            targetHost: null,
            targetPort: null,
          },
        ],
      }),
      fileConnection({
        ref: "connection-internal-second",
        name: "内部连接",
        host: "second.internal",
        proxyRef: "proxy-new",
        parentRef: "folder-source",
        order: 1,
      }),
      fileConnection({
        ref: "connection-root",
        name: "服务器",
        proxyRef: "proxy-same",
        parentRef: null,
        order: 1,
      }),
    ],
    proxies: [
      fileProxy({ ref: "proxy-same", name: "导入时的其他名称", host: "proxy.example.com" }),
      fileProxy({
        ref: "proxy-new",
        name: "新增 HTTP 代理",
        proxyType: "http",
        host: "http-proxy.example.com",
        port: 8080,
      }),
    ],
  });

  const plan = planConnectionImport(file, snapshot, { idFactory: sequentialIdFactory() });
  const internalConnections = plan.connections.filter((item) => item.parentId === "folder-existing");
  const rootConnection = plan.connections.find((item) => item.parentId === null);

  assert.equal(plan.folders.length, 0);
  assert.deepEqual(internalConnections.map((item) => item.name), ["内部连接 (2)", "内部连接 (3)"]);
  assert.deepEqual(internalConnections.map((item) => item.order), [1, 2]);
  assert.equal(rootConnection.name, "服务器 (3)");
  assert.equal(internalConnections[0].privateKeyPath, "/home/me/.ssh/id_ed25519");
  assert.equal(internalConnections[0].password, undefined);
  assert.equal(internalConnections[0].passphrase, "key-passphrase");
  assert.notEqual(internalConnections[0].tunnels[0].id, "tunnel-source-id");
  assert.equal(internalConnections[0].proxyId, "proxy-existing");
  assert.equal(rootConnection.proxyId, "proxy-existing");
  assert.equal(plan.proxies.length, 1);
  assert.equal(plan.proxies[0].name, "新增 HTTP 代理");
  assert.equal(internalConnections[1].proxyId, plan.proxies[0].id);
  assert.deepEqual(plan.rootOrderIds, [
    "folder-existing",
    "connection-existing",
    "connection-existing-2",
    rootConnection.id,
  ]);
  assert.deepEqual(plan.summary, {
    connectionCount: 3,
    folderCount: 0,
    reusedFolderCount: 1,
    addedProxyCount: 1,
    reusedProxyCount: 1,
    renamedConnectionCount: 3,
    privateKeyConnectionCount: 1,
  });
  assert.equal(plan.connections.every((item) => item.id.startsWith("generated-")), true);
});

test("嵌套文件夹导出后导入会回到原文件夹并复用同名子文件夹", () => {
  const snapshot = {
    folders: [
      folder({ id: "folder-parent", name: "父级", parentId: null, order: 0 }),
      folder({ id: "folder-target", name: "目标", parentId: "folder-parent", order: 0 }),
      folder({ id: "folder-child-existing", name: "子级", parentId: "folder-target", order: 0 }),
    ],
    connections: [
      connection({ id: "connection-target-existing", name: "服务器", parentId: "folder-target", order: 1 }),
      connection({ id: "connection-child-existing", name: "子连接", parentId: "folder-child-existing", order: 0 }),
    ],
    proxies: [],
  };
  const file = exportFile({
    folders: [
      { ref: "folder-target", name: "目标", parentRef: null, order: 0 },
      { ref: "folder-child-source", name: "子级", parentRef: "folder-target", order: 0 },
      { ref: "folder-new", name: "新增子级", parentRef: "folder-target", order: 1 },
    ],
    connections: [
      fileConnection({
        ref: "connection-child-source",
        name: "子连接",
        parentRef: "folder-child-source",
        order: 0,
      }),
      fileConnection({
        ref: "connection-target-source",
        name: "服务器",
        parentRef: "folder-target",
        order: 2,
      }),
    ],
  });

  const plan = planConnectionImport(file, snapshot, { idFactory: sequentialIdFactory("nested") });
  const newFolder = plan.folders[0];
  const targetConnection = plan.connections.find((item) => item.name === "服务器 (2)");
  const childConnection = plan.connections.find((item) => item.name === "子连接 (2)");

  assert.equal(plan.folders.length, 1);
  assert.equal(newFolder.name, "新增子级");
  assert.equal(newFolder.parentId, "folder-target");
  assert.equal(newFolder.order, 2);
  assert.equal(targetConnection.parentId, "folder-target");
  assert.equal(targetConnection.order, 3);
  assert.equal(childConnection.parentId, "folder-child-existing");
  assert.equal(childConnection.order, 1);
  assert.deepEqual(plan.rootOrderIds, ["folder-parent"]);
  assert.equal(plan.summary.folderCount, 1);
  assert.equal(plan.summary.reusedFolderCount, 2);
  assert.equal(plan.summary.renamedConnectionCount, 2);
});

test("导入追加前保留旧的无 order 根层显示顺序", () => {
  const snapshot = {
    folders: [folder({ id: "folder-old", name: "Z 文件夹", order: undefined })],
    connections: [connection({ id: "connection-old", name: "A 连接", order: undefined })],
    proxies: [],
  };
  const plan = planConnectionImport(
    exportFile({ connections: [fileConnection({ name: "新增连接" })] }),
    snapshot,
    { idFactory: sequentialIdFactory("append") }
  );
  assert.deepEqual(plan.rootOrderIds, ["folder-old", "connection-old", plan.connections[0].id]);
  assert.equal(plan.connections[0].order, 2);
});

test("相同配置的多个导入代理合并为一条，首次引用代理保留名称", () => {
  const file = exportFile({
    connections: [
      fileConnection({ ref: "connection-a", name: "A", proxyRef: "proxy-first", order: 0 }),
      fileConnection({ ref: "connection-b", name: "B", proxyRef: "proxy-second", order: 1 }),
    ],
    proxies: [
      fileProxy({ ref: "proxy-first", name: "首次名称" }),
      fileProxy({ ref: "proxy-second", name: "第二名称" }),
    ],
  });
  const plan = planConnectionImport(file, { folders: [], connections: [], proxies: [] }, {
    idFactory: sequentialIdFactory("proxy-deduplicate"),
  });
  assert.equal(plan.proxies.length, 1);
  assert.equal(plan.proxies[0].name, "首次名称");
  assert.equal(plan.connections[0].proxyId, plan.connections[1].proxyId);
  assert.equal(plan.summary.reusedProxyCount, 1);
});

test("未知系统密码的本地代理仅通过后端匹配映射复用，并优先覆盖同配置来源代理", () => {
  const secureProxy = proxy({
    id: "proxy-secure",
    username: undefined,
    password: undefined,
    hasPassword: true,
  });
  const file = exportFile({
    connections: [
      fileConnection({ ref: "connection-first", name: "A", proxyRef: "proxy-first", order: 0 }),
      fileConnection({ ref: "connection-second", name: "B", proxyRef: "proxy-second", order: 1 }),
    ],
    proxies: [
      fileProxy({ ref: "proxy-first", name: "首个来源", username: null, password: null }),
      fileProxy({ ref: "proxy-second", name: "后端已匹配", username: null, password: null }),
    ],
  });

  const withoutMatch = planConnectionImport(file, { folders: [], connections: [], proxies: [secureProxy] }, {
    idFactory: sequentialIdFactory("without-match"),
  });
  assert.equal(withoutMatch.proxies.length, 1);
  assert.notEqual(withoutMatch.connections[0].proxyId, "proxy-secure");

  const withMatch = planConnectionImport(file, { folders: [], connections: [], proxies: [secureProxy] }, {
    idFactory: sequentialIdFactory("with-match"),
    matchedProxyIds: new Map([["proxy-second", "proxy-secure"]]),
  });
  assert.equal(withMatch.proxies.length, 0);
  assert.equal(withMatch.connections[0].proxyId, "proxy-secure");
  assert.equal(withMatch.connections[1].proxyId, "proxy-secure");
  assert.equal(withMatch.summary.reusedProxyCount, 2);
});

test("id 工厂返回文件内 ref 时会继续生成，避免把外部引用当作内部 id", () => {
  const file = exportFile({
    folders: [{ ref: "folder-source", name: "来源文件夹", parentRef: null, order: 0 }],
    connections: [
      fileConnection({
        ref: "connection-source",
        proxyRef: "proxy-source",
        parentRef: "folder-source",
        order: 0,
      }),
    ],
    proxies: [fileProxy({ ref: "proxy-source" })],
  });
  const plan = planConnectionImport(file, { folders: [], connections: [], proxies: [] }, {
    idFactory: listedIdFactory([
      "folder-source",
      "folder-generated",
      "proxy-source",
      "proxy-generated",
      "connection-generated",
    ]),
  });
  assert.equal(plan.folders[0].id, "folder-generated");
  assert.equal(plan.proxies[0].id, "proxy-generated");
  assert.equal(plan.connections[0].id, "connection-generated");
});

test("代理指纹忽略名称与主机大小写，并仅比较协议实际使用的认证字段", () => {
  const socks4A = createProxyConfigFingerprint({
    proxyType: "socks4",
    host: " PROXY.EXAMPLE.COM ",
    port: 1080,
    username: "user",
    password: "unused-a",
  });
  const socks4B = createProxyConfigFingerprint({
    proxyType: "socks4",
    host: "proxy.example.com",
    port: 1080,
    username: "user",
    password: "unused-b",
  });
  const socks5A = createProxyConfigFingerprint({
    proxyType: "socks5",
    host: "proxy.example.com",
    port: 1080,
    username: "user",
    password: "password-a",
  });
  const socks5B = createProxyConfigFingerprint({
    proxyType: "socks5",
    host: "proxy.example.com",
    port: 1080,
    username: "user",
    password: "password-b",
  });
  const trimmedUsername = createProxyConfigFingerprint({
    proxyType: "socks5",
    host: "proxy.example.com",
    port: 1080,
    username: " user ",
    password: "password-a",
  });
  assert.equal(socks4A, socks4B);
  assert.notEqual(socks5A, socks5B);
  assert.equal(socks5A, trimmedUsername);
});

test("导入拒绝协议不使用的代理密码和缺少用户名的认证密码", () => {
  assert.throws(
    () => validateConnectionExport(exportFile({
      connections: [fileConnection({ proxyRef: "proxy-socks4" })],
      proxies: [fileProxy({
        ref: "proxy-socks4",
        proxyType: "socks4",
        password: "unused-password",
      })],
    })),
    /不能用于 SOCKS4/
  );
  assert.throws(
    () => validateConnectionExport(exportFile({
      connections: [fileConnection({ proxyRef: "proxy-without-username" })],
      proxies: [fileProxy({
        ref: "proxy-without-username",
        username: null,
        password: "proxy-password",
      })],
    })),
    /包含密码时不能为空/
  );
});

test("同层连接名称从第一个可用序号开始递增", () => {
  assert.equal(findAvailableConnectionName("服务器", new Set()), "服务器");
  assert.equal(
    findAvailableConnectionName("服务器", new Set(["服务器", "服务器 (2)", "服务器 (4)"])),
    "服务器 (3)"
  );
});

test("JSON 解析错误使用中文业务错误", () => {
  assert.throws(() => parseConnectionExport("{invalid"), /导入文件不是有效的 JSON/);
});

test("导出拒绝无效的注入时间", () => {
  assert.throws(
    () =>
      buildConnectionExport(
        { kind: "all" },
        { folders: [], connections: [], proxies: [] },
        { now: () => new Date(Number.NaN) }
      ),
    /导出时间无效/
  );
});
