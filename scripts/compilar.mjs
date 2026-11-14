import { spawnSync, spawn } from "node:child_process";
import { existsSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const isWindows = process.platform === "win32";

function run(command, args, label, options = {}) {
  console.log("\\n▶ " + label);
  const result = spawnSync(command, args, {
    cwd: root,
    stdio: "inherit",
    shell: isWindows && command === "where",
    ...options,
  });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    throw new Error(label + " falhou (código " + result.status + ").");
  }
}

function findCargo() {
  const candidates = isWindows
    ? [
        process.env.CARGO_EXE,
        process.env.USERPROFILE && resolve(process.env.USERPROFILE, ".cargo", "bin", "cargo.exe"),
      ]
    : [process.env.CARGO_EXE, process.env.HOME && resolve(process.env.HOME, ".cargo", "bin", "cargo")];
  for (const candidate of candidates) {
    if (candidate && existsSync(candidate)) return candidate;
  }
  return "cargo";
}

function closePreviousGui() {
  if (!isWindows) return;
  const command = "$running = Get-Process -Name 'srm-gui' -ErrorAction SilentlyContinue; if ($running) { $running | ForEach-Object { [void]$_.CloseMainWindow() }; Start-Sleep -Seconds 3; Get-Process -Name 'srm-gui' -ErrorAction SilentlyContinue | Stop-Process -Force }";
  run("powershell", ["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", command], "Encerrando a versão anterior da interface");
}

async function main() {
  console.log("Shared Runtime Manager · atualização e compilação");
  run("git", ["pull", "--ff-only", "origin", "main"], "Atualizando o código pela branch main");

  const cargo = findCargo();
  run(cargo, ["fmt", "--all"], "Formatando o código Rust");
  run(cargo, ["test", "--all-targets"], "Executando os testes");
  closePreviousGui();
  run(cargo, ["build", "--release", "--features", "desktop", "--bin", "srm-gui"], "Compilando o aplicativo desktop e a GUI");

  const executable = isWindows
    ? resolve(root, "target", "release", "srm-gui.exe")
    : resolve(root, "target", "release", "srm-gui");
  if (!existsSync(executable)) {
    throw new Error("A compilação terminou, mas o executável da interface não foi encontrado: " + executable);
  }

  console.log("\\n✓ Atualização e compilação concluídas. Iniciando a interface gráfica...");
  const child = spawn(executable, [], { cwd: root, detached: true, stdio: "ignore" });
  child.unref();
}

main().catch(error => {
  console.error("\\n✖ " + (error?.message || error));
  process.exitCode = 1;
});
