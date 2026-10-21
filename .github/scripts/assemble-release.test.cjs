'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { spawnSync } = require('node:child_process');

const commit = 'a'.repeat(40);
const tagObject = 'b'.repeat(40);
const names = [
    'lyrel-0.1.2-macos-arm64.dmg', 'lyrel-0.1.2-macos-arm64.zip',
    'lyrel-0.1.2-windows-x86_64-setup.exe', 'lyrel-0.1.2-windows-x86_64.zip',
    'lyrel-server-linux-aarch64-musl', 'lyrel-server-linux-x86_64-musl',
    'lyrel-server-macos-aarch64', 'lyrel-server-windows-x86_64.zip',
];
const github = String.raw`#!/usr/bin/env node
const fs = require('node:fs');
const crypto = require('node:crypto');
const args = process.argv.slice(2);
const file = process.env.RELEASE_FIXTURE;
const state = JSON.parse(fs.readFileSync(file, 'utf8'));
const endpoint = args.find(a => a.startsWith('repos/') || a.startsWith('https://uploads.github.com/'));
const method = args.includes('--method') ? args[args.indexOf('--method') + 1] : 'GET';
const fields = {};
for (let i = 0; i < args.length; i++) {
    if (args[i] === '-f' || args[i] === '-F') {
        const field = args[++i], equal = field.indexOf('=');
        fields[field.slice(0, equal)] = field.slice(equal + 1);
    }
}
function respond(value) {
    fs.writeFileSync(file, JSON.stringify(state));
    console.log(JSON.stringify(value));
}
if (endpoint === 'repos/owner/repo/git/ref/tags/v0.1.2') {
    respond({ object: state.ref });
} else if (endpoint?.startsWith('repos/owner/repo/git/tags/')) {
    const object = state.tags[endpoint.split('/').pop()];
    if (!object) { console.error('Unknown tag object'); process.exit(1); }
    respond({ object });
} else if (endpoint === 'repos/owner/repo/releases?per_page=100') {
    respond([state.release ? [state.release] : []]);
} else if (endpoint === 'repos/owner/repo/releases' && method === 'POST') {
    state.creates++;
    // Model the real API failure: tag names are invalid release creation targets.
    if (fields.target_commitish !== state.commit) {
        respond({ message: 'Validation Failed', errors: [{ resource: 'Release', field: 'target_commitish', code: 'invalid' }] });
        console.error('gh: Validation Failed (HTTP 422)');
        process.exit(1);
    }
    state.release = { id: 17, name: fields.name, tag_name: fields.tag_name,
        target_commitish: fields.target_commitish, draft: fields.draft === 'true', prerelease: fields.prerelease === 'true' };
    respond(state.release);
} else if (endpoint === 'repos/owner/repo/releases/17') {
    respond(state.release);
} else if (endpoint === 'repos/owner/repo/releases/17/assets?per_page=100') {
    respond([state.assets]);
} else if (endpoint?.startsWith('https://uploads.github.com/repos/owner/repo/releases/17/assets?name=') && method === 'POST') {
    const name = new URL(endpoint).searchParams.get('name');
    const bytes = fs.readFileSync(args[args.indexOf('--input') + 1]);
    if (state.assets.some(asset => asset.name === name)) { console.error('Duplicate asset'); process.exit(1); }
    const asset = { name, state: 'uploaded', digest: 'sha256:' + crypto.createHash('sha256').update(bytes).digest('hex') };
    state.assets.push(asset);
    respond(asset);
} else {
    console.error('Unexpected GitHub request: ' + method + ' ' + endpoint);
    process.exit(1);
}
`;
function fixture(t, { ref = { type: 'tag', sha: tagObject }, tags = { [tagObject]: { type: 'commit', sha: commit } } } = {}) {
    const root = fs.mkdtempSync(path.join(fs.realpathSync(os.tmpdir()), 'assemble-release-'));
    t.after(() => fs.rmSync(root, { recursive: true, force: true }));
    const bin = path.join(root, 'bin'), assets = path.join(root, 'assets'), state = path.join(root, 'state.json');
    fs.mkdirSync(bin); fs.mkdirSync(assets);
    fs.writeFileSync(path.join(bin, 'gh'), github, { mode: 0o755 });
    for (const name of names) fs.writeFileSync(path.join(assets, name), `production fixture: ${name}\n`);
    fs.writeFileSync(state, JSON.stringify({ ref, tags, commit, creates: 0, release: null, assets: [] }));
    return {
        state: () => JSON.parse(fs.readFileSync(state, 'utf8')),
        run: () => spawnSync('bash', [path.join(__dirname, 'assemble-release.sh').replaceAll('\\', '/'), 'v0.1.2', assets.replaceAll('\\', '/')], {
            encoding: 'utf8', timeout: 30000,
            env: { ...process.env, PATH: bin + path.delimiter + process.env.PATH, GITHUB_REPOSITORY: 'owner/repo', DRAFT_RELEASE_ID: '', RELEASE_FIXTURE: state },
        }),
    };
}
test('a new draft for an annotated tag assembles verified assets at its commit, not the tag object', t => {
    const f = fixture(t);
    const result = f.run();
    assert.equal(result.status, 0, result.error?.message || result.stdout + result.stderr);
    const state = f.state();
    assert.equal(state.release.target_commitish, commit);
    assert.equal(state.release.name, 'v0.1.2');
    assert.equal(state.release.draft, true);
    assert.deepEqual(state.assets.map(asset => asset.name).sort(), [...names, 'checksums.txt'].sort());
});

test('a lightweight tag assembles the release directly at its commit', t => {
    const f = fixture(t, { ref: { type: 'commit', sha: commit }, tags: {} });
    const result = f.run();
    assert.equal(result.status, 0, result.error?.message || result.stdout + result.stderr);
    assert.equal(f.state().release.target_commitish, commit);
    assert.deepEqual(f.state().assets.map(asset => asset.name).sort(), [...names, 'checksums.txt'].sort());
});

for (const [name, options] of [
    ['tree target', { ref: { type: 'tree', sha: commit }, tags: {} }],
    ['malformed commit SHA', { ref: { type: 'commit', sha: 'main' }, tags: {} }],
    ['cyclic tag objects', { tags: { [tagObject]: { type: 'tag', sha: tagObject } } }],
]) {
    test(`rejects ${name} without creating a release or uploading assets`, t => {
        const f = fixture(t, options);
        const result = f.run();
        assert.equal(result.status, 1, result.error?.message || result.stdout + result.stderr);
        const state = f.state();
        assert.equal(state.creates, 0);
        assert.equal(state.release, null);
        assert.deepEqual(state.assets, []);
    });
}
