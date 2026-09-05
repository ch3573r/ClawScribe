import assert from "node:assert/strict";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { loadTsModule } from "./load-ts-module.mjs";

const modulePath = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../src/lib/transcriptDisplay.ts");
const { formatTranscriptDisplayText } = loadTsModule(modulePath);

test("preserves meaningful words that resemble English fillers", () => {
  for (const text of ["Er kommt um 10 Uhr.", "Es geht um die Freigabe.", "Um documento foi aprovado."]) {
    assert.equal(formatTranscriptDisplayText(text), text);
  }
});

test("does not silently edit speech, negations, numbers, or internal whitespace", () => {
  for (const text of ["Oh, no. Do NOT approve 15,000 EUR.", "Um, we have not agreed.", "Ja. Nein. 42.", "名前\t😀\nZweite Zeile"]) {
    assert.equal(formatTranscriptDisplayText(text), text);
  }
});

test("only trims outer whitespace and labels genuinely empty text", () => {
  assert.equal(formatTranscriptDisplayText("  Er kommt. \n"), "Er kommt.");
  assert.equal(formatTranscriptDisplayText(" \r\n\t"), "[Silence]");
  assert.equal(formatTranscriptDisplayText(""), "[Silence]");
});
