import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import ts from 'typescript';
const source=fs.readFileSync(new URL('../src/progress-ui.ts',import.meta.url),'utf8');
const output=ts.transpileModule(source,{compilerOptions:{module:ts.ModuleKind.ESNext,target:ts.ScriptTarget.ES2020}}).outputText;
const {ProgressIndicator}=await import(`data:text/javascript;base64,${Buffer.from(output).toString('base64')}`);
function fixture(){
 const classes=new Set();const attrs=new Map();
 const fill={style:{},classList:{toggle(name,on){if(on)classes.add(name);else classes.delete(name);}}};
 const bar={setAttribute(k,v){attrs.set(k,v);},removeAttribute(k){attrs.delete(k);}};
 const label={textContent:''};const progress=new ProgressIndicator(fill,bar,label,()=>({unknown:'Working, total unknown',overall:'Overall'}));
 return {progress,fill,label,classes,attrs};
}
test('resume preflight stays visible across zero events without inventing a percentage',()=>{
 const f=fixture();f.progress.setBusy(true);f.progress.reset();f.progress.update(0);
 assert.equal(f.classes.has('indeterminate'),true);assert.notEqual(f.fill.style.width,'0%');
 assert.equal(f.attrs.has('aria-valuenow'),false);assert.equal(f.label.textContent,'Working, total unknown');
});
test('known progress replaces unknown activity and never retreats between backend phases',()=>{
 const f=fixture();f.progress.setBusy(true);f.progress.update(6);f.progress.update(1);f.progress.update(0);
 assert.equal(f.classes.has('indeterminate'),false);assert.equal(f.classes.has('animating'),true);
 assert.equal(f.fill.style.width,'6%');assert.equal(f.attrs.get('aria-valuenow'),'6');
 assert.equal(f.label.textContent,'Overall: 6 %');
});
test('cancellation and failure stop all activity without reporting success',()=>{
 for(const value of [0,23]){
  const f=fixture();f.progress.setBusy(true);f.progress.update(value);f.progress.setBusy(false);
  assert.equal(f.classes.size,0);assert.equal(f.attrs.get('aria-busy'),'false');
  assert.notEqual(f.attrs.get('aria-valuenow'),'100');
 }
});
test('a new operation resets the prior completed state and bounds malformed values',()=>{
 const f=fixture();f.progress.setBusy(true);f.progress.update(100);f.progress.setBusy(false);
 assert.equal(f.fill.style.width,'100%');f.progress.setBusy(true);
 assert.equal(f.attrs.has('aria-valuenow'),false);f.progress.update(NaN);f.progress.update(Infinity);
 assert.equal(f.attrs.has('aria-valuenow'),false);f.progress.update(300);
 assert.equal(f.fill.style.width,'100%');assert.equal(f.classes.size,0);
});
