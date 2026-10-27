'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { spawnSync } = require('node:child_process');

function run(command, args, options = {}) {
    return spawnSync(command, args, { encoding: 'utf8', timeout: 30000, ...options });
}
function isolated(t) {
    const root = fs.mkdtempSync(path.join(fs.realpathSync(os.tmpdir()), 'lyrel-signing-'));
    t.after(() => fs.rmSync(root, { recursive: true, force: true }));
    return root;
}
function cleanEnvironment() {
    return Object.fromEntries(Object.entries(process.env).filter(([key]) =>
        !key.startsWith('APPLE_') && !key.startsWith('ASC_') && !key.startsWith('LYREL_') && key !== 'KEYCHAIN_PASSWORD'));
}

for (const [name, credentials, message] of [
    ['missing signing credentials', {}, /official releases require APPLE_CERTIFICATE/],
    ['ad-hoc identity', { APPLE_CERTIFICATE: 'fixture', APPLE_SIGNING_IDENTITY: '-' }, /ad-hoc fallback is forbidden/],
    ['missing Team', { APPLE_CERTIFICATE: 'fixture', APPLE_SIGNING_IDENTITY: 'fixture' }, /expected 10-character APPLE_TEAM_ID/],
    ['missing notarization', { APPLE_CERTIFICATE: 'fixture', APPLE_SIGNING_IDENTITY: 'fixture', APPLE_TEAM_ID: 'ABCDEFGHIJ' }, /notarization credentials/],
]) {
    test(`official packaging rejects ${name} before touching the output or keychain`, t => {
        const root = isolated(t);
        fs.writeFileSync(path.join(root, 'Cargo.toml'), 'version = "0.1.2"\n');
        const dist = path.join(root, 'dist');
        fs.mkdirSync(dist);
        fs.writeFileSync(path.join(dist, 'keep'), 'untouched');
        const result = run('bash', [path.join(__dirname, 'bundle-macos.sh'), 'aarch64-apple-darwin', 'arm64'], {
            cwd: root,
            env: { ...cleanEnvironment(), ...credentials, LYREL_REQUIRE_RELEASE_SIGNING: '1' },
        });
        assert.equal(result.status, 1, result.error?.message || result.stdout + result.stderr);
        assert.match(result.stderr, message);
        assert.equal(fs.readFileSync(path.join(dist, 'keep'), 'utf8'), 'untouched');
    });
}

test('same bundle ID does not stabilize ad-hoc identity or pass the official publisher gate', { skip: process.platform !== 'darwin' }, t => {
    const root = isolated(t);
    function bundle(name, source) {
        const app = path.join(root, `${name}.app`);
        const macos = path.join(app, 'Contents', 'MacOS');
        fs.mkdirSync(macos, { recursive: true });
        fs.copyFileSync(source, path.join(macos, 'lyrel-app'));
        fs.writeFileSync(path.join(app, 'Contents', 'Info.plist'), `<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>io.github.dlivxpr.lyrel</string>
<key>CFBundleExecutable</key><string>lyrel-app</string>
<key>CFBundlePackageType</key><string>APPL</string>
</dict></plist>`);
        const signed = run('/usr/bin/codesign', ['--force', '--deep', '--sign', '-', app]);
        assert.equal(signed.status, 0, signed.stdout + signed.stderr);
        return app;
    }
    const current = bundle('current', '/usr/bin/true');
    const replacement = bundle('replacement', '/usr/bin/false');
    const displayed = run('/usr/bin/codesign', ['--display', '--requirements', '-', current]);
    assert.equal(displayed.status, 0, displayed.stdout + displayed.stderr);
    const requirement = (displayed.stdout + displayed.stderr).match(/designated => ([^\n]+)/)?.[1];
    assert.ok(requirement, 'codesign must report the synthesized requirement');
    const own = run('/usr/bin/codesign', ['--verify', '--strict', `-R=${requirement}`, current]);
    assert.equal(own.status, 0, own.stdout + own.stderr);
    const changed = run('/usr/bin/codesign', ['--verify', '--strict', `-R=${requirement}`, replacement]);
    assert.equal(changed.status, 3, changed.stdout + changed.stderr);
    assert.match(changed.stderr, /failed to satisfy specified code requirement/);
    const release = run('bash', [path.join(__dirname, 'verify-macos-release-signing.sh'), replacement, 'ABCDEFGHIJ']);
    assert.notEqual(release.status, 0, release.stdout + release.stderr);
    assert.match(release.stderr, /failed to satisfy specified code requirement/);
});
