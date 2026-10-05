//! Injection des parametres de build dans le module wasm.
//!
//! - SAV_API_KEY / SAV_URL: cle d'API et URL du serveur SAV, depuis les
//!   variables d'environnement puis le .env a la racine du repo.
//! - Toutes les cles de la section [ui] de config.toml (textes de
//!   l'interface: title, placeholder, ...) sont injectees sous la forme
//!   SAV_UI_<CLE> (cle en majuscules), lues cote code via option_env!.
//!
//! Attention: la cle est donc lisible dans le .wasm - c'est le comportement
//! demande (pas de saisie cote page), la cle doit rester dediee a ce module.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    let repo_env = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap())
        .parent()
        .unwrap()
        .to_path_buf()
        .join(".env");
    let repo_dir = repo_env.parent().unwrap().to_path_buf();

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

    println!("cargo:rustc-env=SAV_API_KEY={}", api_key);
    println!("cargo:rustc-env=SAV_URL={}", url);

    // Section [ui] de config.toml: chaque cle texte -> SAV_UI_<CLE>.
    let config_path = repo_dir.join("config.toml");
    if let Some(ui) = read_ui_section(&config_path) {
        for (key, value) in ui {
            println!("cargo:rustc-env=SAV_UI_{}={}", key.to_uppercase(), value);
        }
    }
    println!("cargo:rerun-if-changed={}", config_path.display());

    println!("cargo:rerun-if-changed={}", repo_env.display());
    println!("cargo:rerun-if-env-changed=SAV_API_KEY");
    println!("cargo:rerun-if-env-changed=SAV_URL");
}

/// Cles texte de la section [ui] de config.toml.
fn read_ui_section(config_path: &Path) -> Option<Vec<(String, String)>> {
    let content = match fs::read_to_string(config_path) {
        Ok(content) => content,
        Err(e) => {
            eprintln!("[sav build.rs] lecture {} echouee: {}", config_path.display(), e);
            return None;
        }
    };
    let value: toml::Table = match content.parse() {
        Ok(value) => value,
        Err(e) => {
            eprintln!("[sav build.rs] parse config.toml echoue: {}", e);
            return None;
        }
    };
    let ui = match value.get("ui").and_then(|v| v.as_table()) {
        Some(ui) => ui,
        None => {
            eprintln!("[sav build.rs] section [ui] absente ou non-table");
            return None;
        }
    };

    Some(
        ui.iter()
            .filter_map(|(key, value)| {
                value.as_str().map(|text| (key.clone(), text.to_string()))
            })
            .collect(),
    )
}
