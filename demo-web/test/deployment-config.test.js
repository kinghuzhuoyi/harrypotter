import assert from "node:assert/strict";
import { access, readFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..", "..");

test("Cloudflare Workers config points to a complete static site", async () => {
  const config = JSON.parse(await readFile(join(ROOT, "wrangler.jsonc"), "utf8"));
  assert.equal(config.name, "harrypotter");
  assert.equal(config.assets.directory, "./demo-web/public");

  const output = join(ROOT, config.assets.directory);
  await Promise.all([
    access(join(output, "index.html")),
    access(join(output, "styles.css")),
    access(join(output, "app.js"))
  ]);
});
