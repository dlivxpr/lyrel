'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { createArtifact, verifyArtifact } = require('./server-artifact.cjs');
const target = 'x86_64-unknown-linux-musl';
const identity = { sha: 'a'.repeat(40), repository: 'owner/repo', repositoryId: '7', runId: '10', runAttempt: '2' };
const contract = { rustc: 'rustc 1.90.0\nhost: x86_64-unknown-linux-gnu', cargo: 'cargo 1.90.0', zig: '0.16.0',
    zigbuild: 'cargo-zigbuild 0.20.0', rustflags: '-C strip=symbols', args: ['zigbuild', '--release', '--locked', '-p', 'lyrel-server', '--target', target],
    inputs: { 'Cargo.lock': '0'.repeat(64), 'Cargo.toml': '1'.repeat(64) }, environment: {} };
function fixture(t) {
    const root = fs.mkdtempSync(path.join(fs.realpathSync(os.tmpdir()), 'server-artifact-'));
    t.after(() => fs.rmSync(root, { recursive: true, force: true }));
    const binary = path.join(root, 'binary'), directory = path.join(root, 'artifact'), destination = path.join(root, 'target', 'lyrel-server');
    fs.writeFileSync(binary, 'known server bytes\n');
    createArtifact({ target, binary, directory, identity, contract });
    return { root, binary, directory, destination, target, identity, contract };
}
test('matching provenance and contract promotes verified bytes, not an executed binary', t => {
    const f = fixture(t);
    assert.equal(verifyArtifact(f).reused, true);
    assert.equal(fs.readFileSync(f.destination, 'utf8'), 'known server bytes\n');
    if (process.platform !== 'win32') assert.equal(fs.statSync(f.destination).mode & 0o777, 0o755);
});
for (const [name, mutate] of [
    ['binary hash tampering', f => fs.appendFileSync(path.join(f.directory, 'lyrel-server'), 'tampered')],
    ['run identity tampering', f => { const file = path.join(f.directory, 'manifest.json'); const m = JSON.parse(fs.readFileSync(file)); m.identity.runId = '999'; fs.writeFileSync(file, JSON.stringify(m)); }],
    ['SHA mismatch', f => { f.identity = { ...identity, sha: 'b'.repeat(40) }; }],
    ['run attempt mismatch', f => { f.identity = { ...identity, runAttempt: '3' }; }],
    ['toolchain mismatch', f => { f.contract = { ...contract, rustc: 'rustc changed' }; }],
    ['build input mismatch', f => { f.contract = { ...contract, inputs: { 'Cargo.lock': 'f'.repeat(64) } }; }],
    ['missing binary', f => fs.unlinkSync(path.join(f.directory, 'lyrel-server'))],
    ['missing manifest', f => fs.unlinkSync(path.join(f.directory, 'manifest.json'))],
    ['non-directory artifact path', f => { f.directory = f.binary; }],
    ['repository mismatch', f => { f.identity = { ...identity, repository: 'attacker/repo' }; }],
    ['repository ID mismatch', f => { f.identity = { ...identity, repositoryId: '99' }; }],
    ['invalid manifest JSON', f => fs.writeFileSync(path.join(f.directory, 'manifest.json'), '{')],
    ['extra path entry', f => fs.writeFileSync(path.join(f.directory, 'other'), 'unexpected')],
    ['manifest binary traversal', f => { const file = path.join(f.directory, 'manifest.json'); const m = JSON.parse(fs.readFileSync(file)); m.binary.name = '../binary'; fs.writeFileSync(file, JSON.stringify(m)); }]
]) test(`${name} returns a miss without creating the destination`, t => {
    const f = fixture(t); mutate(f);
    const result = verifyArtifact(f);
    assert.equal(result.reused, false);
    assert.ok(result.reason);
    assert.equal(fs.existsSync(f.destination), false);
});
test('symbolic-link binary and manifest are rejected', { skip: process.platform === 'win32' && 'Creating file symlinks needs Windows developer mode' }, t => {
    for (const name of ['lyrel-server', 'manifest.json']) {
        const f = fixture(t);
        const file = path.join(f.directory, name), external = path.join(f.root, `external-${name}`);
        fs.renameSync(file, external); fs.symlinkSync(external, file);
        assert.equal(verifyArtifact(f).reused, false);
        assert.equal(fs.existsSync(f.destination), false);
    }
});
test('symbolic-link artifact directory is rejected on every platform', t => {
    const f = fixture(t), link = path.join(f.root, 'linked-artifact');
    fs.symlinkSync(f.directory, link, process.platform === 'win32' ? 'junction' : 'dir');
    assert.equal(verifyArtifact({ ...f, directory: link }).reused, false);
    assert.equal(fs.existsSync(f.destination), false);
});
test('invalid creation identity never creates an artifact', t => {
    const f = fixture(t), directory = path.join(f.root, 'invalid');
    assert.throws(() => createArtifact({ ...f, directory, identity: { ...identity, sha: 'main' } }));
    assert.equal(fs.existsSync(directory), false);
});
