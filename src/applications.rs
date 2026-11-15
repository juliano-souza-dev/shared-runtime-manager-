use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Debug, Clone, Serialize)]
pub struct CatalogApplication {
    pub id: String,
    pub name: String,
    pub repository: String,
    pub branch: String,
    pub description: String,
    pub installed: bool,
    pub mode: Option<String>,
    pub version: Option<String>,
    pub source_path: Option<String>,
    pub launch_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct InstalledApplication {
    id: String,
    name: String,
    repository: String,
    branch: String,
    mode: String,
    version: String,
    source_path: String,
    launch_path: Option<String>,
    installed_at: u64,
    shared_app_id: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct ApplicationRegistry {
    schema_version: u32,
    applications: BTreeMap<String, InstalledApplication>,
}

struct Definition {
    id: &'static str,
    name: &'static str,
    repository: &'static str,
    branch: &'static str,
    description: &'static str,
    shared_app_id: &'static str,
    cargo_package: &'static str,
    frontend_dir: &'static str,
    install_command: &'static str,
    build_command: &'static str,
    dev_command: &'static str,
}

const DEFINITIONS: &[Definition] = &[
    Definition {
        id: "nova-generator",
        name: "Nova Generator",
        repository: "https://github.com/juliano-souza-dev/nova-generator.git",
        branch: "main",
        description: "Estúdio de criação de materiais educacionais de inglês.",
        shared_app_id: "nova-generator",
        cargo_package: "nova-generator",
        frontend_dir: ".",
        install_command: "npm install",
        build_command: "npm run tauri --workspace @nova-generator/desktop -- build",
        dev_command: "npm run tauri --workspace @nova-generator/desktop -- dev",
    },
    Definition {
        id: "ttk-creator",
        name: "TTK Creator",
        repository: "https://github.com/juliano-souza-dev/ttk-creator.git",
        branch: "main",
        description: "Editor desktop de vídeos curtos e materiais para redes sociais.",
        shared_app_id: "ttk-media",
        cargo_package: "tiktok-media-creator",
        frontend_dir: "apps/desktop",
        install_command: "npm install",
        build_command: "npm run tauri build",
        dev_command: "npm run tauri dev",
    },
];

fn timestamp() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

fn registry_path(root: &Path) -> PathBuf {
    root.join("registry/applications-catalog.json")
}

fn read_registry(root: &Path) -> Result<ApplicationRegistry> {
    let path = registry_path(root);
    if !path.is_file() {
        return Ok(ApplicationRegistry { schema_version: 1, ..Default::default() });
    }
    let registry: ApplicationRegistry = serde_json::from_slice(&fs::read(&path)?)
        .context("O catálogo local de aplicativos está inválido.")?;
    if registry.schema_version != 1 {
        bail!("Versão não suportada do catálogo de aplicativos: {}", registry.schema_version);
    }
    Ok(registry)
}

fn write_registry(root: &Path, registry: &ApplicationRegistry) -> Result<()> {
    let path = registry_path(root);
    fs::create_dir_all(path.parent().expect("registry path has parent"))?;
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, serde_json::to_vec_pretty(registry)?)?;
    if path.exists() { fs::remove_file(&path)?; }
    fs::rename(temporary, path)?;
    Ok(())
}

fn definition(id: &str) -> Result<&'static Definition> {
    DEFINITIONS.iter().find(|item| item.id == id)
        .ok_or_else(|| anyhow!("Aplicativo '{id}' não existe no catálogo do ecossistema."))
}

pub fn catalog() -> Result<Vec<CatalogApplication>> {
    let root = super::manager::root()?;
    let registry = read_registry(&root)?;
    Ok(DEFINITIONS.iter().map(|item| {
        let installed = registry.applications.get(item.id);
        CatalogApplication {
            id: item.id.to_owned(),
            name: item.name.to_owned(),
            repository: item.repository.trim_end_matches(".git").to_owned(),
            branch: item.branch.to_owned(),
            description: item.description.to_owned(),
            installed: installed.is_some(),
            mode: installed.map(|app| app.mode.clone()),
            version: installed.map(|app| app.version.clone()),
            source_path: installed.map(|app| app.source_path.clone()),
            launch_path: installed.and_then(|app| app.launch_path.clone()),
        }
    }).collect())
}

fn write_status(status: &str, current: &str, app_id: &str, progress: u8, result: Option<Value>) -> Result<()> {
    let root = super::manager::root()?;
    let registry = root.join("registry");
    fs::create_dir_all(&registry)?;
    let path = registry.join("dependency-queue-status.json");
    let temporary = registry.join("dependency-queue-status.json.tmp");
    let mut value = json!({
        "status": status,
        "progress": progress,
        "progressMode": "determinate",
        "current": current,
        "taskType": "application-install",
        "application": app_id,
        "completed": progress,
        "total": 100,
        "results": []
    });
    if let Some(result) = result {
        value["results"] = json!([result]);
    }
    fs::write(&temporary, serde_json::to_vec_pretty(&value)?)?;
    if path.exists() { fs::remove_file(&path)?; }
    fs::rename(temporary, path)?;
    Ok(())
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn command_output<F>(program: &str, args: &[&str], cwd: &Path, stage: &str, on_output: &mut F) -> Result<()>
where F: FnMut(&str, &str) {
    let bash = super::shell::bash_path()
        .with_context(|| format!("{stage} exige Bash. Instale Git Bash ou configure SRM_BASH_PATH."))?;
    let command = std::iter::once(program)
        .chain(args.iter().copied())
        .map(shell_quote)
        .collect::<Vec<_>>()
        .join(" ");
    let display_command = format!("{program} {}", args.join(" "));
    let mut child = Command::new(bash)
        .arg("--noprofile").arg("--norc").arg("-lc").arg(command)
        .current_dir(cwd).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().with_context(|| format!(
            "{stage}: não foi possível iniciar o processo pelo Bash.\nComando: {display_command}\nDiretório: {}",
            cwd.display()
        ))?;
    let (sender, receiver) = mpsc::channel::<(&'static str, String)>();
    if let Some(stdout) = child.stdout.take() {
        let tx = sender.clone();
        thread::spawn(move || for line in BufReader::new(stdout).lines().flatten() { let _ = tx.send(("stdout", line)); });
    }
    if let Some(stderr) = child.stderr.take() {
        let tx = sender.clone();
        thread::spawn(move || for line in BufReader::new(stderr).lines().flatten() { let _ = tx.send(("stderr", line)); });
    }
    drop(sender);
    let (mut captured, mut stdout_line, mut stderr_line) = (String::new(), 0usize, 0usize);
    for (stream, line) in receiver {
        on_output(stream, &line);
        let number = if stream == "stdout" { stdout_line += 1; stdout_line } else { stderr_line += 1; stderr_line };
        captured.push_str(&format!("[{stream}:{number:04}] {line}\n"));
    }
    let status = child.wait().with_context(|| format!(
        "{stage}: não foi possível aguardar o processo.\nComando: {display_command}\nDiretório: {}",
        cwd.display()
    ))?;
    if !status.success() {
        bail!(
            "{stage} falhou.\nComando: {display_command}\nDiretório: {}\nCódigo de saída: {:?}\n\nSaída completa do processo (linhas numeradas):\n{}",
            cwd.display(), status.code(), if captured.trim().is_empty() { "O processo não forneceu saída em stdout ou stderr." } else { captured.trim() }
        );
    }
    Ok(())
}

fn git_command_output<F>(args: &[&str], cwd: &Path, stage: &str, on_output: &mut F) -> Result<()>
where F: FnMut(&str, &str) {
    let display_command = format!("git {}", args.join(" "));
    let mut child = Command::new("git")
        .args(args).current_dir(cwd).stdin(Stdio::null())
        .stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().with_context(|| format!(
            "{stage}: não foi possível iniciar o Git. Comando: {display_command}. Diretório: {}. Verifique se o Git está instalado e disponível no PATH.",
            cwd.display()
        ))?;
    let (sender, receiver) = mpsc::channel::<(&'static str, String)>();
    if let Some(stdout) = child.stdout.take() {
        let tx = sender.clone();
        thread::spawn(move || for line in BufReader::new(stdout).lines().flatten() { let _ = tx.send(("stdout", line)); });
    }
    if let Some(stderr) = child.stderr.take() {
        let tx = sender.clone();
        thread::spawn(move || for line in BufReader::new(stderr).lines().flatten() { let _ = tx.send(("stderr", line)); });
    }
    drop(sender);
    let (mut output_text, mut stdout_line, mut stderr_line) = (String::new(), 0usize, 0usize);
    for (stream, line) in receiver {
        on_output(stream, &line);
        let number = if stream == "stdout" { stdout_line += 1; stdout_line } else { stderr_line += 1; stderr_line };
        output_text.push_str(&format!("[{stream}:{number:04}] {line}\n"));
    }
    let status = child.wait().with_context(|| format!(
        "{stage}: não foi possível aguardar o Git. Comando: {display_command}. Diretório: {}",
        cwd.display()
    ))?;
    if !status.success() {
        let details = if !output_text.trim().is_empty() { output_text.trim() }
            else { "O Git encerrou sem fornecer detalhes em stdout ou stderr." };
        bail!("{stage} falhou.\nComando: {display_command}\nDiretório: {}\nCódigo de saída: {:?}\nSaída original do Git:\n{}",
            cwd.display(), status.code(), details);
    }
    Ok(())
}

fn clone_source<F>(def: &Definition, source: &Path, on_output: &mut F) -> Result<()>
where F: FnMut(&str, &str) {
    if let Some(parent) = source.parent() { fs::create_dir_all(parent)?; }
    git_command_output(
        &["clone", "--branch", def.branch, "--depth", "1", def.repository, source.to_string_lossy().as_ref()],
        source.parent().unwrap_or(source), "Git Clone", on_output,
    )
}

fn ensure_source<F>(def: &Definition, source: &Path, dev_mode: bool, on_output: &mut F) -> Result<()>
where F: FnMut(&str, &str) {
    if source.is_dir() && source.join(".git").exists() {
        if dev_mode {
            on_output("system", &format!("Sincronizando com origin/{}: a versão remota será a fonte da verdade.", def.branch));
            git_command_output(&["fetch", "--prune", "origin", &format!("+refs/heads/{0}:refs/remotes/origin/{0}", def.branch)], source, "Git Fetch", on_output)?;
            git_command_output(&["checkout", "-B", def.branch, &format!("origin/{}", def.branch)], source, "Git Checkout forçado", on_output)?;
            git_command_output(&["reset", "--hard", &format!("origin/{}", def.branch)], source, "Git Reset --hard", on_output)?;
            git_command_output(&["clean", "-fdx"], source, "Git Clean", on_output)?;
        } else {
            let source_text = source.to_string_lossy().into_owned();
            let git_cwd = source.parent().unwrap_or(source);
            on_output("system", &format!("Validando repositório antes da atualização: {}", source.display()));
            git_command_output(&["-C", &source_text, "rev-parse", "--show-toplevel"], git_cwd, "Validação do diretório Git (rev-parse)", on_output)
                .with_context(|| format!("A pasta registrada como código-fonte não é um repositório Git válido. Caminho esperado: {}. Verifique se o diretório existe e contém .git (pasta ou arquivo de worktree).", source.display()))?;
            git_command_output(&["-C", &source_text, "remote", "get-url", "origin"], git_cwd, "Validação do remoto origin", on_output)
                .with_context(|| format!("O repositório em {} não possui um remoto 'origin' válido.", source.display()))?;
            git_command_output(&["-C", &source_text, "status", "--short", "--branch"], git_cwd, "Diagnóstico do estado local antes do pull", on_output)?;
            git_command_output(&["-C", &source_text, "pull", "--ff-only", "origin", def.branch], git_cwd, "Atualização do repositório (Git Pull --ff-only)", on_output)
                .with_context(|| format!("Não foi possível atualizar o repositório. Diretório validado: {}. Remoto esperado: {}. Branch esperada: {}. O Git precisa informar se a causa foi autenticação, rede, remoto divergente ou histórico que não permite fast-forward.", source.display(), def.repository, def.branch))?;
        }
    } else {
        if source.exists() {
            if dev_mode {
                on_output("system", "A pasta source não contém um repositório Git válido. Removendo-a para reconstruir a cópia DEV...");
                fs::remove_dir_all(source).with_context(|| format!("Não foi possível remover a pasta source inválida: {}", source.display()))?;
            } else if fs::read_dir(source)?.next().is_some() {
                bail!("A pasta de destino existe e não está vazia: {}", source.display());
            }
        }
        clone_source(def, source, on_output)?;
    }
    Ok(())
}
fn prepare_shared(def: &Definition, source: &Path) -> Result<String> {
    let example_path = source.join("shared.config.example.json");
    if !example_path.is_file() {
        bail!("O repositório de {} não contém shared.config.example.json na raiz.", def.name);
    }
    let example: Value = serde_json::from_slice(&fs::read(&example_path)?)
        .context("shared.config.example.json contém JSON inválido.")?;
    let declared_id = example.pointer("/application/id").and_then(Value::as_str)
        .ok_or_else(|| anyhow!("shared.config.example.json não declara application.id."))?;
    if declared_id != def.shared_app_id {
        bail!("Identidade Shared divergente para {}: esperado '{}', encontrado '{}'.", def.id, def.shared_app_id, declared_id);
    }
    let config = source.join("shared.config.json");
    if !config.exists() {
        fs::copy(&example_path, &config)
            .with_context(|| format!("Não foi possível criar {}", config.display()))?;
    }
    super::manager::register_app_at_path(declared_id, def.name, &source.to_string_lossy())?;
    super::dependency_queue::enqueue_application(declared_id, &source.to_string_lossy())?;
    super::dependency_queue::process_queue().context("Falha ao resolver as dependências declaradas pelo aplicativo.")?;
    let dependency_status = super::dependency_queue::status()?;
    if dependency_status.get("status").and_then(Value::as_str) == Some("error") {
        let details = dependency_status.get("results").and_then(Value::as_array)
            .map(|items| items.iter().filter_map(|item| item.get("message").and_then(Value::as_str)).collect::<Vec<_>>().join("; "))
            .unwrap_or_else(|| "Uma ou mais dependências não puderam ser instaladas.".to_owned());
        bail!("A preparação das dependências falhou: {details}");
    }
    Ok(declared_id.to_owned())
}

fn install_node_dependencies<F>(def: &Definition, source: &Path, on_output: &mut F) -> Result<PathBuf>
where F: FnMut(&str, &str) {
    let workdir = source.join(def.frontend_dir);
    if !workdir.join("package.json").is_file() {
        bail!("Não foi encontrado package.json em {}", workdir.display());
    }
    command_output("npm", &["install"], &workdir, "instalação das dependências Node.js", on_output)?;
    Ok(workdir)
}

fn build_application<F>(def: &Definition, source: &Path, workdir: &Path, on_output: &mut F) -> Result<PathBuf>
where F: FnMut(&str, &str) {
    write_status("running", &format!("Compilando {}", def.name), def.id, 65, None)?;
    let (program, args): (&str, Vec<&str>) = if def.id == "nova-generator" {
        ("npm", vec!["run", "tauri", "--workspace", "@nova-generator/desktop", "--", "build"])
    } else {
        ("npm", vec!["run", "tauri", "build"])
    };
    command_output(program, &args, if def.id == "nova-generator" { source } else { workdir }, "build Tauri", on_output)?;
    let executable_name = if cfg!(windows) { format!("{}.exe", def.cargo_package) } else { def.cargo_package.to_owned() };
    let exe = source.join("apps/desktop/src-tauri/target/release").join(executable_name);
    if !exe.is_file() {
        bail!("O build terminou, mas o executável esperado não foi encontrado: {}", exe.display());
    }
    Ok(exe)
}

pub fn dev_spec(app_id: &str) -> Result<(PathBuf, String, String)> {
    let def = definition(app_id)?;
    let root = super::manager::root()?;
    let source = root.join("applications").join(def.id).join("source");
    let workdir = source.join(def.frontend_dir);
    let command = if def.id == "nova-generator" {
        "npm run tauri --workspace @nova-generator/desktop -- dev".to_owned()
    } else {
        "npm run tauri dev".to_owned()
    };
    Ok((workdir, def.name.to_owned(), command))
}

pub fn install(app_id: &str, mode: &str) -> Result<()> {
    install_with_output(app_id, mode, |_, _| {})
}

pub fn install_with_output<F>(app_id: &str, mode: &str, mut on_output: F) -> Result<()>
where F: FnMut(&str, &str) {
    if !matches!(mode, "dev" | "executable") {
        bail!("Modo inválido. Use 'dev' ou 'executable'.");
    }
    let def = definition(app_id)?;
    let root = super::manager::root()?;
    let source = root.join("applications").join(def.id).join("source");
    fs::create_dir_all(source.parent().expect("source has parent"))?;
    write_status("installing", &format!("Obtendo código-fonte de {}", def.name), def.id, 5, None)?;
    on_output("system", "Obtendo código-fonte pelo Bash...");
    ensure_source(def, &source, mode == "dev", &mut on_output)?;
    write_status("running", "Lendo shared.config.example.json e preparando o Shared Runtime", def.id, 20, None)?;
    on_output("system", "Preparando configurações e dependências compartilhadas...");
    let shared_app_id = prepare_shared(def, &source)?;
    write_status("running", "Instalando dependências próprias do aplicativo", def.id, 45, None)?;
    on_output("system", "Instalando dependências Node.js pelo Bash...");
    let workdir = install_node_dependencies(def, &source, &mut on_output)?;

    let launch_path = if mode == "executable" {
        let built = build_application(def, &source, &workdir, &mut on_output)?;
        let install_dir = root.join("applications").join(def.id).join("installed");
        fs::create_dir_all(&install_dir)?;
        let executable_name = if cfg!(windows) { format!("{}.exe", def.cargo_package) } else { def.cargo_package.to_owned() };
        let destination = install_dir.join(executable_name);
        fs::copy(&built, &destination)
            .with_context(|| format!("Não foi possível instalar o executável em {}", destination.display()))?;
        Some(destination)
    } else {
        None
    };

    let mut registry = read_registry(&root)?;
    registry.schema_version = 1;
    registry.applications.insert(def.id.to_owned(), InstalledApplication {
        id: def.id.to_owned(),
        name: def.name.to_owned(),
        repository: def.repository.to_owned(),
        branch: def.branch.to_owned(),
        mode: mode.to_owned(),
        version: "source-main".to_owned(),
        source_path: source.display().to_string(),
        launch_path: launch_path.as_ref().map(|path| path.display().to_string()),
        installed_at: timestamp(),
        shared_app_id,
    });
    write_registry(&root, &registry)?;
    write_status("completed", &format!("{} instalado em modo {}", def.name, mode), def.id, 100, Some(json!({
        "id": def.id, "status": "completed", "message": if mode == "dev" { "Projeto clonado, dependências preparadas e configuração Shared gerada." } else { "Projeto compilado e executável instalado na biblioteca central." }
    })))?;
    Ok(())
}

pub fn open(app_id: &str) -> Result<()> {
    let root = super::manager::root()?;
    let registry = read_registry(&root)?;
    let app = registry.applications.get(app_id)
        .ok_or_else(|| anyhow!("O aplicativo '{app_id}' ainda não foi instalado pela central."))?;
    if app.mode == "dev" {
        bail!("Aplicativos DEV devem ser iniciados pelo Monitor de Aplicações do Shared Runtime Manager.");
    }
    let executable = app.launch_path.as_ref().map(PathBuf::from)
        .ok_or_else(|| anyhow!("O executável de '{app_id}' não está registrado."))?;
    if !executable.is_file() {
        bail!("O executável instalado não foi encontrado: {}", executable.display());
    }
    Command::new(executable).current_dir(PathBuf::from(&app.source_path))
        .spawn().context("Não foi possível iniciar o aplicativo instalado.")?;
    Ok(())
}
