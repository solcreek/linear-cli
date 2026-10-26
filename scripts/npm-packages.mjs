#!/usr/bin/env node
// Assembles the npm packages for one release from goreleaser's binaries:
// one package per platform holding the binary, and @solcreek/linear-cli,
// which depends on them optionally (npm installs only the one matching the
// machine, by "os" and "cpu").
//
//   node scripts/npm-packages.mjs VERSION BINARIES OUT
//
// BINARIES holds goreleaser's build directories (dist/), whose names start
// with linearctl_<goos>_<goarch>. OUT gets one directory per package, ready
// for npm publish; it must not exist.
import fs from "node:fs"
import path from "node:path"

const [version, binaries, out] = process.argv.slice(2)
if (!version || !binaries || !out || !/^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$/.test(version)) {
  console.error("usage: node scripts/npm-packages.mjs VERSION BINARIES OUT")
  process.exit(2)
}
if (fs.existsSync(out)) {
  console.error(`${out} exists; pass a new directory`)
  process.exit(1)
}

const root = path.resolve(import.meta.dirname, "..")
const meta = {
  license: "MIT",
  author: "solcreek",
  repository: { type: "git", url: "git+https://github.com/solcreek/linear-cli.git" },
  homepage: "https://github.com/solcreek/linear-cli#readme",
  bugs: "https://github.com/solcreek/linear-cli/issues",
}
// goreleaser's GOOS_GOARCH to npm's platform and arch.
const platforms = [
  ["darwin_arm64", "darwin", "arm64"],
  ["darwin_amd64", "darwin", "x64"],
  ["linux_arm64", "linux", "arm64"],
  ["linux_amd64", "linux", "x64"],
  ["windows_amd64", "win32", "x64"],
]

const write = (dir, file, data, mode) => {
  fs.mkdirSync(path.dirname(path.join(dir, file)), { recursive: true })
  fs.writeFileSync(path.join(dir, file), data, mode ? { mode } : undefined)
}
const json = (v) => JSON.stringify(v, null, 2) + "\n"

const optional = {}
for (const [goos, os, cpu] of platforms) {
  const build = fs.readdirSync(binaries).find((d) => d.startsWith(`linearctl_${goos}`))
  if (!build) throw new Error(`no build for ${goos} in ${binaries}`)
  const exe = os === "win32" ? "linearctl.exe" : "linearctl"
  const name = `@solcreek/linear-cli-${os}-${cpu}`
  const dir = path.join(out, `linear-cli-${os}-${cpu}`)
  write(dir, `bin/${exe}`, fs.readFileSync(path.join(binaries, build, exe)), 0o755)
  write(dir, "LICENSE", fs.readFileSync(path.join(root, "LICENSE")))
  write(dir, "README.md", `# ${name}\n\nThe \`linearctl\` binary for ${os}-${cpu}. Install [@solcreek/linear-cli](https://www.npmjs.com/package/@solcreek/linear-cli) instead.\n`)
  write(dir, "package.json", json({
    name, version, description: `linearctl binary for ${os}-${cpu}`, ...meta,
    os: [os], cpu: [cpu], files: ["bin", "LICENSE", "README.md"],
  }))
  optional[name] = version
}

const main = path.join(out, "linear-cli")
fs.cpSync(path.join(root, "npm", "linear-cli"), main, { recursive: true })
write(main, "LICENSE", fs.readFileSync(path.join(root, "LICENSE")))
write(main, "README.md", fs.readFileSync(path.join(root, "README.md")))
write(main, "package.json", json({
  name: "@solcreek/linear-cli", version,
  description: "Agent-first CLI for Linear: --json on every command, --dry-run/--yes on every change, retry-safe creates.",
  ...meta,
  keywords: ["linear", "linear-app", "cli", "agent", "ai-agents", "issue-tracker", "graphql"],
  bin: { linearctl: "bin/linearctl" },
  scripts: { postinstall: "node install.js" },
  files: ["bin", "install.js", "LICENSE", "README.md"],
  engines: { node: ">=18" },
  optionalDependencies: optional,
}))
console.log(JSON.stringify({ version, out, packages: ["@solcreek/linear-cli", ...Object.keys(optional)] }))
