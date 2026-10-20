// Host boundary module traversal. Invoked by check-host-boundary.sh, not a test harness.
const fs = require('node:fs');
const path = require('node:path').posix;
const files = new Set();
function collect(root) {
    for (const entry of fs.readdirSync(root, {withFileTypes: true})) {
        const file = path.join(root, entry.name);
        if (entry.isDirectory()) collect(file);
        else if (file.endsWith('.rs')) files.add(file);
    }
}
['src/ui', 'src/terminal'].forEach(collect);
if (files.size < 10) throw Error('check-host-boundary: fewer than 10 files; scan roots look wrong');

// Strings/comments are units: fixture braces cannot manufacture Rust modules.
function lex(text) {
    const tokens = [];
    const token = /\/\/[^\n]*|\/\*|(?:br|cr|r)(#*)"|(?:b|c)?"(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])'|[A-Za-z_][A-Za-z_0-9]*|\S/gs;
    let match;
    while ((match = token.exec(text))) {
        let value = match[0];
        const start = match.index;
        if (value.startsWith('//')) continue;
        if (value === '/*') {
            const comment = /\/\*|\*\//g;
            comment.lastIndex = token.lastIndex;
            let depth = 1, part;
            while (depth && (part = comment.exec(text))) depth += part[0] === '/*' ? 1 : -1;
            if (depth) throw Error('check-host-boundary: unterminated Rust comment');
            token.lastIndex = comment.lastIndex;
            continue;
        }
        if (match[1] !== undefined) {
            const terminator = '"' + match[1];
            const end = text.indexOf(terminator, token.lastIndex);
            if (end < 0) throw Error('check-host-boundary: unterminated Rust raw string');
            token.lastIndex = end + terminator.length;
            value = text.slice(start, token.lastIndex);
        }
        tokens.push({value, start, end: token.lastIndex});
    }
    return tokens;
}
// Evaluate with test=false and every platform/feature predicate unknown. Only a
// provably false cfg is test-only; any(test, unix) must remain production.
function testOnlyCfg(attr) {
    if (attr[0] !== 'cfg' || attr[1] !== '(' || attr.at(-1) !== ')') return false;
    let ix = 2;
    function predicate() {
        const name = attr[ix++];
        if (attr[ix] === '=') { ix += 2; return undefined; }
        if (attr[ix] !== '(') return name === 'test' ? false : undefined;
        ix++;
        const args = [];
        while (ix < attr.length && attr[ix] !== ')') {
            args.push(predicate());
            if (attr[ix] === ',') ix++;
            else if (attr[ix] !== ')') return undefined;
        }
        ix++;
        if (name === 'all') return args.includes(false) ? false : args.every(v => v === true) ? true : undefined;
        if (name === 'any') return args.includes(true) ? true : args.every(v => v === false) ? false : undefined;
        if (name === 'not' && args.length === 1) return args[0] === undefined ? undefined : !args[0];
        return undefined;
    }
    return predicate() === false && ix === attr.length-1;
}
const texts = new Map(), edges = new Map(), masks = new Map();
function parse(file) {
    const text = fs.readFileSync(file, 'utf8'), tokens = lex(text);
    texts.set(file, text); edges.set(file, []); masks.set(file, []);
    function close(ix, opening, closing) {
        let depth = 1;
        while (++ix < tokens.length) {
            if (tokens[ix].value === opening) depth++;
            if (tokens[ix].value === closing) depth--;
            if (!depth) return ix;
        }
        throw Error(`check-host-boundary: unmatched ${opening} in ${file}`);
    }
    function walk(begin, end, moduleDir, pathDir, testOnly = false) {
        let ix = begin;
        while (ix < end) {
            const start = ix, attrs = [];
            while (ix+1 < end && tokens[ix].value === '#' && tokens[ix+1].value === '[') {
                const last = close(ix+1, '[', ']');
                attrs.push(tokens.slice(ix+2, last).map(t => t.value));
                ix = last+1;
            }
            const item = ix;
            if (tokens[ix]?.value === 'pub') {
                ix++;
                if (tokens[ix]?.value === '(') ix = close(ix, '(', ')')+1;
            }
            if (ix+2 >= end || tokens[ix].value !== 'mod' || ![';', '{'].includes(tokens[ix+2].value)) {
                ix = Math.max(start+1, item);
                continue;
            }
            const name = tokens[ix+1].value;
            // Unknown predicates remain conservative: names/cfg_attr never grant exemptions.
            const isTest = testOnly || attrs.some(testOnlyCfg);
            const explicit = attrs.find(a => a.length === 3 && a[0] === 'path' && a[1] === '=' && a[2].startsWith('"'));
            let last;
            if (tokens[ix+2].value === ';') {
                const candidates = explicit ? [path.join(pathDir, JSON.parse(explicit[2]))] : [path.join(moduleDir, name+'.rs'), path.join(moduleDir, name, 'mod.rs')];
                for (const child of candidates) if (files.has(child)) edges.get(file).push([child, isTest]);
                last = ix+2;
            } else {
                last = close(ix+2, '{', '}');
                walk(ix+3, last, path.join(moduleDir, name), path.join(moduleDir, name), isTest);
            }
            if (isTest) masks.get(file).push([tokens[start].start, tokens[last].end]);
            ix = last+1;
        }
    }
    const directory = path.basename(file) === 'mod.rs' ? path.dirname(file) : path.join(path.dirname(file), path.basename(file, '.rs'));
    walk(0, tokens.length, directory, path.dirname(file));
}
for (const file of files) parse(file);
const testDescendants = new Set([...edges.values()].flat().filter(([, test]) => test).map(([child]) => child));
let pending = [...testDescendants];
while (pending.length) for (const [child] of edges.get(pending.pop())) if (!testDescendants.has(child)) {
    testDescendants.add(child); pending.push(child);
}
// Files also imported by production and unreferenced files remain in scope.
const production = new Set([...files].filter(file => !testDescendants.has(file)));
pending = [...production];
while (pending.length) for (const [child, test] of edges.get(pending.pop())) if (!test && !production.has(child)) {
    production.add(child); pending.push(child);
}
const allow = process.env.HOST_BOUNDARY_ALLOW.split(/\r?\n/).filter(line => line && !line.startsWith('#')).map(line => {
    const split = line.indexOf('|'); return [line.slice(0, split), line.slice(split+1)];
});
const pattern = /std::fs::|Command::new\("git"\)|\.canonicalize\(\)|\.is_absolute\(\)/;
let violations = 0;
for (const file of [...production].sort()) {
    let text = texts.get(file);
    for (const [start, end] of masks.get(file).sort((a,b) => b[0]-a[0])) text = text.slice(0,start) + text.slice(start,end).replace(/[^\n]/g,' ') + text.slice(end);
    text.split(/\r?\n/).forEach((line, ix) => {
        if (line.trimStart().startsWith('//') || !pattern.test(line)) return;
        if (allow.some(([entry, pat]) => entry === file && (pat === '*' || line.includes(pat)))) return;
        console.error(`${file}:${ix+1}:${line}`); violations++;
    });
}
if (violations) {
    console.error('Host boundary violations: route workspace reads through Host / ui::host_ops; only proven local paths may be allowlisted');
    process.exit(1);
}
console.log(`host boundary clean: ${production.size} production files, ${files.size-production.size} test-only files excluded`);
