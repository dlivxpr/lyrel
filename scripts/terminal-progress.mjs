// 运行：node scripts/terminal-progress.mjs
// 仅向当前 PTY 写入 OSC 9;4，不访问 daemon、配置或其他 pane。
import readline from 'node:readline';
const osc = (payload) => process.stdout.write(`\x1b]${payload}\x07`);
console.log('OSC 9;4 progress: 1=42%, 2=working, 3=paused 61%, 4=error, c=clear, d=command end, n=next command, f=trim replay, q=quit');
readline.emitKeypressEvents(process.stdin);
if (process.stdin.isTTY) process.stdin.setRawMode(true);
process.stdin.on('keypress', (_, key) => {
  switch (key?.name) {
    case '1': osc('9;4;1;42'); break;
    case '2': osc('9;4;3'); break;
    case '3': osc('9;4;4;61'); break;
    case '4': osc('9;4;2'); break;
    case 'c': osc('9;4;0'); break;
    case 'd': osc('133;D;0'); break;
    case 'n': osc('133;C'); break;
    case 'f': {
      // 超过生产 ReplayRing 的 8 MiB 容量；状态发送器此后保持静默。
      const block = `${'x'.repeat(120)}\r\n`.repeat(64);
      for (let i = 0; i < 1200; i++) process.stdout.write(block);
      console.log('\r\nProgress bytes evicted; detach and reattach this live pane.');
      break;
    }
    case 'q':
      osc('9;4;0');
      if (process.stdin.isTTY) process.stdin.setRawMode(false);
      process.exit(0);
  }
});
