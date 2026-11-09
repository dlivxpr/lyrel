const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { spawnSync } = require('node:child_process');
const { test } = require('node:test');

const scanner = path.join(__dirname, 'host-boundary.cjs');
for (const { name, cfg, shared, status } of [
    { name: 'explicit native-only feature excludes its fixtures', cfg: 'any(test, feature = "native-markdown-tests")', status: 0 },
    { name: 'a platform alternative remains production', cfg: 'any(test, unix)', status: 1 },
    { name: 'an unknown feature remains production', cfg: 'any(test, feature = "product-mode")', status: 1 },
    { name: 'a shared production import restores boundary enforcement', cfg: 'any(test, feature = "native-markdown-tests")', shared: true, status: 1 },
]) {
    test(name, () => {
        const root = fs.mkdtempSync(path.join(os.tmpdir(), 'lyrel-host-boundary-'));
        try {
            const ui = path.join(root, 'src', 'ui');
            fs.mkdirSync(ui, { recursive: true });
            fs.mkdirSync(path.join(root, 'src', 'terminal'), { recursive: true });
            for (let ix = 0; ix < 8; ix++) fs.writeFileSync(path.join(ui, `component_${ix}.rs`), '');
            fs.writeFileSync(path.join(ui, 'mod.rs'), `#[cfg(${cfg})]\nmod native;\n${shared ? '#[path = "native.rs"]\nmod shared;' : ''}\n`);
            fs.writeFileSync(path.join(ui, 'native.rs'), 'fn fixture() { std::fs::read("workspace-file").unwrap(); }\n');
            const result = spawnSync(process.execPath, [scanner], { cwd: root, env: { ...process.env, HOST_BOUNDARY_ALLOW: '' }, encoding: 'utf8' });
            assert.equal(result.status, status, result.stderr || result.error?.message);
            if (status === 1) assert.match(result.stderr, /src\/ui\/native\.rs:1:.*std::fs::read/);
        } finally {
            fs.rmSync(root, { recursive: true, force: true });
        }
    });
}
