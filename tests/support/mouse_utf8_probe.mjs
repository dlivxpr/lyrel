// Wide-pane mouse probe for #263 acceptance scenes.
//
// Run this inside a real lyrel pane — local native window, WSL, or a remote
// SSH pane (run this file with Node on the remote host).
// It negotiates UTF-8 mouse coordinates (?1002h ?1005h), asks the terminal to
// report the effective encodings (DECRQM ?1005$p ?1006$p ?1015$p), and then
// hex-dumps every byte the terminal delivers on stdin, decoding X10/UTF-8
// mouse reports in-line so clicks past column 222 are visible byte-for-byte.
//
// Controls: click/drag/wheel, Shift+click for local selection, s=SGR,
// u=UTF-8, q=mode queries, Ctrl+C to leave. Negotiated mouse modes are reset on
// exit. Nothing is simulated: the bytes shown are the bytes the host
// terminal's encoder emitted.
//
// Usage: node mouse_utf8_probe.mjs
// Requires Node >= 18 (only stdlib). Works under ConPTY, raw POSIX PTYs and
// through SSH because it only reads stdin/writes stdout.

import process from "node:process";

const ESC = "\x1b";
const out = process.stdout;
const input = process.stdin;

function write(text) {
  out.write(text);
}

function hex(bytes) {
  return [...bytes].map((b) => b.toString(16).padStart(2, "0")).join(" ");
}

// Decode `ESC [ M` reports the way a mode-1005 consumer reads them: Cb, Cx,
// Cy each arrive as one UTF-8 character. A real application parses its input
// as UTF-8 under this mode, so decode the stream, not single bytes.
export function describeReports(text) {
  const found = [];
  let i = 0;
  while (i < text.length) {
    if (text.startsWith(`${ESC}[M`, i)) {
      if (text.length - i < 6) break;
      const cb = text.charCodeAt(i + 3) - 32;
      const cx = text.charCodeAt(i + 4) - 32;
      const cy = text.charCodeAt(i + 5) - 32;
      const mods = [cb & 4 ? "shift" : null, cb & 8 ? "alt" : null, cb & 16 ? "ctrl" : null]
        .filter(Boolean).join("+");
      found.push(
        `UTF-8 report: cb=${cb} button=${cb & 3}${cb & 32 ? " motion" : ""}${cb & 64 ? " wheel" : ""}` +
        `${mods ? ` mods=${mods}` : ""} cell=(${cx - 1},${cy - 1}) (1-based col ${cx}, row ${cy})`
      );
      i += 6;
    } else if (text.startsWith(`${ESC}[<`, i) || text.startsWith(`${ESC}[?`, i)) {
      const rest = text.slice(i);
      const match = rest.startsWith(`${ESC}[<`)
        ? /^\x1b\[<(\d+;\d+;\d+[Mm])/.exec(rest)
        : /^\x1b\[\?(\d+;\d+\$y)/.exec(rest);
      if (!match) {
        if (rest.length < 64 && /^\x1b\[[<?][\d;$]*$/.test(rest)) break;
        i++;
        continue;
      }
      found.push(`${rest[2] === "<" ? "SGR report" : "DECRQM reply"}: ${match[1]}`);
      i += match[0].length;
    } else if (text[i] === ESC && text.length - i < 3) {
      break;
    } else {
      const command = text[i++];
      if (command === "\x03") found.push("exit");
      if (command === "u" || command === "s" || command === "q") found.push(`command:${command}`);
    }
  }
  return { found, remainder: text.slice(i) };
}

async function run() {
  if (!input.isTTY || !out.isTTY) throw new Error("run inside a real terminal PTY");
  const { StringDecoder } = await import("node:string_decoder");
  const decoder = new StringDecoder("utf8");
  let leftover = "";
  const query = () => write(`${ESC}[?1005$p${ESC}[?1006$p${ESC}[?1015$p`);
  const select = (mode) => {
    write(`\r\nselecting ?${mode}h; pane=${out.columns}x${out.rows}\r\n${ESC}[?${mode}h`);
    query();
  };
  const finish = () => {
    input.setRawMode(false);
    write(`\r\n${ESC}[?1005l${ESC}[?1006l${ESC}[?1002l`);
    write("mouse modes reset, exiting\r\n");
    process.exit(0);
  };
  input.setRawMode(true);
  input.resume();
  input.on("data", (chunk) => {
    write(`stdin ${chunk.length}B: ${hex(chunk)}\r\n`);
    const result = describeReports(leftover + decoder.write(chunk));
    leftover = result.remainder;
    for (const line of result.found) {
      if (line === "exit") finish();
      else if (line === "command:u") select(1005);
      else if (line === "command:s") select(1006);
      else if (line === "command:q") query();
      else write(`  -> ${line}\r\n`);
    }
  });
  process.on("SIGINT", finish);
  process.on("SIGTERM", finish);
  write("\r\n== mouse_utf8_probe ==\r\n");
  write("Need >=241 columns: click/drag at column 241, row 11; Ctrl/Alt change modifiers.\r\n");
  write("Shift+click stays local. Keys: s=SGR, u=UTF-8, q=query, Ctrl+C=exit.\r\n");
  write(`${ESC}[?1002h`);
  select(1005);
}

const { pathToFileURL } = await import("node:url");
if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) await run();
