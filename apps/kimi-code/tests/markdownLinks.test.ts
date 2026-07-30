import assert from "node:assert/strict";
import test from "node:test";

import { resolveMarkdownExternalUrl } from "../src/markdownLinks.ts";

test("markdown links accept browser and email protocols", () => {
  assert.equal(
    resolveMarkdownExternalUrl("https://example.com/docs"),
    "https://example.com/docs",
  );
  assert.equal(
    resolveMarkdownExternalUrl("http://example.com"),
    "http://example.com/",
  );
  assert.equal(
    resolveMarkdownExternalUrl("mailto:team@example.com"),
    "mailto:team@example.com",
  );
});

test("markdown links normalize common web addresses", () => {
  assert.equal(
    resolveMarkdownExternalUrl("www.example.com/docs"),
    "https://www.example.com/docs",
  );
  assert.equal(
    resolveMarkdownExternalUrl("//example.com/docs"),
    "https://example.com/docs",
  );
});

test("markdown links reject local, relative, and executable protocols", () => {
  assert.equal(resolveMarkdownExternalUrl("javascript:alert(1)"), undefined);
  assert.equal(resolveMarkdownExternalUrl("data:text/plain,hello"), undefined);
  assert.equal(resolveMarkdownExternalUrl("file:///C:/secret.txt"), undefined);
  assert.equal(resolveMarkdownExternalUrl("../relative/path"), undefined);
  assert.equal(resolveMarkdownExternalUrl("  "), undefined);
  assert.equal(resolveMarkdownExternalUrl(undefined), undefined);
});
