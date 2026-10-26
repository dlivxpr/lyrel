import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import { mkdtempSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join, resolve } from "node:path"

assert.equal(process.platform, "win32", "此回归用例需要 Windows")
const root = mkdtempSync(join(resolve(process.argv[2] ?? tmpdir()), "lyrel-opencode-dacl-"))
const shell = join(process.env.SystemRoot, "System32", "WindowsPowerShell", "v1.0", "powershell.exe")
const source = readFileSync(new URL("./opencode_tui.ts", import.meta.url), "utf8")
  .replace('import { Plugin } from "@opencode/plugin/tui"', "const Plugin = { define: value => value }")
  .replace('import { createEffect } from "solid-js"', "const createEffect = () => {}")
  .replace("__EXE__", JSON.stringify(process.execPath))
const hostSource = readFileSync(new URL("../host/session_operations/opencode.rs", import.meta.url), "utf8")
const hostScript = JSON.parse(hostSource.match(/let script = ("[^\r\n]+");/)[1])
// 隔离模块模拟继承环境中不可加载的 Security 模块，不修改系统配置。
const modules = join(root, "modules")
const securityModule = join(modules, "Microsoft.PowerShell.Security")
mkdirSync(securityModule, { recursive: true })
writeFileSync(join(securityModule, "Microsoft.PowerShell.Security.psd1"), "@{ ModuleVersion='99.0'; RootModule='blocked.psm1'; FunctionsToExport=@('Set-Acl','Get-Acl') }")
writeFileSync(join(securityModule, "blocked.psm1"), "throw 'isolated Security module unavailable'")
process.env.PSModulePath = modules
process.env.LOCALAPPDATA = join(root, "私有 home [test] 'quoted'")
process.env.LYREL = "1"
process.env.LYREL_PANE = "test-pane"
// 此处加载的是本次读取、注入隔离入口的模块，静态 import 无法引用它。
const plugin = (await import(`data:text/javascript;base64,${Buffer.from(source).toString("base64")}`)).default
const context = {
  ui: { router: { current: () => ({ type: "home" }) }, tabs: { list: () => [] }, slot: () => () => {}, toast: { show() {} } },
  data: { listen: () => () => {} },
  client: { server: { info: () => new Promise(() => {}) } },
}
function powershell(script, directory) {
  return spawnSync(shell, ["-NoProfile", "-NonInteractive", "-Command", script], {
    env: { ...process.env, LYREL_OPENCODE_PRIVATE_DIR: directory }, encoding: "utf8", timeout: 10000, windowsHide: true,
  })
}
let dispose
try {
  dispose = plugin.setup(context)
  const clients = join(process.env.LOCALAPPDATA, "lyrel", "opencode-clients")
  const directory = join(clients, readdirSync(clients)[0])
  const registration = join(directory, "client.json")
  const deadline = Date.now() + 2000
  while (!readdirSync(directory).includes("client.json") && Date.now() < deadline) {
    await new Promise(resolve => setTimeout(resolve, 10))
  }
  const record = readFileSync(registration, "utf8")
  assert.equal(JSON.parse(record).token.length, 64)
  const inspect = powershell(`$ErrorActionPreference='Stop';
    $sid=[Security.Principal.WindowsIdentity]::GetCurrent().User.Value;
    foreach($p in @($env:LYREL_OPENCODE_PRIVATE_DIR,[IO.Path]::Combine($env:LYREL_OPENCODE_PRIVATE_DIR,'client.json'))) {
      $isDirectory=$p -eq $env:LYREL_OPENCODE_PRIVATE_DIR;
      $a=if($isDirectory) { [IO.Directory]::GetAccessControl($p) } else { [IO.File]::GetAccessControl($p) };
      if($a.GetOwner([Security.Principal.SecurityIdentifier]).Value -ne $sid) { throw 'incorrect owner' };
      if($isDirectory -and -not $a.AreAccessRulesProtected) { throw 'directory inheritance enabled' };
      $rules=$a.GetAccessRules($true,$true,[Security.Principal.SecurityIdentifier]);
      if($rules.Count -ne 2) { throw 'unexpected ACE count' };
      foreach($identity in @($sid,'S-1-5-18')) {
        $found=$false;
        foreach($r in $rules) {
          if($r.IdentityReference.Value -eq $identity) {
            $found=$true;
            if($r.AccessControlType -ne 'Allow' -or $r.FileSystemRights -ne 'FullControl') { throw 'incorrect access' };
            if($isDirectory -and ($r.IsInherited -or $r.InheritanceFlags -ne 'ContainerInherit,ObjectInherit')) { throw 'incorrect directory inheritance' };
            if(-not $isDirectory -and -not $r.IsInherited) { throw 'file did not inherit private access' }
          }
        };
        if(-not $found) { throw 'missing private identity' }
      }
    };
    [Console]::WriteLine('ACL_OK owner='+$sid+' allowed='+$sid+',S-1-5-18 protected=True fileInherited=True')`, directory)
  assert.equal(inspect.status, 0, inspect.stderr)
  console.log(inspect.stdout.trim())
  assert.equal(powershell(hostScript, directory).status, 0, "Host 必须接受真实私有注册")
  const broaden = powershell("$ErrorActionPreference='Stop'; $p=[IO.Path]::Combine($env:LYREL_OPENCODE_PRIVATE_DIR,'client.json'); $a=[IO.File]::GetAccessControl($p); $a.AddAccessRule([Security.AccessControl.FileSystemAccessRule]::new([Security.Principal.SecurityIdentifier]::new('S-1-1-0'),'Read','Allow')); [IO.File]::SetAccessControl($p,$a)", directory)
  assert.equal(broaden.status, 0, broaden.stderr)
  assert.notEqual(powershell(hostScript, directory).status, 0, "Host 必须拒绝其他 SID 的访问权限")
  rmSync(registration)
  writeFileSync(registration, record, { flag: "wx", mode: 0o600 })
  assert.equal(powershell(hostScript, directory).status, 0)
  const inherit = powershell("$ErrorActionPreference='Stop'; $p=$env:LYREL_OPENCODE_PRIVATE_DIR; $a=[IO.Directory]::GetAccessControl($p); $a.SetAccessRuleProtection($false,$true); [IO.Directory]::SetAccessControl($p,$a)", directory)
  assert.equal(inherit.status, 0, inherit.stderr)
  assert.notEqual(powershell(hostScript, directory).status, 0, "Host 必须拒绝未保护的目录继承")
  dispose()
  dispose = undefined
  assert.deepEqual(readdirSync(clients), [], "关闭后必须清除注册")
  const systemRoot = process.env.SystemRoot
  try {
    process.env.SystemRoot = join(root, "missing-system-root")
    assert.throws(() => plugin.setup(context), /cannot protect private client registration DACL/)
    assert.deepEqual(readdirSync(clients), [], "保护 DACL 失败后必须清除目录")
    delete process.env.SystemRoot
    assert.throws(() => plugin.setup(context), /cannot protect private client registration DACL/)
    assert.deepEqual(readdirSync(clients), [], "启动保护命令抛错后必须清除目录")
  } finally { process.env.SystemRoot = systemRoot }
  console.log("PASS: registration, strict host validation, normal cleanup, and failed-protection cleanup")
} finally {
  dispose?.()
  rmSync(root, { recursive: true, force: true })
}
