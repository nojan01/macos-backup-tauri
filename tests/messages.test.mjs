import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import ts from 'typescript';
const source=fs.readFileSync(new URL('../src/messages.ts',import.meta.url),'utf8');
const output=ts.transpileModule(source,{compilerOptions:{module:ts.ModuleKind.ESNext,target:ts.ScriptTarget.ES2020}}).outputText;
const {localizeMessage,messagePairs}=await import(`data:text/javascript;base64,${Buffer.from(output).toString('base64')}`);
test('all message templates translate in both directions without altering parameters',()=>{
 for(const [de,en] of messagePairs){
  const fill=s=>s.replace(/\{(\d+)\}/g,(_,n)=>`/Pfad ${n}/Prüfen · Datei $&.png`);
  assert.equal(localizeMessage(fill(de),'en'),fill(en),de);
  assert.equal(localizeMessage(fill(en),'de'),fill(de),en);
 }
});
test('screenshot heartbeats translate phase, counters and waiting text',()=>{
 assert.equal(localizeMessage('Archiv erstellen und komprimieren: Parallels · 2:12 min · warte auf Abschluss dieses Arbeitsschritts','en'),'Creating and compressing archive: Parallels · 2:12 min · waiting for this step to finish');
 const de='Quelländerungen prüfen: Parallels · 2:52 min · 49 Einträge · 263072.7 MiB gelesen · 1524.3 MiB/s · Prüfen · Datei.png';
 const en='Checking source changes: Parallels · 2:52 min · 49 entries · 263072.7 MiB read · 1524.3 MiB/s · Prüfen · Datei.png';
 assert.equal(localizeMessage(de,'en'),en);assert.equal(localizeMessage(en,'de'),de);
});
test('unrecognized paths and external diagnostics remain unchanged',()=>{
 for(const input of ['/Users/example/Quelländerungen prüfen/foo','Device not configured (os error 6)','README.md','Prüfen · Test']) assert.equal(localizeMessage(input,'en'),input);
});
