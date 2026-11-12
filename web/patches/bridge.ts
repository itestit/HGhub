/* 运行环境桥：桌面端走 Tauri IPC，网页版走同源 HTTP RPC。
 * 命令名、参数名、返回结构完全一致，因此业务代码一行都不用改。 */

const tauriInternals = (window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
export const isTauri = typeof tauriInternals !== "undefined";

/** 网页版把 invoke 映射成 POST /rpc/<cmd>，body 就是参数对象。 */
export async function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (isTauri) {
    const core = await import("@tauri-apps/api/core");
    return core.invoke<T>(cmd, args);
  }

  // 外链由浏览器自己开（服务端没有 shell）
  if (cmd === "open_url" && args && typeof args.url === "string") {
    window.open(String(args.url), "_blank", "noopener,noreferrer");
    return undefined as T;
  }

  const resp = await fetch(`/rpc/${encodeURIComponent(cmd)}`, {
    method: "POST",
    credentials: "same-origin",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(args ?? {}),
  });
  // 口令门过期：回登录页，让用户重新输入
  if (resp.status === 401) {
    location.replace("/login");
    throw "需要登录";
  }
  const payload = (await resp.json().catch(() => null)) as
    | { ok?: boolean; data?: unknown; error?: string }
    | null;
  if (!resp.ok || !payload || payload.ok === false) {
    throw (payload && payload.error) || `HTTP ${resp.status}`;
  }
  return payload.data as T;
}

/** 安卓返回键监听在网页版没有宿主，给个空实现即可。 */
export async function addPluginListener(...args: unknown[]): Promise<{ unregister: () => Promise<void> }> {
  if (isTauri) {
    const core = (await import("@tauri-apps/api/core")) as unknown as {
      addPluginListener: (...a: unknown[]) => Promise<{ unregister: () => Promise<void> }>;
    };
    return core.addPluginListener(...args);
  }
  return { unregister: async () => {} };
}

/** 桌面 chrome / 缩放把手在网页版没有原生窗口，给一组无操作实现。 */
const stubWindow = {
  label: "main",
  isMaximized: async () => false,
  isFullscreen: async () => false,
  onResized: async (_cb?: unknown) => () => {},
  onMoved: async (_cb?: unknown) => () => {},
  toggleMaximize: async () => {},
  minimize: async () => {},
  close: async () => {},
  startResizeDragging: async () => {},
  startDragging: async () => {},
  innerSize: async () => ({ width: window.innerWidth, height: window.innerHeight }),
  setSize: async () => {},
  setPosition: async () => {},
};

export function getCurrentWindow(): typeof stubWindow {
  return stubWindow;
}