import fs from 'node:fs';
import path from 'node:path';
import assert from 'node:assert/strict';
import {fileURLToPath} from 'node:url';

const root=path.resolve(path.dirname(fileURLToPath(import.meta.url)),'..');
const categories=['paletteColors','hexColors','nativeControls'];
const palettes='slate|gray|zinc|neutral|stone|red|orange|amber|yellow|lime|green|emerald|teal|cyan|sky|blue|indigo|violet|purple|fuchsia|pink|rose|black|white';
const utilities='bg|text|border(?:-[trblxy])?|ring(?:-offset)?|outline|shadow|decoration|fill|stroke|from|via|to|divide|placeholder|caret|accent';
export function scanUiSource(source) {
  return {
    paletteColors:[...source.matchAll(new RegExp(`\\b(?:${utilities})-(?:${palettes})(?:-(?:50|[1-9]00|950))?\\b`,'g'))].length,
    hexColors:[...source.matchAll(/(?<![\w])#(?:[\da-f]{8}|[\da-f]{6}|[\da-f]{4}|[\da-f]{3})(?![\da-f])/gi)].length,
    nativeControls:[...source.matchAll(/<(?:select|textarea|progress|details)\b|<input\b[^>]*\btype\s*=\s*(?:["']checkbox["']|\{\s*["']checkbox["']\s*\})/g)].length,
  };
}
function violations(file,counts,baseline) {
  return categories.filter(category=>counts[category]>(baseline[file]?.[category]??0))
    .map(category=>`${file}: ${category} ${counts[category]} exceeds ${baseline[file]?.[category]??0}`);
}
function files(folder) {
  return fs.readdirSync(folder,{withFileTypes:true}).flatMap(entry=>{
    const absolute=path.join(folder,entry.name);
    const relative=path.relative(root,absolute).replaceAll(path.sep,'/');
    if(relative==='frontend/src/components/ui') return [];
    return entry.isDirectory()?files(absolute):relative.endsWith('.tsx')?[relative]:[];
  }).sort();
}
function selfTest() {
  const counts=scanUiSource('<div className="bg-green-500 text-[#f00]"><select /><textarea /><progress /><details /><input type={"checkbox"} /></div>');
  assert.deepEqual(counts,{paletteColors:1,hexColors:1,nativeControls:5});
  const clean=scanUiSource('<div className="bg-card text-muted-foreground border-border"><Checkbox /><Select /><Textarea /><Progress /></div>');
  assert.deepEqual(clean,{paletteColors:0,hexColors:0,nativeControls:0});
  assert.deepEqual(violations('old.tsx',counts,{'old.tsx':counts}),[]);
  assert.equal(violations('old.tsx',{...counts,paletteColors:2},{'old.tsx':counts}).length,1);
  assert.equal(violations('new.tsx',counts,{}).length,3);
  assert.deepEqual(violations('new.tsx',clean,{}),[]);
  assert.equal(scanUiSource('<input\n type="checkbox" />').nativeControls,1);
  console.log('UI convention scanner self-tests passed.');
}
if(process.argv[1]&&path.resolve(process.argv[1])===fileURLToPath(import.meta.url)) {
  if(process.argv.includes('--self-test')) selfTest();
  const baseline=JSON.parse(fs.readFileSync(path.join(root,'scripts/ui-conventions-baseline.json'),'utf8'));
  assert.equal(baseline.schema,1,'Unknown UI baseline schema');
  assert.match(baseline.sourceCommit,/^[0-9a-f]{40}$/,'Baseline must identify its source commit');
  for(const [file,counts] of Object.entries(baseline.files)) {
    assert(file.startsWith('frontend/src/')&&file.endsWith('.tsx')&&!file.startsWith('frontend/src/components/ui/'),'Invalid baseline path');
    assert(categories.every(category=>Number.isInteger(counts[category])&&counts[category]>=0),'Invalid baseline counts');
  }
  const scanned=files(path.join(root,'frontend/src'));
  const failures=scanned.flatMap(file=>violations(file,scanUiSource(fs.readFileSync(path.join(root,file),'utf8')),baseline.files));
  if(failures.length) { console.error(failures.join('\n')); process.exitCode=1; }
  else console.log(`UI conventions passed for ${scanned.length} files; no file increased its baseline counts.`);
}
