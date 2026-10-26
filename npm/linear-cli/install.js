// Replaces the bin/linearctl shim with the native binary, as esbuild does, so
// running linearctl does not start Node first (~40 ms a run). Skipped on
// Windows, where npm's .cmd wrapper needs the JS file. Any failure keeps the
// shim, which still works.
"use strict"
const fs = require("node:fs")
const path = require("node:path")

if (process.platform !== "win32") {
  try {
    const native = require.resolve(`@solcreek/linear-cli-${process.platform}-${process.arch}/bin/linearctl`)
    const shim = path.join(__dirname, "bin", "linearctl")
    const temp = `${shim}.${process.pid}.tmp`
    fs.copyFileSync(native, temp)
    fs.chmodSync(temp, 0o755)
    fs.renameSync(temp, shim)
  } catch {
    // No platform package, or a read-only tree: the shim stays.
  }
}
