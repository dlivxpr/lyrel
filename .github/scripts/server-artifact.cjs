'use strict';
const fs = require('node:fs');
const path = require('node:path');
const crypto = require('node:crypto');
const { execFileSync } = require('node:child_process');
const { isDeepStrictEqual } = require('node:util');
const TARGETS = new Set(['x86_64-unknown-linux-musl', 'aarch64-unknown-linux-musl', 'aarch64-apple-darwin']);
const digest = bytes => crypto.createHash('sha256').update(bytes).digest('hex');
function requireValue(condition, reason) { if (!condition) throw Error(reason); }
function validateIdentity(identity) {
    requireValue(identity && /^[0-9a-f]{40}$/.test(identity.sha) && /^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(identity.repository), 'invalid source identity');
    for (const key of ['repositoryId', 'runId', 'runAttempt']) {
        requireValue(/^[1-9][0-9]*$/.test(identity[key]) && Number.isSafeInteger(Number(identity[key])), `invalid ${key}`);
    }
}
function checkPath(file) {
    const absolute = path.resolve(file), parsed = path.parse(absolute);
    let current = parsed.root;
    for (const part of absolute.slice(parsed.root.length).split(path.sep).filter(Boolean)) {
        current = path.join(current, part);
        try { requireValue(!fs.lstatSync(current).isSymbolicLink(), `symbolic link refused: ${current}`); }
        catch (error) { if (error.code !== 'ENOENT') throw error; }
    }
    return absolute;
}
function readRegular(file) {
    const absolute = checkPath(file);
    requireValue(fs.lstatSync(absolute).isFile(), `not a regular file: ${absolute}`);
    const fd = fs.openSync(absolute, fs.constants.O_RDONLY | (fs.constants.O_NOFOLLOW || 0));
    try { requireValue(fs.fstatSync(fd).isFile(), 'file changed type'); return fs.readFileSync(fd); }
    finally { fs.closeSync(fd); }
}
function createArtifact({ target, binary, directory, identity, contract }) {
    requireValue(TARGETS.has(target), 'unsupported server target');
    validateIdentity(identity);
    const bytes = readRegular(binary);
    requireValue(bytes.length > 0, 'empty server binary');
    const manifest = { format: 1, target, identity, contract, binary: { name: 'lyrel-server', size: bytes.length, sha256: digest(bytes) } };
    const output = checkPath(directory);
    // Never merge with old artifacts or follow an existing directory supplied by a caller.
    fs.mkdirSync(output, { recursive: false });
    fs.writeFileSync(path.join(output, 'lyrel-server'), bytes, { flag: 'wx', mode: 0o755 });
    fs.writeFileSync(path.join(output, 'manifest.json'), `${JSON.stringify(manifest, null, 2)}\n`, { flag: 'wx' });
    return manifest;
}
function verifyArtifact({ target, directory, destination, identity, contract }) {
    try {
        requireValue(TARGETS.has(target), 'unsupported server target');
        validateIdentity(identity);
        const input = checkPath(directory);
        requireValue(fs.lstatSync(input).isDirectory(), 'artifact is not a directory');
        requireValue(isDeepStrictEqual(fs.readdirSync(input).sort(), ['lyrel-server', 'manifest.json']), 'artifact must contain exactly the two fixed regular files');
        const manifestBytes = readRegular(path.join(input, 'manifest.json'));
        requireValue(manifestBytes.length <= 1024 * 1024, 'manifest is too large');
        const manifest = JSON.parse(manifestBytes.toString('utf8'));
        requireValue(manifest?.format === 1 && manifest.target === target && isDeepStrictEqual(manifest.identity, identity), 'artifact provenance mismatch');
        requireValue(isDeepStrictEqual(manifest.contract, contract), 'toolchain or build-input contract mismatch');
        requireValue(manifest.binary?.name === 'lyrel-server' && /^[0-9a-f]{64}$/.test(manifest.binary.sha256), 'invalid binary metadata');
        const bytes = readRegular(path.join(input, 'lyrel-server'));
        requireValue(bytes.length > 0 && manifest.binary.size === bytes.length && digest(bytes) === manifest.binary.sha256, 'binary digest mismatch');
        const output = checkPath(destination);
        fs.mkdirSync(path.dirname(output), { recursive: true });
        // Write the exact bytes just verified, rather than rereading a mutable source pathname.
        const fd = fs.openSync(output, fs.constants.O_WRONLY | fs.constants.O_CREAT | fs.constants.O_TRUNC | (fs.constants.O_NOFOLLOW || 0), 0o755);
        try { requireValue(fs.fstatSync(fd).isFile(), 'destination is not a regular file'); fs.writeFileSync(fd, bytes); fs.fchmodSync(fd, 0o755); }
        finally { fs.closeSync(fd); }
        return { reused: true };
    } catch (error) { return { reused: false, reason: error.message }; }
}
function buildContract(target, sourceSha) {
    requireValue(TARGETS.has(target), 'unsupported server target');
    const run = (file, args) => execFileSync(file, args, { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] }).trim();
    requireValue(run('git', ['rev-parse', 'HEAD']) === sourceSha, 'checkout does not match immutable source SHA');
    const inputs = {};
    const files = execFileSync('git', ['ls-files', '-z', '--', 'Cargo.lock', 'Cargo.toml', 'LICENSE', '.cargo', 'crates', '.github/workflows/ci.yml', '.github/workflows/release-build.yml', '.github/scripts/server-artifact.cjs'], { encoding: 'utf8' }).split('\0').filter(Boolean).sort();
    requireValue(['Cargo.lock', 'Cargo.toml', 'LICENSE'].every(file => files.includes(file)), 'missing workspace inputs');
    for (const file of files) inputs[file] = digest(readRegular(file));
    const musl = target.endsWith('-musl'), environment = {};
    for (const key of Object.keys(process.env).sort()) {
        if (/^(RUSTFLAGS|CARGO_ENCODED_RUSTFLAGS|CARGO_PROFILE_RELEASE_.*|CARGO_TARGET_.*_(RUSTFLAGS|LINKER)|CC|CXX|AR|CFLAGS|CXXFLAGS|MACOSX_DEPLOYMENT_TARGET|SDKROOT)$/.test(key)) environment[key] = process.env[key];
    }
    const contract = {
        rustc: run('rustc', ['-vV']), cargo: run('cargo', ['--version']),
        args: [musl ? 'zigbuild' : 'build', '--release', '--locked', '-p', 'lyrel-server', '--target', target],
        rustflags: process.env.RUSTFLAGS || '', environment, inputs
    };
    if (musl) {
        requireValue(process.env.CARGO_ZIGBUILD_PYTHON_PATH, 'missing Zig Python environment');
        contract.zig = run(process.env.CARGO_ZIGBUILD_PYTHON_PATH, ['-m', 'ziglang', 'version']);
        contract.zigbuild = run('cargo', ['zigbuild', '--version']);
    } else contract.sdk = run('xcrun', ['--sdk', 'macosx', '--show-sdk-version']);
    return contract;
}
function identityFromEnvironment(create) {
    return {
        sha: create ? process.env.GITHUB_SHA : process.env.LYREL_SERVER_SOURCE_SHA,
        repository: process.env.GITHUB_REPOSITORY,
        repositoryId: create ? process.env.GITHUB_REPOSITORY_ID : process.env.SERVER_REUSE_REPOSITORY_ID,
        runId: create ? process.env.GITHUB_RUN_ID : process.env.SERVER_REUSE_RUN_ID,
        runAttempt: create ? process.env.GITHUB_RUN_ATTEMPT : process.env.SERVER_REUSE_RUN_ATTEMPT
    };
}
function main() {
    const [command, target, first, second, ...extra] = process.argv.slice(2);
    requireValue(['create', 'verify'].includes(command) && target && first && second && extra.length === 0,
        'usage: server-artifact.cjs create <target> <binary> <directory> | verify <target> <directory> <destination>');
    if (command === 'create') {
        const identity = identityFromEnvironment(true);
        validateIdentity(identity);
        createArtifact({ target, binary: first, directory: second, identity, contract: buildContract(target, identity.sha) });
        console.log(`Created ci-server-${target} in ${second}`);
    } else {
        let result;
        try {
            const identity = identityFromEnvironment(false);
            validateIdentity(identity);
            result = verifyArtifact({ target, directory: first, destination: second, identity, contract: buildContract(target, identity.sha) });
        }
        catch (error) { result = { reused: false, reason: error.message }; }
        if (result.reason) console.log(`Server reuse unavailable; compiling normally: ${result.reason}`);
        const output = `reused=${result.reused}\n`;
        if (process.env.GITHUB_OUTPUT) fs.appendFileSync(process.env.GITHUB_OUTPUT, output);
        else process.stdout.write(output);
    }
}
if (require.main === module) { try { main(); } catch (error) { console.error(error.message); process.exitCode = 1; } }
module.exports = { createArtifact, verifyArtifact };
