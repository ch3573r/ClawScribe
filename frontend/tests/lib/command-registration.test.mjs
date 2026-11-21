import assert from 'node:assert/strict';
import { readdirSync, readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import ts from 'typescript';

const frontend = fileURLToPath(new URL('../../', import.meta.url));

function* sourceFiles(directory) {
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    const file = path.join(directory, entry.name);
    if (entry.isDirectory()) yield* sourceFiles(file);
    else if (/\.tsx?$/.test(entry.name)) yield file;
  }
}

test('literal app commands invoked by the frontend are registered in Tauri', () => {
  const core = readFileSync(path.join(frontend, 'src-tauri/src/lib.rs'), 'utf8');
  const handler = /tauri::generate_handler!\[([\s\S]+?)\]\)/.exec(core);
  assert.ok(handler, 'Missing application invoke handler');
  const registered = new Set(handler[1].replace(/\/\/[^\n]*/g, '').split(',')
    .map(entry => /([a-zA-Z_]\w*)\s*$/.exec(entry)?.[1]));
  const missing = [];
  for (const file of sourceFiles(path.join(frontend, 'src'))) {
    const source = readFileSync(file, 'utf8');
    if (!source.includes('@tauri-apps/api/core')) continue;
    const tree = ts.createSourceFile(file, source, ts.ScriptTarget.Latest, true);
    const aliases = new Set();
    for (const statement of tree.statements) {
      if (!ts.isImportDeclaration(statement) || statement.moduleSpecifier.text !== '@tauri-apps/api/core') continue;
      const imports = statement.importClause?.namedBindings;
      if (!imports || !ts.isNamedImports(imports)) continue;
      for (const binding of imports.elements) {
        if ((binding.propertyName ?? binding.name).text === 'invoke') aliases.add(binding.name.text);
      }
    }
    const visit = node => {
      if (ts.isCallExpression(node) && ts.isIdentifier(node.expression) && aliases.has(node.expression.text)) {
        const command = node.arguments[0];
        if (command && ts.isStringLiteralLike(command) && !command.text.startsWith('plugin:') && !registered.has(command.text)) {
          missing.push(path.relative(frontend, file) + ': ' + command.text);
        }
      }
      ts.forEachChild(node, visit);
    };
    visit(tree);
  }
  assert.deepEqual(missing, []);
});
