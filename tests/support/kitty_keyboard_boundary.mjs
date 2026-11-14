// Native admission fixture: run each command from an isolated integrated shell.
// The fixture never manufactures shell/agent OSC 133 markers.
const mode = process.argv[2];
if (mode === 'dirty') {
  // Leave a push on each screen, but no effective flags on the main screen.
  process.stdout.write('\x1b[>7u\x1b[?1049h\x1b[>7u\x1b[?1049l\x1b[=0;1u');
} else if (mode === 'clean') {
  process.stdout.write('KEYBOARD_CLEAN_EXIT\r\n');
} else if (mode === 'record' || mode === 'events') {
  if (!process.stdin.isTTY) throw new Error('A real PTY is required');
  process.stdin.setRawMode(true);
  process.stdin.resume();
  let received = Buffer.alloc(0);
  process.stdin.on('data', (chunk) => {
    const end = chunk.indexOf(10);
    received = Buffer.concat([received, end === -1 ? chunk : chunk.subarray(0, end)]);
    if (received.length > 8192) throw new Error('Keyboard fixture input limit exceeded');
    if (mode === 'events') {
      process.stdout.write(`\r\nKEYBOARD_EVENT_CHUNK:${chunk.toString('hex')}\r\n`);
    }
    if (end !== -1) {
      const hex = received.toString('hex');
      process.stdout.write(`\r\nKEYBOARD_BYTES:${hex}\r\n\x1b]2;KEYBOARD_BYTES:${hex}\x07`);
      process.stdin.removeAllListeners('data');
      // Keep the observed mode/record available until this owned pane is closed.
      process.stdin.on('data', () => {});
    }
  });
  const flags = mode === 'events' ? Number(process.argv[3] ?? 27) : 0;
  if (!Number.isInteger(flags) || flags < 0 || flags > 31) throw new Error('Invalid keyboard flags');
  const negotiation = mode === 'events' ? `\x1b[>${flags}u\x1b[?u` : '';
  process.stdout.write(`\x1b[?1049h${negotiation}\x1b]2;KEYBOARD_RECORDER_READY\x07KEYBOARD_RECORDER_READY flags=${flags}\r\n`);
} else {
  throw new Error('Use dirty, clean, record, or events [flags]');
}
