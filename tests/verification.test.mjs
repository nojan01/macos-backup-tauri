import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import ts from 'typescript';
const source = readFileSync(new URL('../src/verification-ui.ts', import.meta.url), 'utf8');
const {outputText} = ts.transpileModule(source, {compilerOptions:{module:ts.ModuleKind.ESNext}});
const {verificationStatusKey} = await import('data:text/javascript;base64,' + Buffer.from(outputText).toString('base64'));

test('reloaded backup uses persisted verification and invalid metadata takes precedence', () => {
  assert.equal(verificationStatusKey({metadata_valid:true,hash_verified:true}), 'backupVerified');
  assert.equal(verificationStatusKey({metadata_valid:true,hash_verified:false}), 'backupNotVerified');
  assert.equal(verificationStatusKey({metadata_valid:false,hash_verified:true}), 'backupInvalid');
});
