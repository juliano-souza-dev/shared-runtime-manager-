use anyhow::{anyhow, Result};
use std::path::PathBuf;

pub fn bash_path() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("SRM_BASH_PATH") {
        let candidate = PathBuf::from(path);
        if candidate.is_file() { return Ok(candidate); }
        return Err(anyhow!("SRM_BASH_PATH aponta para um arquivo inexistente: {}", candidate.display()));
    }
    if let Some(path) = find_on_path("bash") { return Ok(path); }
    #[cfg(windows)]
    {
        for candidate in [
            PathBuf::from(r"C:\Program Files\Git\bin\bash.exe"),
            PathBuf::from(r"C:\Program Files (x86)\Git\bin\bash.exe"),
        ] {
            if candidate.is_file() { return Ok(candidate); }
        }
    }
    Err(anyhow!("Bash não encontrado. Instale o Git for Windows (Git Bash) ou Bash e adicione bash ao PATH."))
}

fn find_on_path(name: &str) -> Option<PathBuf> {
    let executable = if cfg!(windows) { format!("{name}.exe") } else { name.to_owned() };
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|directory| directory.join(&executable))
        .find(|candidate| candidate.is_file())
}
