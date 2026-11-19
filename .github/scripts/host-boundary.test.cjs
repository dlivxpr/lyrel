const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { spawnSync } = require('node:child_process');
const { test } = require('node:test');

const scanner = path.join(__dirname, 'host-boundary.cjs');
const desktopCrates = ['lyrel-ui-base', 'lyrel-terminal', 'lyrel-workbench-editor'];

function scanFixture(t) {
    const root = fs.mkdtempSync(path.join(os.tmpdir(), 'lyrel-host-boundary-roots-'));
    t.after(() => fs.rmSync(root, { recursive: true, force: true }));
    const ui = path.join(root, 'src', 'ui');
    fs.mkdirSync(ui, { recursive: true });
    fs.mkdirSync(path.join(root, 'src', 'terminal_tests'), { recursive: true });
    for (let ix = 0; ix < 10; ix++) fs.writeFileSync(path.join(ui, `shell_${ix}.rs`), '');
    for (const name of desktopCrates) fs.mkdirSync(path.join(root, 'crates', name, 'src'), { recursive: true });
    const run = () => spawnSync(process.execPath, [scanner], { cwd: root, env: { ...process.env, HOST_BOUNDARY_ALLOW: '' }, encoding: 'utf8' });
    return { root, run };
}

test('a missing desktop crate cannot silently fall out of the Host scan', (t) => {
    const { root, run } = scanFixture(t);
    fs.rmdirSync(path.join(root, 'crates', 'lyrel-workbench-editor', 'src'));
    const result = run();
    assert.equal(result.status, 2, result.stderr);
    assert.match(result.stderr, /lyrel-workbench-editor\/src.*does not exist/);
});

test('the terminal and editor scans reject a raw workspace read', (t) => {
    const { root, run } = scanFixture(t);
    for (const name of ['lyrel-terminal', 'lyrel-workbench-editor']) {
        fs.writeFileSync(path.join(root, 'crates', name, 'src', 'lib.rs'), 'fn bad() { std::fs::read("workspace-file").unwrap(); }\n');
    }
    const result = run();
    assert.equal(result.status, 1, result.stderr);
    assert.match(result.stderr, /lyrel-terminal\/src\/lib\.rs:1:/);
    assert.match(result.stderr, /lyrel-workbench-editor\/src\/lib\.rs:1:/);
});
for (const { name, cfg, shared, status } of [
    { name: 'explicit native-only feature excludes its fixtures', cfg: 'any(test, feature = "native-markdown-tests")', status: 0 },
    { name: 'a platform alternative remains production', cfg: 'any(test, unix)', status: 1 },
    { name: 'an unknown feature remains production', cfg: 'any(test, feature = "product-mode")', status: 1 },
    { name: 'a shared production import restores boundary enforcement', cfg: 'any(test, feature = "native-markdown-tests")', shared: true, status: 1 },
]) {
    test(name, (t) => {
        const { root, run } = scanFixture(t);
        const ui = path.join(root, 'src', 'ui');
        fs.writeFileSync(path.join(ui, 'mod.rs'), `#[cfg(${cfg})]\nmod native;\n${shared ? '#[path = "native.rs"]\nmod shared;' : ''}\n`);
        fs.writeFileSync(path.join(ui, 'native.rs'), 'fn fixture() { std::fs::read("workspace-file").unwrap(); }\n');
        const result = run();
        assert.equal(result.status, status, result.stderr || result.error?.message);
        if (status === 1) assert.match(result.stderr, /src\/ui\/native\.rs:1:.*std::fs::read/);
    });
}

// Crate roots resolve `mod child;` in their own directory, not in a
// `lib/`-named module directory: `src/lib.rs`'s `#[cfg(test)] mod fixture;`
// must stay excluded, while a production `#[path]` re-import reports again.
for (const { name, cfg, shared, status } of [
    { name: 'crate-root test-only fixtures stay excluded', cfg: 'test', status: 0 },
    { name: 'crate-root platform alternatives remain production', cfg: 'any(test, unix)', status: 1 },
    { name: 'a production #[path] re-import through a crate root restores enforcement', cfg: 'test', shared: true, status: 1 },
]) {
    test(name, (t) => {
        const { root, run } = scanFixture(t);
        const crateSrc = path.join(root, 'crates', 'lyrel-ui-base', 'src');
        fs.writeFileSync(path.join(crateSrc, 'lib.rs'), `#[cfg(${cfg})]\nmod fixture;\n${shared ? '#[path = "fixture.rs"]\nmod shared;' : ''}\n`);
        fs.writeFileSync(path.join(crateSrc, 'fixture.rs'), 'fn fixture() { std::fs::read("workspace-file").unwrap(); }\n');
        const result = run();
        assert.equal(result.status, status, result.stderr || result.error?.message);
        if (status === 1) assert.match(result.stderr, /crates\/lyrel-ui-base\/src\/fixture\.rs:1:.*std::fs::read/);
    });
}
