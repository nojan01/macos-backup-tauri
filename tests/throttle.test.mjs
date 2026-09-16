import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import ts from 'typescript';
function load(name){
 const source=fs.readFileSync(new URL(`../src/${name}`,import.meta.url),'utf8');
 const output=ts.transpileModule(source,{compilerOptions:{module:ts.ModuleKind.ESNext,target:ts.ScriptTarget.ES2020}}).outputText;
 return import(`data:text/javascript;base64,${Buffer.from(output).toString('base64')}`);
}
const {normalizeThrottleMbPerS,THROTTLE_MIN_MB_PER_S,THROTTLE_MAX_MB_PER_S,THROTTLE_DEFAULT_MB_PER_S}=await load('throttle-ui.ts');
const {localizeMessage}=await load('messages.ts');

test('bounds mirror the backend limits',()=>{
 assert.equal(THROTTLE_MIN_MB_PER_S,1);assert.equal(THROTTLE_MAX_MB_PER_S,5000);assert.equal(THROTTLE_DEFAULT_MB_PER_S,80);
 const rust=fs.readFileSync(new URL('../src-tauri/src/throttle.rs',import.meta.url),'utf8');
 assert.match(rust,/MIN_MB_PER_S: u32 = 1;/);assert.match(rust,/MAX_MB_PER_S: u32 = 5000;/);assert.match(rust,/DEFAULT_MB_PER_S: u32 = 80;/);
});
test('valid input is accepted as whole MB/s',()=>{
 assert.equal(normalizeThrottleMbPerS('120'),120);
 assert.equal(normalizeThrottleMbPerS(' 45 '),45);
 assert.equal(normalizeThrottleMbPerS(64),64);
 assert.equal(normalizeThrottleMbPerS('12.6'),13);
});
test('out-of-range values are clamped instead of rejected',()=>{
 assert.equal(normalizeThrottleMbPerS('0'),1);
 assert.equal(normalizeThrottleMbPerS('-5'),1);
 assert.equal(normalizeThrottleMbPerS('99999'),5000);
});
test('garbage and empty input fall back to the previous or default value',()=>{
 assert.equal(normalizeThrottleMbPerS(''),80);
 assert.equal(normalizeThrottleMbPerS('abc'),80);
 assert.equal(normalizeThrottleMbPerS(null),80);
 assert.equal(normalizeThrottleMbPerS(undefined,150),150);
 assert.equal(normalizeThrottleMbPerS(NaN,33),33);
});
test('throttle log and heartbeat messages are translated',()=>{
 assert.equal(localizeMessage('🌡️ Durchsatzbegrenzung aktiv: 80 MB/s','en'),'🌡️ Throughput limit active: 80 MB/s');
 assert.equal(localizeMessage('Archiv erstellen und komprimieren: Fotos · 1:05 min · 512.3 MiB geschrieben · gedrosselt auf 80 MB/s','en'),
  'Creating and compressing archive: Fotos · 1:05 min · 512.3 MiB written · throttled to 80 MB/s');
 assert.equal(localizeMessage('512.3 MiB written · throttled to 80 MB/s','de'),'512.3 MiB geschrieben · gedrosselt auf 80 MB/s');
});
