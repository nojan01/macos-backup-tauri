import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import ts from 'typescript';

const source = fs.readFileSync(new URL('../src/restore-ui.ts', import.meta.url), 'utf8');
const { outputText } = ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2020 } });
const { createRestoreRow, restoreStatusKey } = await import(`data:text/javascript;base64,${Buffer.from(outputText).toString('base64')}`);

test('backup paths remain literal checkbox values and text, never HTML', () => {
  // A DOM sink spy throws if rendering attempts to parse HTML. The browser's
  // value/textContent setters are the only permitted sinks for backup strings.
  const doc = { createElement(tag) { return { tag, children: [], append(...nodes) { this.children.push(...nodes); }, set innerHTML(_) { throw new Error('Unsafe HTML sink'); } }; } };
  const path = '~/Documents/" ><img src=x onerror=alert(1)> & Grüße';
  const row = createRestoreRow(path, '📁', '1 KB', doc);
  assert.equal(row.children[0].value, path);
  assert.equal(row.children[0].checked, true);
  assert.equal(row.children[2].children[0].textContent, path);
});

test('partial and total failures never select the success message', () => {
  assert.equal(restoreStatusKey({ error_count: 1, restored_count: 3 }), 'restoreWithErrors');
  assert.equal(restoreStatusKey({ error_count: 2, restored_count: 0 }), 'restoreWithErrors');
  assert.equal(restoreStatusKey({ error_count: 0, restored_count: 0 }), 'restoreNothingChanged');
  assert.equal(restoreStatusKey({ error_count: 0, restored_count: 2 }), 'restoreComplete');
});


test('version labels use the installed app version instead of a hardcoded release', () => {
  const html=fs.readFileSync(new URL('../index.html',import.meta.url),'utf8');
  const main=fs.readFileSync(new URL('../src/main.ts',import.meta.url),'utf8');
  assert.equal((html.match(/data-app-version/g)||[]).length,2);
  assert.doesNotMatch(html,/v\d+\.\d+\.\d+/);
  assert.match(main,/await getVersion\(\)/);
});

const cancelSource=fs.readFileSync(new URL('../src/cancel-ui.ts',import.meta.url),'utf8');
const cancelOutput=ts.transpileModule(cancelSource,{compilerOptions:{module:ts.ModuleKind.ESNext,target:ts.ScriptTarget.ES2020}}).outputText;
const {renderCancelControl}=await import(`data:text/javascript;base64,${Buffer.from(cancelOutput).toString('base64')}`);
test('cancel remains visible for checks and can be retried after a failed request', () => {
  const button={disabled:false,textContent:'',style:{display:'none'}};
  const labels={cancel:'Abbrechen',cancelling:'Abbruch läuft …'};
  renderCancelControl(button,true,false,labels);assert.equal(button.style.display,'block');assert.equal(button.disabled,false);
  renderCancelControl(button,true,true,labels);assert.equal(button.style.display,'block');assert.equal(button.disabled,true);assert.equal(button.textContent,labels.cancelling);
  renderCancelControl(button,true,false,labels);assert.equal(button.disabled,false);
  renderCancelControl(button,false,false,labels);assert.equal(button.style.display,'none');assert.equal(button.disabled,true);
});
