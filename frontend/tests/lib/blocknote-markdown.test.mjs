import assert from "node:assert/strict";
import path from "node:path";
import { afterEach, describe, mock, test } from "node:test";
import { fileURLToPath } from "node:url";
import { loadTsModule } from "./load-ts-module.mjs";

const modulePath = path.join(
  path.dirname(fileURLToPath(import.meta.url)),
  "..",
  "..",
  "src",
  "lib",
  "blocknote-markdown.ts",
);

describe("blocksToMarkdownSafely", () => {
  afterEach(() => {
    mock.restoreAll();
  });

  test("returns markdown when conversion succeeds", async () => {
    const { blocksToMarkdownSafely } = loadTsModule(modulePath);
    const blocksToMarkdownLossy = mock.fn(async () => "# Summary");
    const editor = { blocksToMarkdownLossy };

    const result = await blocksToMarkdownSafely(editor, [], {
      source: "test-success",
    });

    assert.equal(result.markdown, "# Summary");
    assert.equal(result.ok, true);
    assert.equal(blocksToMarkdownLossy.mock.callCount(), 1);
  });

  test("returns fallback markdown when conversion throws", async () => {
    const { blocksToMarkdownSafely } = loadTsModule(modulePath);
    const error = new Error("conversion failed");
    const blocksToMarkdownLossy = mock.fn(async () => {
      throw error;
    });
    const consoleError = mock.method(console, "error", () => {});

    const result = await blocksToMarkdownSafely(
      { blocksToMarkdownLossy },
      [{ id: "block-1" }],
      {
        source: "test-fallback",
        fallbackMarkdown: "existing markdown",
      },
    );

    assert.equal(result.markdown, "existing markdown");
    assert.equal(result.ok, false);
    assert.equal(consoleError.mock.callCount(), 1);
    assert.equal(
      consoleError.mock.calls[0].arguments[0],
      "Failed to convert BlockNote blocks to markdown",
    );
    assert.equal(consoleError.mock.calls[0].arguments[1].source, "test-fallback");
    assert.equal(consoleError.mock.calls[0].arguments[1].blocksCount, 1);
    assert.equal(consoleError.mock.calls[0].arguments[1].error, error);
  });

  test("omits markdown when conversion throws without fallback", async () => {
    const { blocksToMarkdownSafely } = loadTsModule(modulePath);
    const blocksToMarkdownLossy = mock.fn(async () => {
      throw new Error("conversion failed");
    });
    mock.method(console, "error", () => {});

    const result = await blocksToMarkdownSafely(
      { blocksToMarkdownLossy },
      [],
      {
        source: "test-empty-fallback",
      },
    );

    assert.equal(result.markdown, undefined);
    assert.equal(result.ok, false);
  });
});
