import { closeSync, fstatSync, openSync, readdirSync, readFileSync, readSync } from 'node:fs';
import { basename, dirname, join, resolve } from 'node:path';

const required = ['msvcp140.dll', 'msvcp140_1.dll', 'vcruntime140.dll', 'vcruntime140_1.dll', 'vcomp140.dll'];
const vcRuntime = /^(?:msvcp|msvcr|vcruntime|vccorlib|concrt|vcomp|mfcm?|vcamp|atl)\d\w*\.dll$/i;

// Read only PE headers and import tables. Codex includes executables of several
// hundred megabytes, so reading the whole image would inflate acceptance memory.
function readImage(file, target) {
  let fd;
  try {
    fd = openSync(file, 'r');
    const length = fstatSync(fd).size;
    const read = (offset, size) => {
      if (offset < 0 || offset + size > length) throw new Error('Truncated PE data');
      const buffer = Buffer.alloc(size);
      if (readSync(fd, buffer, 0, size, offset) !== size) throw new Error('Truncated PE data');
      return buffer;
    };
    if (length < 2 || read(0, 2).toString('ascii') !== 'MZ') {
      if (/\.(?:exe|dll|pyd|node|ocx)$/i.test(target)) throw new Error('Missing PE header');
      return;
    }

    const pe = read(0x3c, 4).readUInt32LE();
    const coff = read(pe, 24);
    if (coff.readUInt32LE() !== 0x4550) throw new Error('Invalid PE signature');
    const machine = coff.readUInt16LE(4);
    const optionalSize = coff.readUInt16LE(20);
    const optional = read(pe + 24, optionalSize);
    const magic = optional.readUInt16LE();
    if (magic !== 0x10b && magic !== 0x20b) throw new Error('Unsupported PE optional header');
    const directoryStart = magic === 0x20b ? 112 : 96;
    const directoryCount = optional.readUInt32LE(directoryStart - 4);
    const sections = read(pe + 24 + optionalSize, coff.readUInt16LE(6) * 40);
    const imports = new Set();

    const offsetForRva = (rva, size) => {
      for (let section = 0; section < sections.length; section += 40) {
        const delta = rva - sections.readUInt32LE(section + 12);
        if (delta >= 0 && delta + size <= sections.readUInt32LE(section + 16)) {
          return sections.readUInt32LE(section + 20) + delta;
        }
      }
      throw new Error('PE import address is outside the file-backed sections');
    };

    // IMAGE_DIRECTORY_ENTRY_IMPORT and IMAGE_DIRECTORY_ENTRY_DELAY_IMPORT.
    for (const { index, stride, nameOffset } of [
      { index: 1, stride: 20, nameOffset: 12 },
      { index: 13, stride: 32, nameOffset: 4 },
    ]) {
      if (index >= directoryCount) continue;
      const directory = directoryStart + index * 8;
      const rva = optional.readUInt32LE(directory);
      if (rva === 0) continue;
      const size = optional.readUInt32LE(directory + 4);

      for (let entry = 0; ; entry += stride) {
        if (entry + stride > size) throw new Error('Unterminated PE import table');
        const descriptor = read(offsetForRva(rva + entry, stride), stride);
        let nameRva = descriptor.readUInt32LE(nameOffset);
        if (nameRva === 0) break;
        if (index === 13 && (descriptor.readUInt32LE() & 1) === 0) {
          const imageBase = magic === 0x20b ? Number(optional.readBigUInt64LE(24)) : optional.readUInt32LE(28);
          nameRva -= imageBase;
        }
        const nameOffsetInFile = offsetForRva(nameRva, 1);
        const string = read(nameOffsetInFile, Math.min(260, length - nameOffsetInFile));
        const terminator = string.indexOf(0);
        if (terminator < 1) throw new Error('Invalid PE import name');
        const name = string.toString('ascii', 0, terminator);
        if (vcRuntime.test(name)) imports.add(name.toLowerCase());
      }
    }
    return { machine, dll: (coff.readUInt16LE(22) & 0x2000) !== 0, imports };
  } catch (error) {
    throw new Error('Cannot inspect packaged file ' + basename(target) + ': ' + (error.code || error.message));
  } finally {
    if (fd !== undefined) closeSync(fd);
  }
}

try {
  const [root, appFolder, msiFileTable] = process.argv.slice(2);
  if (!root || !appFolder) throw new Error('Expected package root and application folder');
  const packageRoot = resolve(root);
  const applicationFolder = resolve(packageRoot, appFolder);
  // MSI cabinet entries use File table keys as filenames. The table maps those
  // keys to the installed filenames and Component directory IDs.
  const files = msiFileTable
    ? JSON.parse(readFileSync(msiFileTable, 'utf8')).map(({ key, name, directory }) => ({
        source: join(packageRoot, key),
        target: join(packageRoot, directory, name),
      }))
    : readdirSync(packageRoot, { recursive: true, withFileTypes: true })
        .filter(entry => entry.isFile())
        .map(entry => {
          const path = join(entry.parentPath, entry.name);
          return { source: path, target: path };
        });
  const images = new Map();
  for (const { source, target } of files) {
    const image = readImage(source, target);
    if (image) images.set(target.toLowerCase(), image);
  }
  const missing = [];
  const fromFolder = (folder, dll) => images.get(join(folder, dll).toLowerCase());
  for (const [file, image] of images) {
    // A process created from a nested executable has its own application
    // directory. DLLs loaded by ClawScribe resolve against its application root.
    const folder = image.dll ? applicationFolder : dirname(file);
    for (const dll of image.imports) {
      const dependency = fromFolder(folder, dll);
      if (!dependency?.dll || dependency.machine !== image.machine) {
        missing.push(basename(file) + ' requires ' + dll + ' for its architecture beside the executable');
      }
    }
  }
  for (const dll of required) {
    const image = fromFolder(applicationFolder, dll);
    if (!image?.dll || image.machine !== 0x8664) missing.push('Missing x64 ' + dll + ' in the application folder');
  }
  if (missing.length) throw new Error(missing.join('\n'));
  console.log('Verified app-local VC++ runtime imports across ' + images.size + ' packaged PE images.');
} catch (error) {
  console.error(error.message);
  process.exitCode = 1;
}
