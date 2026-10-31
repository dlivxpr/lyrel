#!/usr/bin/env node
// Run only in a disposable lyrel pane; this never reads histories or writes images to disk.
import { once } from 'node:events';
if (!process.stdout.isTTY) throw new Error('Run directly in an isolated lyrel terminal');
const stage = process.argv[2] || 'scene';
const apc = header => `\x1b_G${header},q=2\x1b\\`;
let bytes;
switch (stage) {
  case 'scene': {
    // IDs are fixture-owned, placement 1. Red/green/blue/white 2x2 RGBA.
    const pixels = '/wAA/wD/AP8AAP///////w==';
    bytes = '\x1b[2J\x1b[3J\x1b[5;8H' +
      `\x1b_Ga=T,i=14401,p=1,c=4,r=2,C=1,f=32,s=2,v=2,q=2;${pixels}\x1b\\` +
      apc('a=p,i=14401,p=2,P=14401,Q=1,H=-3,V=3,c=4,r=2') +
      apc('a=p,i=14401,p=3,P=14401,Q=2,H=6,V=-1,x=1,w=1,c=2,r=2,z=-1') +
      '\x1b_Ga=T,i=14402,p=1,U=1,c=1,r=1,f=32,s=1,v=1,q=2;AAAAAA==\x1b\\' +
      apc('a=p,i=14401,p=4,P=14402,Q=1,H=2,V=1,c=4,r=2') +
      '\x1b[12;15H\x1b[38;2;0;56;66;58;5;1m\u{10eeee}\u{305}\u{305}\x1b[0m' +
      '\x1b[18;1HKITTY_RELATIVE_SCENE: direct=(8,5), child=(5,8), crop=(11,7), virtual-child=(17,13).\r\n' +
      'Sender exits now. Reopen the same living daemon without scene/retransmission.\r\n';
    break;
  }
  case 'move':
    bytes = '\x1b[6;10H' + apc('a=p,i=14401,p=1,c=4,r=2,C=1') +
      '\x1b[12;1H\x1b[L\x1b[20;1HKITTY_RELATIVE_MOVED: direct=(10,6), child=(7,9), crop=(13,8), virtual-child=(17,14).\r\n';
    break;
  case 'erase':
    bytes = '\x1b[12;1H\x1b[2K\x1b[13;1H\x1b[2K\x1b[20;1HKITTY_RELATIVE_ERASED: virtual-child must disappear, prototype remains.\r\n';
    break;
  case 'delete':
    bytes = apc('a=d,d=i,i=14401,p=1') + apc('a=d,d=i,i=14402,p=1') +
      '\x1b[20;1HKITTY_RELATIVE_DELETED: all fixture pixels stay absent after reopening.\r\n';
    break;
  default: throw new Error('Usage: node scripts/kitty-relative.mjs [scene|move|erase|delete]');
}
if (!process.stdout.write(bytes)) await once(process.stdout, 'drain');
