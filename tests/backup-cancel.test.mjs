import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';
import ts from 'typescript';

// Exercise the actual orchestration functions without loading the native Tauri
// window. Only IPC and DOM endpoints are replaced; no backup files are touched.
const source = fs.readFileSync(new URL('../src/main.ts', import.meta.url), 'utf8');
const ast = ts.createSourceFile('main.ts', source, ts.ScriptTarget.Latest, true);
const names = ['startBackup', 'cancelOperation', 'setOperationControls', 'setProfileControlsDisabled', 'setupEventListeners'];
const functions = names.map(name => {
  const node = ast.statements.find(n => ts.isFunctionDeclaration(n) && n.name?.text === name);
  assert.ok(node, name);
  return node.getText(ast);
}).join('\n');
const cancel = fs.readFileSync(new URL('../src/cancel-ui.ts', import.meta.url), 'utf8').replace('export function', 'function');
const code = ts.transpileModule(cancel + '\n' + functions, {compilerOptions: {target: ts.ScriptTarget.ES2020}}).outputText;

function fixture(language) {
  let resolve, reject, started;
  const ready = new Promise(r => { started = r; });
  const backend = new Promise((yes, no) => { resolve = yes; reject = no; });
  const labels = language === 'de'
    ? {cancelling: 'Abbruch läuft …', backupCancelled: 'Backup abgebrochen!'}
    : {cancelling: 'Cancelling…', backupCancelled: 'Backup cancelled!'};
  const events = new Map();
  const state = {
    operationInProgress: false, backupInProgress: false, cancelRequested: false,
    hasFDA: true, config: {directories: ['~/Example']}, rawStatus: '', progress: '',
    checkFullDiskAccess: async () => {}, getFullTargetPath: () => '/mock/backup',
    t: key => labels[key] ?? key, log: () => {}, loadBackups: async () => {},
    sendNotification: async () => {},
    progressIndicator: {value: 0, reset() {this.value = 0;}, setBusy() {}, update(n) {this.value = n;}},
    listen: async (event, callback) => {events.set(event, callback);},
    invoke: async command => {
      if (command === 'list_resumable_backups') return [];
      if (command === 'create_backup') {started(); return backend;}
    },
  };
  for (const name of ['btnCancel', 'btnBackup', 'btnRestore', 'btnRestoreTest', 'btnTestRestore', 'btnDeleteBackup', 'restoreStart', 'restoreQuickBtn', 'testRestoreStart', 'backupSelect', 'volumeSelect', 'browseTargetBtn', 'profileSelect', 'profileNewBtn', 'profileDuplicateBtn', 'profileRenameBtn', 'profileDeleteBtn']) {
    state[name] = {disabled: false, style: {}, textContent: ''};
  }
  state.profileSelect.options = {length: 1};
  state.setStatusMessage = message => {state.rawStatus = message;};
  state.setProgressMessage = message => {state.progress = message;};
  vm.createContext(state);
  vm.runInContext(code, state);
  return {state, labels, events, ready, resolve, reject};
}

for (const language of ['de', 'en']) {
  for (const result of ['resolve', 'reject']) {
    test(`completed ${language} cancellation clears pending text when backend ${result}s`, async () => {
      const f = fixture(language);
      await f.state.setupEventListeners();
      const running = f.state.startBackup();
      await f.ready;
      f.events.get('backup-progress')({payload: {progress: 50, message: 'Working'}});
      await f.state.cancelOperation();
      assert.equal(f.state.progress, f.labels.cancelling);
      assert.equal(f.state.operationInProgress, true);
      assert.equal(f.state.btnBackup.disabled, true);
      assert.equal(f.state.btnCancel.style.display, 'block');
      if (result === 'resolve') f.resolve(); else f.reject(new Error('cancelled'));
      await running;
      assert.equal(f.state.rawStatus, f.labels.backupCancelled);
      assert.equal(f.state.progress, f.labels.backupCancelled);
      assert.equal(f.state.operationInProgress, false);
      assert.equal(f.state.btnBackup.disabled, false);
      assert.equal(f.state.btnCancel.style.display, 'none');
      f.events.get('backup-progress')({payload: {progress: 99, message: 'Late queued event'}});
      assert.equal(f.state.progress, f.labels.backupCancelled);
      assert.equal(f.state.progressIndicator.value, 50);
    });
  }
}
