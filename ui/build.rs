//! Injection des parametres de build : la cle d'API et l'URL du serveur SAV
//! sont incorporees au module wasm. Sources (par ordre de priorite):
//! 1. variables d'environnement SAV_API_KEY / SAV_URL
//! 2. le fichier .env a la racine du repo
//!
//! Attention: la cle est donc lisible dans le .wasm - c'est le comportement
//! demande (pas de saisie cote page), la cle doit rester dediee a ce module.

use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let repo_env = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap())
        .parent()
        .unwrap()
        .to_path_buf()
        .join(".env");

    let mut api_key = env::var("SAV_API_KEY").ok().filter(|v| !v.is_empty());
    // SAV_URL peut etre vide (= meme origine que la page au runtime).
    let mut url = env::var("SAV_URL").ok();

    if let Ok(content) = fs::read_to_string(&repo_env) {
        for line in content.lines() {
            let line = line.trim();
            if let Some(value) = line.strip_prefix("SAV_API_KEY=") {
                if api_key.is_none() {
                    api_key = Some(value.trim().to_string());
                }
            }
            if let Some(value) = line.strip_prefix("SAV_URL=") {
                if url.is_none() {
                    url = Some(value.trim().to_string());
                }
            }
        }
    }

    let api_key = api_key.expect("SAV_API_KEY requis au build (variable d environnement ou .env)");
    let url = url.unwrap_or_else(|| "http://localhost:8080".to_string());

    // Titre de l'interface depuis config.toml ([ui] title).
    let title = read_config_title(&repo_env)
        .or_else(|| std::env::var("SAV_TITLE").ok().filter(|t| !t.is_empty()))
        .unwrap_or_else(|| "SAV - Rendez-vous".to_string());

    println!("cargo:rustc-env=SAV_API_KEY={}", api_key);
    println!("cargo:rustc-env=SAV_URL={}", url);
    println!("cargo:rustc-env=SAV_TITLE={}", title);
    println!("cargo:rerun-if-changed={}", repo_env.display());
    println!("cargo:rerun-if-env-changed=SAV_API_KEY");
    println!("cargo:rerun-if-env-changed=SAV_URL");
    println!("cargo:rerun-if-env-changed=SAV_TITLE");
}

/// Extrait le titre depuis la section [ui] de config.toml (parse leger,
/// sans dependance).
fn read_config_title(repo_env: &std::path::Path) -> Option<String> {
    // config.toml est a cote du .env (racine du repo).
    let config_path = repo_env.parent()?.join("config.toml");
    let content = std::fs::read_to_string(config_path).ok()?;

    let mut in_ui_section = false;
    for line in content.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_ui_section = line == "[ui]";
            continue;
        }
        if in_ui_section {
            if let Some(value) = line.strip_prefix("title") {
                let value = value.trim().strip_prefix('=')?.trim();
                let value = value.trim_matches('"');
                if !value.is_empty() {
                    return Some(value.to_string());
                }
            }
        }
    }
    None
}
