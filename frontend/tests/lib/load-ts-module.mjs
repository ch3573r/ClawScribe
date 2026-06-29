import { createRequire } from "node:module";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import vm from "node:vm";
import ts from "typescript";

const require = createRequire(import.meta.url);
const testsDir = path.dirname(fileURLToPath(import.meta.url));
const projectRoot = path.resolve(testsDir, "..", "..");

function resolveLocalModule(specifier, parentDir) {
  if (specifier.startsWith("@/")) {
    return path.join(projectRoot, "src", specifier.slice(2));
  }

  if (specifier.startsWith(".")) {
    return path.resolve(parentDir, specifier);
  }

  return null;
}

function resolveWithExtension(modulePath) {
  const candidates = [
    modulePath,
    `${modulePath}.ts`,
    `${modulePath}.tsx`,
    `${modulePath}.js`,
    `${modulePath}.mjs`,
    path.join(modulePath, "index.ts"),
    path.join(modulePath, "index.tsx"),
    path.join(modulePath, "index.js"),
  ];

  const resolved = candidates.find((candidate) => fs.existsSync(candidate));
  if (!resolved) {
    throw new Error(`Unable to resolve test module: ${modulePath}`);
  }
  return resolved;
}

export function loadTsModule(filePath, mocks = {}) {
  const resolvedPath = resolveWithExtension(filePath);
  const source = fs.readFileSync(resolvedPath, "utf8");
  const compiled = ts.transpileModule(source, {
    compilerOptions: {
      esModuleInterop: true,
      jsx: ts.JsxEmit.ReactJSX,
      module: ts.ModuleKind.CommonJS,
      target: ts.ScriptTarget.ES2020,
    },
  }).outputText;

  const module = { exports: {} };
  const parentDir = path.dirname(resolvedPath);
  const localRequire = (specifier) => {
    if (Object.prototype.hasOwnProperty.call(mocks, specifier)) {
      return mocks[specifier];
    }

    const localModule = resolveLocalModule(specifier, parentDir);
    if (localModule) {
      return loadTsModule(localModule, mocks);
    }

    return require(specifier);
  };

  vm.runInNewContext(compiled, {
    console,
    exports: module.exports,
    globalThis,
    module,
    require: localRequire,
    window: globalThis.window,
    Event: globalThis.Event,
    CustomEvent: globalThis.CustomEvent,
  }, {
    filename: resolvedPath,
  });

  return module.exports;
}
