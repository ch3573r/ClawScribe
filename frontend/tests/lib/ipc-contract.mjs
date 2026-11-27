import { readdirSync, readFileSync } from 'node:fs';
import path from 'node:path';
import ts from 'typescript';

export function* sourceFiles(directory, extension) {
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    const file = path.join(directory, entry.name);
    if (entry.isDirectory()) yield* sourceFiles(file, extension);
    else if (extension.test(entry.name)) yield file;
  }
}

export function registeredCommands(core) {
  const handler = /tauri::generate_handler!\[([\s\S]+?)\]\)/.exec(core);
  if (!handler) throw new Error('Missing application invoke handler');
  return new Set(handler[1].replace(/\/\/[^\n]*/g, '').split(',')
    .flatMap(entry => /([a-zA-Z_]\w*)\s*$/.exec(entry)?.[1] ?? []));
}

// Skip Rust comments and literals so tests, documentation and braces in
// strings cannot masquerade as a command signature.
function rustCode(source) {
  return source.replace(
    /\/\*[\s\S]*?\*\/|\/\/[^\n]*|r(#+)"[\s\S]*?"\1|"(?:\\[\s\S]|[^"\\])*"|'(?:\\.|[^'\\])'/g,
    match => match.replace(/[^\n]/g, ' '),
  );
}

const brackets = { '(': ')', '<': '>', '[': ']', '{': '}' };

function splitRust(text, separator) {
  const parts = [];
  const stack = [];
  let start = 0;
  for (let index = 0; index < text.length; index++) {
    const char = text[index];
    if (!stack.length && char === separator) {
      parts.push(text.slice(start, index).trim());
      start = index + 1;
    } else if (Object.hasOwn(brackets, char)) {
      stack.push(brackets[char]);
    } else if (stack.at(-1) === char && !(char === '>' && text[index - 1] === '-')) {
      stack.pop();
    }
  }
  if (text.slice(start).trim()) parts.push(text.slice(start).trim());
  return parts;
}

function closingParen(text, start) {
  let depth = 1;
  for (let index = start + 1; index < text.length; index++) {
    if (text[index] === '(') depth++;
    if (text[index] === ')' && --depth === 0) return index;
  }
  throw new Error('Unclosed Rust command parameters');
}

function rustType(type) {
  const text = type.trim().replace(/^(?:[\w]+::)+/, '');
  const generic = /^(\w+)\s*<([\s\S]*)>$/.exec(text);
  if (generic?.[1] === 'Result') return rustType(splitRust(generic[2], ',')[0]);
  if (generic?.[1] === 'Option') return { shape: 'option', item: rustType(generic[2]) };
  if (text === '()') return { shape: 'unit' };
  if (text === 'bool') return { shape: 'boolean' };
  if (text === 'String' || text === '&str') return { shape: 'string' };
  if (/^[iu](?:8|16|32|64|128|size)$|^f(?:32|64)$/.test(text)) return { shape: 'number' };
  return { shape: 'other' };
}

const injected = new Set(['AppHandle', 'State', 'WebviewWindow', 'Window', 'Webview', 'Request']);

export function rustCommands(source) {
  const code = rustCode(source);
  const pattern = /#\[(?:tauri::)?command(?:\([^\]]*\))?\]\s*(?:(?!#\[(?:tauri::)?command\b)#\[[^\]]*\]\s*)*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?fn\s+(\w+)[^(]*\(/g;
  const commands = [];
  for (const match of code.matchAll(pattern)) {
    const open = match.index + match[0].length - 1;
    const close = closingParen(code, open);
    const typed = splitRust(code.slice(open + 1, close), ',');
    const attribute = source.slice(match.index, source.indexOf(']', match.index));
    const snakeCase = /rename_all\s*=\s*"snake_case"/.test(attribute);
    const args = typed.flatMap(parameter => {
      const argument = /^(?:mut\s+)?(\w+)\s*:\s*([\s\S]+)$/.exec(parameter);
      if (!argument) throw new Error(`${match[1]}: unsupported parameter ${parameter}`);
      const [, name, type] = argument;
      const base = type.replace(/^&\s*/, '').split('<')[0].trim().split('::').at(-1);
      if (injected.has(base)) return [];
      const bare = name.replace(/^_+/, '');
      const key = snakeCase ? bare : bare.replace(/_([a-z\d])/g, (_, letter) => letter.toUpperCase());
      return [{ key, optional: rustType(type).shape === 'option' }];
    });
    const signature = code.slice(close + 1, code.indexOf('{', close));
    const output = /^\s*->\s*([\s\S]*?)(?:\bwhere\b|$)/.exec(signature)?.[1] ?? '()';
    commands.push([match[1], { args, returns: rustType(output) }]);
  }
  return commands;
}

function fromType(checker, expression) {
  const type = checker.getTypeAtLocation(expression);
  const variants = type.isUnion() ? type.types : [type];
  return variants.every(variant => variant.isStringLiteral())
    ? variants.map(variant => variant.value)
    : [];
}

function commandNames(checker, expression, seen = new Set()) {
  if (!expression || seen.has(expression)) return [];
  seen.add(expression);
  if (ts.isStringLiteralLike(expression)) return [expression.text];
  const inferred = fromType(checker, expression);
  if (inferred.length) return inferred;
  if (ts.isConditionalExpression(expression)) {
    const left = commandNames(checker, expression.whenTrue, new Set(seen));
    const right = commandNames(checker, expression.whenFalse, new Set(seen));
    return left.length && right.length ? [...left, ...right] : [];
  }
  if (ts.isAsExpression(expression) || ts.isParenthesizedExpression(expression)) {
    return commandNames(checker, expression.expression, seen);
  }
  if (ts.isElementAccessExpression(expression)) {
    const keys = ts.isStringLiteralLike(expression.argumentExpression)
      ? [expression.argumentExpression.text]
      : fromType(checker, expression.argumentExpression);
    const object = checker.getTypeAtLocation(expression.expression);
    const names = keys.map(key => {
      const property = checker.getPropertyOfType(object, key)?.valueDeclaration;
      return property && ts.isPropertyAssignment(property)
        ? commandNames(checker, property.initializer, new Set(seen))
        : [];
    });
    return names.every(values => values.length) ? names.flat() : [];
  }
  // A fixed object map can have widened string values without "as const".
  const symbol = checker.getSymbolAtLocation(ts.isPropertyAccessExpression(expression) ? expression.name : expression);
  const declaration = symbol?.valueDeclaration;
  if (declaration && (ts.isVariableDeclaration(declaration) || ts.isPropertyAssignment(declaration))) {
    return commandNames(checker, declaration.initializer, seen);
  }
  return [];
}

function argumentVariants(checker, args) {
  if (!args) return [new Set()];
  const type = checker.getTypeAtLocation(args);
  return (type.isUnion() ? type.types : [type]).map(variant => {
    if (variant.flags & (ts.TypeFlags.Null | ts.TypeFlags.Undefined)) return new Set();
    if (variant.flags & (ts.TypeFlags.Any | ts.TypeFlags.Unknown) || checker.getIndexTypeOfType(variant, ts.IndexKind.String)) return null;
    return new Set(checker.getPropertiesOfType(variant).map(property => property.name));
  });
}

function returnMatches(expected, type) {
  const variants = type.isUnion() ? type.types : [type];
  if (variants.some(item => item.flags & (ts.TypeFlags.Any | ts.TypeFlags.Unknown))) return true;
  const isNull = item => Boolean(item.flags & ts.TypeFlags.Null);
  if (expected.shape === 'option') {
    return variants.some(isNull) && variants.some(item => !isNull(item))
      && variants.filter(item => !isNull(item)).every(item => returnMatches(expected.item, item));
  }
  if (expected.shape === 'unit') {
    return variants.every(item => item.flags & (ts.TypeFlags.Void | ts.TypeFlags.Undefined | ts.TypeFlags.Null));
  }
  const flags = {
    boolean: ts.TypeFlags.BooleanLike,
    number: ts.TypeFlags.NumberLike,
    string: ts.TypeFlags.StringLike,
  }[expected.shape];
  if (flags) return variants.every(item => item.flags & flags);
  return variants.every(item => !(item.flags & (ts.TypeFlags.Void | ts.TypeFlags.Undefined | ts.TypeFlags.Null)));
}

export function inspectInvokes(program, sources, signatures) {
  const checker = program.getTypeChecker();
  const problems = [];
  let checked = 0;
  let computed = 0;
  for (const tree of sources) {
    const imports = new Set();
    const namespaces = new Set();
    for (const statement of tree.statements) {
      if (!ts.isImportDeclaration(statement) || statement.moduleSpecifier.text !== '@tauri-apps/api/core') continue;
      const names = statement.importClause?.namedBindings;
      if (!names) continue;
      if (ts.isNamespaceImport(names)) {
        namespaces.add(checker.getSymbolAtLocation(names.name));
        continue;
      }
      for (const binding of names.elements) {
        if ((binding.propertyName ?? binding.name).text === 'invoke') imports.add(checker.getSymbolAtLocation(binding.name));
      }
    }
    function isInvoke(expression) {
      return ts.isIdentifier(expression) && imports.has(checker.getSymbolAtLocation(expression))
        || ts.isPropertyAccessExpression(expression) && expression.name.text === 'invoke'
          && namespaces.has(checker.getSymbolAtLocation(expression.expression));
    }
    function visit(node) {
      if (ts.isCallExpression(node) && isInvoke(node.expression)) {
        const [command, args] = node.arguments;
        const line = tree.getLineAndCharacterOfPosition(node.getStart(tree)).line + 1;
        const location = `${tree.fileName}:${line}`;
        const names = [...new Set(commandNames(checker, command))];
        if (!names.length) problems.push(`${location} unresolved command ${command?.getText()}`);
        const variants = argumentVariants(checker, args);
        const declared = node.typeArguments?.[0];
        const contracts = names.filter(name => !name.startsWith('plugin:')).flatMap(name => {
          const signature = signatures.get(name);
          if (!signature) {
            problems.push(`${location} unregistered command ${name}`);
            return [];
          }
          checked++;
          if (!ts.isStringLiteralLike(command)) computed++;
          if (declared && !returnMatches(signature.returns, checker.getTypeFromTypeNode(declared))) {
            problems.push(`${location} ${name} returns ${JSON.stringify(signature.returns)}, invoked as ${declared.getText()}`);
          }
          return [{ name, args: signature.args }];
        });
        if (contracts.length) {
          const matches = (variant, contract) => variant
            && contract.args.every(({ key, optional }) => optional || variant.has(key))
            && [...variant].every(key => contract.args.some(arg => arg.key === key));
          for (const contract of contracts) {
            if (!variants.some(variant => matches(variant, contract))) {
              const expected = contract.args.map(arg => arg.key + (arg.optional ? '?' : '')).join(', ');
              const actual = variants.map(variant => variant ? [...variant].join(', ') : '<dynamic keys>').join(' | ');
              problems.push(`${location} ${contract.name} argument keys expected [${expected}], got [${actual}]`);
            }
          }
          for (const variant of variants) {
            if (variants.length > 1 && !contracts.some(contract => matches(variant, contract))) {
              problems.push(`${location} unmatched argument variant [${variant ? [...variant].join(', ') : '<dynamic keys>'}]`);
            }
          }
        }
      }
      ts.forEachChild(node, visit);
    }
    if (imports.size || namespaces.size) visit(tree);
  }
  return { problems, checked, computed };
}

export function projectContracts(frontend) {
  const rust = path.join(frontend, 'src-tauri', 'src');
  const registered = registeredCommands(readFileSync(path.join(rust, 'lib.rs'), 'utf8'));
  const signatures = new Map();
  for (const file of sourceFiles(rust, /\.rs$/)) {
    for (const [name, signature] of rustCommands(readFileSync(file, 'utf8'))) {
      if (!registered.has(name)) continue;
      if (signatures.has(name) && JSON.stringify(signature) !== JSON.stringify(signatures.get(name))) {
        throw new Error(`Multiple signatures for registered command ${name}`);
      }
      signatures.set(name, signature);
    }
  }
  for (const name of registered) {
    if (!signatures.has(name)) throw new Error(`Registered command without a parsed signature: ${name}`);
  }
  const configFile = path.join(frontend, 'tsconfig.json');
  const config = ts.readConfigFile(configFile, ts.sys.readFile);
  const options = ts.convertCompilerOptionsFromJson(config.config.compilerOptions, frontend).options;
  const files = [...sourceFiles(path.join(frontend, 'src'), /\.tsx?$/)];
  const program = ts.createProgram(files, options);
  const result = inspectInvokes(program, files.map(file => program.getSourceFile(file)), signatures);
  return {
    ...result,
    problems: result.problems.map(problem => problem.replaceAll(frontend.replaceAll('\\', '/') + '/', '')),
  };
}
