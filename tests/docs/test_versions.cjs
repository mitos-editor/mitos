const { test } = require("node:test");
const assert = require("node:assert/strict");
const { readFileSync } = require("node:fs");
const { join } = require("node:path");
const { runInNewContext } = require("node:vm");

const script = readFileSync(join(__dirname, "../../book/doc-versions.js"), "utf8");

async function switchVersion(url, pathToRoot, pages, available = true) {
  let handler;
  let destination;
  const select = { value: "main", addEventListener: (_, fn) => { handler = fn; } };
  const location = new URL(url);
  location.assign = (target) => { destination = target; };
  runInNewContext(script, {
    document: { getElementById: () => select },
    location,
    URL,
    path_to_root: pathToRoot,
    fetch: async () => ({ ok: available, json: async () => ({ "v0.1.0": pages }) }),
  });
  select.value = "v0.1.0";
  await handler();
  assert.equal(select.disabled, false);
  return destination;
}

test("switches chapters and preserves the query and anchor for mdBook and clean URLs", async () => {
  for (const page of ["configuration.html", "configuration"]) {
    assert.equal(await switchVersion(`https://mitos.computer/docs/main/${page}?search=test#editor`, "", ["configuration.html"]),
      "/docs/v0.1.0/configuration.html?search=test#editor");
  }
  assert.equal(await switchVersion("https://mitos.computer/docs/main/guides/example.html", "../", ["guides/example.html"]),
    "/docs/v0.1.0/guides/example.html");
  assert.equal(await switchVersion("https://mitos.computer/docs/", "", ["index.html"]), "/docs/v0.1.0/index.html");
});

test("falls back to the version introduction for missing chapters and manifest errors", async () => {
  assert.equal(await switchVersion("https://mitos.computer/docs/main/new-feature.html", "", ["index.html"]), "/docs/v0.1.0/");
  assert.equal(await switchVersion("https://mitos.computer/docs/main/configuration.html", "", [], false), "/docs/v0.1.0/");
});
