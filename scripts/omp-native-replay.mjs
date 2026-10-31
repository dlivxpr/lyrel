// Released omp native replay fixture. Only synthetic public JSONL v3 inputs.
import { createHash, randomUUID } from 'node:crypto';
import { createServer } from 'node:http';
import { mkdirSync, readFileSync, writeFileSync, appendFileSync, existsSync, rmSync, realpathSync, lstatSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { spawn, spawnSync } from 'node:child_process';
import { deflateSync } from 'node:zlib';
import { fileURLToPath } from 'node:url';
import { isatty } from 'node:tty';

const [action, ...args] = process.argv.slice(2);
const option = (name, fallback) => {
  const index = args.indexOf(name);
  if (index < 0) return fallback;
  if (!args[index + 1] || args[index + 1].startsWith('--')) throw new Error(`Missing ${name}`);
  return args[index + 1];
};
const root = resolve(option('--root', ''));
if (!args.includes('--root')) throw new Error('Required: --root <new isolated directory>');
const manifestPath = join(root, 'fixture.json');
const marker = 'lyrel-owned-omp-replay-148';
const json = (path, value) => writeFileSync(path, JSON.stringify(value, null, 2));
let lastStage;
function stage(name, detail = {}) {
  const value = {stage:name,time:new Date().toISOString(),pid:process.pid,...detail};
  lastStage = name;
  json(join(root,'stage.json'),value);
  appendFileSync(join(root,'stages.jsonl'),JSON.stringify(value) + '\n');
  console.error(`LYREL_OMP_STAGE ${JSON.stringify(value)}`);
}
process.on('uncaughtExceptionMonitor', error => {
  if (action === 'run') stage('failed',{during:lastStage,error:error.stack ?? String(error)});
});
function load() {
  const value = JSON.parse(readFileSync(manifestPath, 'utf8'));
  if (value.owner !== marker || realpathSync(root) !== value.root || lstatSync(root).isSymbolicLink()) {
    throw new Error('Not an owned fixture directory');
  }
  return value;
}
function crc32(bytes) {
  let crc = 0xffffffff;
  for (const byte of bytes) {
    crc ^= byte;
    for (let bit = 0; bit < 8; bit++) crc = (crc >>> 1) ^ ((crc & 1) ? 0xedb88320 : 0);
  }
  return (crc ^ 0xffffffff) >>> 0;
}
function chunk(type, data) {
  const name = Buffer.from(type);
  const length = Buffer.alloc(4); length.writeUInt32BE(data.length);
  const crc = Buffer.alloc(4); crc.writeUInt32BE(crc32(Buffer.concat([name, data])));
  return Buffer.concat([length, name, data, crc]);
}
function png(rgb) {
  const width = 64, height = 32;
  const scan = Buffer.alloc(height * (1 + width * 4));
  const bgra = Buffer.alloc(width * height * 4);
  for (let y = 0; y < height; y++) for (let x = 0; x < width; x++) {
    // Black centre cross makes identities visible as pictures rather than swatches/text.
    const color = x >= 30 && x < 34 || y >= 14 && y < 18 ? [0, 0, 0] : rgb;
    scan.set([...color, 255], y * (1 + width * 4) + 1 + x * 4);
    bgra.set([color[2], color[1], color[0], 255], (y * width + x) * 4);
  }
  const header = Buffer.alloc(13);
  header.writeUInt32BE(width); header.writeUInt32BE(height, 4); header[8] = 8; header[9] = 6;
  const bytes = Buffer.concat([Buffer.from([137,80,78,71,13,10,26,10]), chunk('IHDR', header), chunk('IDAT', deflateSync(scan)), chunk('IEND', Buffer.alloc(0))]);
  return { bytes, bgraHash: createHash('sha256').update(bgra).digest('hex') };
}
const palette = [[255,0,0],[0,255,0],[0,0,255],[255,255,0],[255,0,255],[0,255,255],[192,96,0],[96,0,192],[0,128,96],[128,128,128]];
const usage = { input:0, output:0, cacheRead:0, cacheWrite:0, totalTokens:0, cost:{input:0,output:0,cacheRead:0,cacheWrite:0,total:0} };
if (action === 'prepare') {
  const omp = resolve(option('--omp', 'omp.exe'));
  const scenario = option('--case', 'pictures');
  const mode = option('--mode', 'placeholders');
  if (!['pictures','budget','disabled','protocol-off','placeholder-off'].includes(scenario)) throw new Error('Unknown --case');
  if (!['placeholders','direct'].includes(mode)) throw new Error('Unknown --mode');
  mkdirSync(root); // Exclusive: never overwrite somebody else's data.
  const canonical = realpathSync(root);
  for (const name of ['home','agent','workspace','lyrel','config','data','state','cache','tmp']) mkdirSync(join(root, name));
  const session = join(root, 'workspace', 'pictures.jsonl');
  const now = Date.now();
  const entries = [{type:'session',version:3,id:randomUUID(),timestamp:new Date(now).toISOString(),cwd:join(root,'workspace'),title:'LYREL #148 synthetic pictures',titleSource:'user'}];
  let parentId = null;
  const append = (message) => {
    const id = (entries.length).toString(16).padStart(8,'0');
    entries.push({type:'message',id,parentId,timestamp:new Date(now + entries.length).toISOString(),message}); parentId = id;
  };
  const assistant = (content, stopReason = 'stop') => ({role:'assistant',api:'openai-completions',provider:'lyrel-fixture',model:'never-infer',content,usage,stopReason,timestamp:now});
  append({role:'user',content:[{type:'text',text:'Synthetic saved input; do not submit a new prompt.'}],timestamp:now});
  const identities = [];
  const count = scenario === 'budget' ? 10 : 1;
  // Released 18.14.0/18.14.1 keys assistant images by content slot (`native:${i}`),
  // not by message. Ten separate slot-1 messages alias one transmit-once image.
  // Put all ten assistant pictures in one native message to exercise 12 distinct
  // images against the real eight-image budget, without patching the renderer.
  const assistantContent = [];
  for (let index = 0; index < count; index++) {
    const image = png(palette[index]);
    identities.push({name:`assistant-${index + 1}`,rgb:palette[index],width:64,height:32,bgraSha256:image.bgraHash});
    assistantContent.push({type:'text',text:`LYREL_ASSISTANT_${index + 1}`},{type:'image',mimeType:'image/png',data:image.bytes.toString('base64')});
  }
  append(assistant(assistantContent));
  const toolImage = png([240,128,192]);
  identities.push({name:'tool-result',rgb:[240,128,192],width:64,height:32,bgraSha256:toolImage.bgraHash});
  append(assistant([{type:'toolCall',id:'lyrel-picture-read',name:'read',arguments:{path:'synthetic-picture.png'}}], 'toolUse'));
  append({role:'toolResult',toolCallId:'lyrel-picture-read',toolName:'read',content:[{type:'text',text:'LYREL_TOOL_RESULT'},{type:'image',mimeType:'image/png',data:toolImage.bytes.toString('base64')}],isError:false,timestamp:now});
  append(assistant([{type:'text',text:'LYREL_RASTERIZED_FIGURE\n```svg\n<svg xmlns="http://www.w3.org/2000/svg" width="240" height="80" viewBox="0 0 240 80"><rect width="240" height="80" fill="#123456"/><path d="M20 65 L120 10 L220 65 Z" fill="#ff8000"/></svg>\n```\nLYREL_REPLAY_END'}]));
  writeFileSync(session, entries.map(value => JSON.stringify(value)).join('\n') + '\n');
  const overlay = join(root, 'overlay.json');
  json(overlay, {terminal:{showImages:scenario !== 'disabled'},tui:{renderSvg:true,maxInlineImages:8,resizeScrollback:'rebuild'},startup:{quiet:true,setupWizard:false},recap:{enabled:false},compaction:{enabled:false},retry:{modelFallback:false}});
  const environment = {
    HOME:join(root,'home'),USERPROFILE:join(root,'home'),APPDATA:join(root,'config'),LOCALAPPDATA:join(root,'data'),
    XDG_CONFIG_HOME:join(root,'config'),XDG_DATA_HOME:join(root,'data'),XDG_STATE_HOME:join(root,'state'),XDG_CACHE_HOME:join(root,'cache'),
    TEMP:join(root,'tmp'),TMP:join(root,'tmp'),OMP_PROFILE:'default',PI_CONFIG_DIR:'.omp',PI_CODING_AGENT_DIR:join(root,'agent'),
    PI_FORCE_IMAGE_PROTOCOL:scenario === 'protocol-off' ? 'off' : 'kitty',PI_KITTY_PLACEHOLDERS:mode === 'direct' ? '0' : '1',PI_NO_TITLE:'1',
  };
  if (scenario === 'placeholder-off') environment.PI_NO_KITTY_PLACEHOLDERS = '1';
  const manifest = {owner:marker,root:canonical,omp,scenario,mode,session,overlay,environment,identities,initialMessages:entries.length - 1,
    figure:{backgroundRgb:[18,52,86],triangleRgb:[255,128,0]},upstreamReviewed:'18.14.1',latestReleaseObserved:'2026-10-31',
    argv:['--config',overlay,'--model','lyrel-fixture/never-infer','--no-extensions','--no-skills','--no-rules','--no-context-files','--no-lsp','--no-tools','--resume',session]};
  json(manifestPath, manifest);
  json(join(root,'lyrel','config.json'), {shell:{program:process.execPath,args:[fileURLToPath(import.meta.url),'run','--root',canonical]},restore_windows:false});
  console.log(JSON.stringify(manifest));
} else if (action === 'run') {
  const manifest = load();
  // Never read stdin in this wrapper: a pending Windows console/readline read
  // can compete with the native child even after pause(). Only omp owns stdin.
  json(join(root,'running.json'), {pid:process.pid,childPid:null});
  stage('run-entered',{scenario:manifest.scenario,omp:manifest.omp,session:manifest.session});
  if (args.includes('--wait-ready')) {
    const gate = join(root,'consumer-ready');
    stage('waiting-consumer',{gate});
    const deadline = Date.now() + 30000;
    while (!existsSync(gate)) {
      if (Date.now() >= deadline) throw new Error('Rooted consumer did not release the owned readiness gate');
      await new Promise(resolve => setTimeout(resolve,20));
    }
    stage('consumer-ready');
  }
  // Deliberately allow-list OS/terminal plumbing; no provider keys, proxy, profile,
  // extensions, private stores, dotenv overlays or credential-command variables.
  const allowed = ['PATH','PATHEXT','SystemRoot','SYSTEMROOT','WINDIR','COMSPEC','TERM','TERM_PROGRAM','TERM_PROGRAM_VERSION','COLORTERM','LYREL','LYREL_PANE_ID','ConEmuANSI','WT_SESSION'];
  const environment = {};
  for (const [key,value] of Object.entries(process.env)) if (allowed.some(name => name.toLowerCase() === key.toLowerCase())) environment[key] = value;
  Object.assign(environment, manifest.environment);
  // A TUI must inherit a real console, not wrapper-created stdin/stdout pipes.
  if (!isatty(0) || !isatty(1)) throw new Error('Native omp replay requires inherited TTY stdin and stdout');
  const inspect = (name, argv, extraEnvironment = {}) => {
    stage(name,{argv});
    const result = spawnSync(manifest.omp,argv,{env:{...environment,...extraEnvironment},cwd:join(root,'workspace'),encoding:'utf8',timeout:30000,maxBuffer:4 << 20});
    json(join(root,`${name}.json`),{argv,status:result.status,signal:result.signal,error:result.error?.message,stdout:result.stdout,stderr:result.stderr});
    if (result.status !== 0) throw new Error(`${name}: status=${result.status} signal=${result.signal} error=${result.error?.message ?? ''} stderr=${result.stderr} stdout=${result.stdout}`);
    return result.stdout.trim();
  };
  const version = inspect('version',['--version']);
  writeFileSync(join(root,'version.txt'),version + '\n');
  const configPath = inspect('config-path',['config','path']);
  const expectedConfig = join(root,'agent');
  if (resolve(configPath).toLowerCase() !== resolve(expectedConfig).toLowerCase()) throw new Error(`Public config path ${configPath} is not owned ${expectedConfig}`);
  for (const [key,expected] of [['terminal.showImages',manifest.scenario !== 'disabled'],['tui.renderSvg',true],['tui.maxInlineImages',8]]) {
    const {value:actual} = JSON.parse(inspect(`config-${key}`,['config','get',key,'--json'],{PI_CONFIG_FILES:manifest.overlay}));
    if (actual !== expected) throw new Error(`Public setting ${key}: expected ${expected}, got ${JSON.stringify(actual)}`);
  }
  const requests = [];
  const server = createServer((request,response) => {
    requests.push({method:request.method,url:request.url});
    json(join(root,'requests.json'),requests);
    stage('inference-forbidden',{method:request.method,url:request.url});
    response.writeHead(503); response.end('LYREL fixture forbids inference');
  });
  await new Promise(resolve => server.listen(0,'127.0.0.1',resolve));
  json(join(root,'agent','models.yml'), {providers:{'lyrel-fixture':{baseUrl:`http://127.0.0.1:${server.address().port}/v1`,api:'openai-completions',auth:'none',models:[{id:'never-infer',name:'Never infer (fixture)',reasoning:false,input:['text','image'],cost:{input:0,output:0,cacheRead:0,cacheWrite:0},contextWindow:128000,maxTokens:1024}]}}});
  const listing = JSON.parse(inspect('models',['models','lyrel-fixture','--json','--no-extensions','--config',manifest.overlay]));
  if (listing.models?.length !== 1 || listing.models[0].selector !== 'lyrel-fixture/never-infer') throw new Error(`Owned public provider was not selectable: ${JSON.stringify(listing)}`);
  stage('native-launch',{version,argv:manifest.argv,modelsPath:join(root,'agent','models.yml'),baseUrl:`http://127.0.0.1:${server.address().port}/v1`,stdinTTY:isatty(0),stdoutTTY:isatty(1)});
  const child = spawn(manifest.omp, manifest.argv, {env:environment,cwd:join(root,'workspace'),stdio:'inherit'});
  json(join(root,'running.json'), {pid:process.pid,childPid:child.pid});
  child.once('spawn',() => stage('native-spawned',{childPid:child.pid}));
  const stop = () => {
    if (child.exitCode !== null || !child.pid) return;
    if (process.platform === 'win32') spawnSync('taskkill',['/PID',String(child.pid),'/T','/F'],{stdio:'ignore'});
    else child.kill('SIGTERM');
  };
  process.on('SIGINT',stop); process.on('SIGTERM',stop); process.on('exit',stop);
  try {
    const code = await new Promise((resolve,reject) => {child.once('error',reject); child.once('exit',code => resolve(code));});
    stage('native-exited',{exitCode:code});
    const records = readFileSync(manifest.session,'utf8').trim().split('\n').map(line => JSON.parse(line));
    const messages = records.filter(record => record.type === 'message').length;
    const modelUsage = records.filter(record => record.type === 'model_usage').length;
    const audit = {version,requests,messages,initialMessages:manifest.initialMessages,modelUsage,exitCode:code,
      zeroInference:requests.length === 0 && messages === manifest.initialMessages && modelUsage === 0};
    json(join(root,'audit.json'),audit);
    stage('audited',audit);
    if (!audit.zeroInference || code !== 0) process.exitCode = 1;
    console.error(`\nLYREL_OMP_REPLAY_AUDIT ${JSON.stringify(audit)}`);
  } finally {
    stop(); server.close(); rmSync(join(root,'running.json'),{force:true});
    process.removeListener('SIGINT',stop); process.removeListener('SIGTERM',stop); process.removeListener('exit',stop);
  }
} else if (action === 'launch') {
  load();
  const app = option('--lyrel-app');
  if (!app) throw new Error('Required: --lyrel-app <absolute executable>');
  const environment = {};
  for (const [key,value] of Object.entries(process.env)) {
    if (['PATH','PATHEXT','SYSTEMROOT','WINDIR','COMSPEC','USERPROFILE','APPDATA','LOCALAPPDATA'].includes(key.toUpperCase())) environment[key] = value;
  }
  Object.assign(environment, {LYREL_CONFIG_DIR:join(root,'lyrel'),LYREL_DATA_DIR:join(root,'lyrel')});
  const appChild = spawn(resolve(app), ['--config-dir',join(root,'lyrel')], {env:environment,stdio:'ignore',detached:true});
  await new Promise((resolve,reject) => {appChild.once('spawn',resolve); appChild.once('error',reject);});
  appChild.unref();
  json(join(root,'app.json'), {pid:appChild.pid});
  console.log(`Owned lyrel GUI pid ${appChild.pid}; close this window before cleanup.`);
} else if (action === 'cleanup') {
  load();
  try {
    const active = JSON.parse(readFileSync(join(root,'running.json'),'utf8'));
    try { process.kill(active.pid,0); throw new Error('Fixture still running; exit its native TUI before cleanup'); }
    catch (error) { if (error.code !== 'ESRCH') throw error; }
  } catch (error) { if (error.code !== 'ENOENT') throw error; }
  const cli = option('--lyrel-cli');
  if (!cli) throw new Error('Required: --lyrel-cli <absolute executable> to stop only this fixture daemon');
  try {
    const app = JSON.parse(readFileSync(join(root,'app.json'),'utf8'));
    try { process.kill(app.pid,0); throw new Error('Close the owned lyrel window before cleanup'); }
    catch (error) { if (error.code !== 'ESRCH') throw error; }
  } catch (error) { if (error.code !== 'ENOENT') throw error; }
  const environment = {LYREL_CONFIG_DIR:join(root,'lyrel'),LYREL_DATA_DIR:join(root,'lyrel')};
  for (const [key,value] of Object.entries(process.env)) {
    if (['PATH','PATHEXT','SYSTEMROOT','WINDIR','COMSPEC'].includes(key.toUpperCase())) environment[key] = value;
  }
  const stopped = spawnSync(resolve(cli), ['server','stop'], {env:environment,encoding:'utf8',timeout:30000});
  if (stopped.status !== 0) throw new Error(`Owned daemon shutdown failed: ${stopped.stderr || stopped.error}`);
  rmSync(root,{recursive:true});
} else throw new Error('Usage: node scripts/omp-native-replay.mjs prepare|run|launch|cleanup --root <directory> [--omp <absolute executable>] [--lyrel-app <absolute executable>] [--lyrel-cli <absolute executable>] [--case pictures|budget|disabled|protocol-off|placeholder-off] [--mode placeholders|direct]');
