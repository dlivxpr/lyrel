import { spawn, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { deflateSync } from "node:zlib";

// Only fixture data is authored/read here. The released native CLI owns replay and
// all terminal output; no imports from Pi internals and no escape-sequence proxy.
const args = process.argv.slice(2);
function option(name, fallback) {
  const index = args.indexOf(name);
  return index < 0 ? fallback : args[index + 1];
}
const suppliedCli = option("--pi", process.env.LYREL_PI_GRAPHICS_CLI);
if (!suppliedCli) throw new Error("Provide --pi <official pi executable or npm dist/cli.js>; no global install is performed");
const cli = resolve(suppliedCli);
const policy = option("--policy", "enabled");
const pictures = Number(option("--pictures", "20"));
const rounds = Number(option("--rounds", "1"));
if (!process.stdin.isTTY || !process.stdout.isTTY) throw new Error("Run in an owned real lyrel PTY, not redirected stdio");
if (!["enabled", "env-none", "settings-none", "display-none", "settings-kitty"].includes(policy)) throw new Error("Invalid --policy");
if (!Number.isInteger(pictures) || pictures < 2 || pictures > 24 || ![1, 2].includes(rounds)) throw new Error("Use --pictures 2..24 and --rounds 1|2");
const nodeEntry = /\.[cm]?js$/i.test(cli);
const executable = nodeEntry ? process.execPath : cli;
const prefix = nodeEntry ? [cli] : [];
if (nodeEntry) {
  const pkg = JSON.parse(readFileSync(join(dirname(cli), "..", "package.json"), "utf8"));
  if (pkg.name !== "@earendil-works/pi-coding-agent" || pkg.version !== "1.4.0") throw new Error("Not the official pinned Pi npm package");
}
const version = spawnSync(executable, [...prefix, "--version"], { encoding: "utf8", timeout: 15_000, windowsHide: true });
if (version.error || version.status !== 0 || version.stdout.trim() !== "1.4.0") {
  throw new Error("Fixture requires released earendil-works/pi 1.4.0; revalidate source/format before updating this pin (omp is not pi)");
}
const pkg = { name: "@earendil-works/pi-coding-agent", version: version.stdout.trim() };

const crcTable = Uint32Array.from({ length: 256 }, (_, n) => {
  for (let k = 0; k < 8; k++) n = n & 1 ? 0xedb88320 ^ (n >>> 1) : n >>> 1;
  return n >>> 0;
});
function chunk(type, bytes) {
  const body = Buffer.concat([Buffer.from(type), bytes]);
  let crc = 0xffffffff;
  for (const b of body) crc = crcTable[(crc ^ b) & 255] ^ (crc >>> 8);
  const result = Buffer.alloc(body.length + 8);
  result.writeUInt32BE(bytes.length);
  body.copy(result, 4);
  result.writeUInt32BE((crc ^ 0xffffffff) >>> 0, result.length - 4);
  return result;
}
function picture(index) {
  const width = 48, height = 24;
  const rgba = Buffer.alloc(width * height * 4);
  const scan = Buffer.alloc(height * (width * 4 + 1));
  for (let y = 0; y < height; y++) {
    for (let x = 0; x < width; x++) {
      const color = x >= 22 && x < 26 && y >= 10 && y < 14 ? [255, 255, 255]
        : y < 12 ? x < 24 ? [255, 0, index * 11] : [0, 255, index * 7]
        : x < 24 ? [index * 5, 0, 255] : [255, 255, 0];
      const offset = (y * width + x) * 4;
      rgba.set([...color, 255], offset);
    }
    rgba.copy(scan, y * (width * 4 + 1) + 1, y * width * 4, (y + 1) * width * 4);
  }
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(width); ihdr.writeUInt32BE(height, 4);
  ihdr[8] = 8; ihdr[9] = 6;
  const png = Buffer.concat([Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]), chunk("IHDR", ihdr), chunk("IDAT", deflateSync(scan)), chunk("IEND", Buffer.alloc(0))]);
  return { data: png.toString("base64"), rgbaSha256: createHash("sha256").update(rgba).digest("hex") };
}

const root = mkdtempSync(join(resolve(option("--temp-parent", tmpdir())), "lyrel-pi-replay-"));
let child;
let timedOut = false;
const deadline = setTimeout(() => {
  timedOut = true;
  child?.kill();
}, 180_000);
const stop = () => child?.kill();
process.once("SIGTERM", stop);
process.once("SIGINT", stop);
try {
  const cwd = join(root, "project"), agent = join(root, "agent"), sessions = join(root, "sessions");
  for (const dir of [cwd, agent, sessions, join(root, "home"), join(root, "config"), join(root, "cache"), join(root, "data"), join(root, "state")]) mkdirSync(dir);
  const settings = {
    quietStartup: true, theme: "dark", enableInstallTelemetry: false, enableAnalytics: false,
    cacheWarming: "off", compaction: { enabled: false },
    terminal: {
      images: policy === "settings-none" ? false : policy === "settings-kitty" ? "kitty" : "auto",
      showImages: policy !== "display-none", imageWidthCells: 12,
    },
    fullscreenExitOutput: "resume-hint",
  };
  writeFileSync(join(agent, "settings.json"), JSON.stringify(settings));
  writeFileSync(join(agent, "keybindings.json"), JSON.stringify({ "tui.altScreen.top": "f5", "tui.altScreen.bottom": "f6" }));
  const timestamp = "2026-10-31T00:00:00.000Z";
  const entries = [{ type: "session", version: 3, id: "lyrel-pi-synthetic-replay", timestamp, cwd }];
  let parentId = null;
  function message(value) {
    const id = entries.length.toString(16).padStart(8, "0");
    entries.push({ type: "message", id, parentId, timestamp, message: { ...value, timestamp: 1793404800000 } });
    parentId = id;
  }
  const usage = { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, totalTokens: 0, cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 } };
  const assistant = (content, stopReason = "stop") => message({ role: "assistant", content, api: "anthropic-messages", provider: "anthropic", model: "claude-sonnet-4-5", usage, stopReason });
  message({ role: "user", content: "Synthetic saved tool-result replay fixture. Do not submit a new task." });
  const identities = [];
  for (let index = 0; index < pictures; index++) {
    const image = picture(index);
    identities.push({ index, width: 48, height: 24, rgbaSha256: image.rgbaSha256 });
    assistant([{ type: "toolCall", id: `fixture-read-${index}`, name: "read", arguments: { path: `synthetic-picture-${index}.png` } }], "toolUse");
    message({ role: "toolResult", toolCallId: `fixture-read-${index}`, toolName: "read", content: [{ type: "text", text: `PI_REPLAY_PICTURE_${index}` }, { type: "image", mimeType: "image/png", data: image.data }], isError: false });
    assistant([{ type: "text", text: index === pictures - 1 ? "PI_REPLAY_TAIL" : Array.from({ length: 24 }, (_, n) => `Synthetic separator ${index}:${n}`).join("\n") }]);
  }
  const file = join(sessions, "synthetic.jsonl");
  const original = entries.map(e => JSON.stringify(e)).join("\n") + "\n";
  writeFileSync(file, original);
  // Preserve only OS startup/terminal facts. Never forward credentials, extensions,
  // private config directories, NODE_OPTIONS, proxies, or shell startup files.
  const env = {};
  const keep = new Set(["path", "pathext", "systemroot", "windir", "comspec", "temp", "tmp", "term", "colorterm", "term_program", "term_program_version", "lang", "lc_all", "lc_ctype"]);
  for (const [key, value] of Object.entries(process.env)) if (keep.has(key.toLowerCase())) env[key] = value;
  Object.assign(env, {
    HOME: join(root, "home"), USERPROFILE: join(root, "home"),
    APPDATA: join(root, "config"), LOCALAPPDATA: join(root, "data"),
    XDG_CONFIG_HOME: join(root, "config"), XDG_CACHE_HOME: join(root, "cache"), XDG_DATA_HOME: join(root, "data"), XDG_STATE_HOME: join(root, "state"),
    PI_CODING_AGENT_DIR: agent, PI_CODING_AGENT_SESSION_DIR: sessions,
    PI_IMAGE_PROTOCOL: policy === "env-none" || policy === "settings-kitty" ? "none" : "kitty", PI_OFFLINE: "1",
  });
  if (env.TERM_PROGRAM !== "lyrel") throw new Error("Fixture requires truthful TERM_PROGRAM=lyrel from an isolated lyrel pane");
  console.log(`PI_REPLAY_FIXTURE ${JSON.stringify({ package: pkg.name, version: pkg.version, policy, rounds, identities })}`);
  for (let round = 1; round <= rounds; round++) {
    if (timedOut) throw new Error("Owned Pi replay exceeded 180 seconds");
    child = spawn(executable, [...prefix, "--offline", "--no-extensions", "--no-mcp", "--no-skills", "--no-prompt-templates", "--no-context-files", "--no-approve", "--tui-mode", "fullscreen", "--session", file, "--session-dir", sessions], { cwd, env, stdio: "inherit" });
    const status = await new Promise((resolveExit, reject) => { child.once("error", reject); child.once("exit", (code, signal) => resolveExit({ code, signal })); });
    child = undefined;
    if (timedOut || status.code !== 0) throw new Error(`Owned Pi failed: ${JSON.stringify(status)}, timeout=${timedOut}`);
    const after = readFileSync(file, "utf8").trim().split("\n").map(line => JSON.parse(line));
    const beforeMessages = entries.filter(e => e.type === "message");
    const afterMessages = after.filter(e => e.type === "message");
    if (JSON.stringify(afterMessages) !== JSON.stringify(beforeMessages) || after.some(e => ["usage", "compaction", "branch_summary"].includes(e.type))) {
      throw new Error("Replay changed synthetic messages or performed model-attributed work");
    }
    console.log(`PI_REPLAY_ROUND_${round}_NO_NEW_MESSAGES`);
  }
} finally {
  clearTimeout(deadline);
  stop();
  process.removeListener("SIGTERM", stop);
  process.removeListener("SIGINT", stop);
  // This directory was created by this invocation, never a supplied user path.
  rmSync(root, { recursive: true, force: true });
}
console.log("PI_REPLAY_FIXTURE_DONE");
