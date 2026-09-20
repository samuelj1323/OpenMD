//! openmd CLI — vault inspection until the GPUI editor shell lands.
//!
//! Usage:
//!   openmd list [vault]          list pages
//!   openmd search [vault] <query>  full-text search page titles/bodies

use openmd_storage::{Index, Vault};
use std::error::Error;
use std::path::PathBuf;

fn default_vault() -> PathBuf {
    std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join("openMD"))
        .unwrap_or_else(|| PathBuf::from("vault-example"))
}

fn ensure_index(vault: &Vault) -> Result<Index, Box<dyn Error>> {
    let mut index = Index::open(vault.index_path())?;
    index.rebuild(vault)?;
    Ok(index)
}

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("list") => {
            let root = args.get(2).map(PathBuf::from).unwrap_or_else(default_vault);
            let vault = Vault::open(&root)?;
            for meta in vault.list()? {
                println!("{}\t{}", meta.id, meta.title);
            }
        }
        Some("search") => {
            let (root, query) = match (args.get(2), args.get(3)) {
                (Some(q1), Some(q2)) => (PathBuf::from(q1), q2.clone()),
                (Some(q), None) => (default_vault(), q.clone()),
                (None, _) => {
                    eprintln!("usage: openmd search [vault] <query>");
                    std::process::exit(2);
                }
            };
            let vault = Vault::open(&root)?;
            let index = ensure_index(&vault)?;
            for hit in index.search(&query, 20)? {
                println!("{}\t{}", hit.id, hit.title);
            }
        }
        _ => {
            println!("OpenMD — open-source Markdown editor (GPUI shell coming in milestone 4)");
            println!();
            println!("usage:");
            println!("  openmd list [vault]");
            println!("  openmd search [vault] <query>");
        }
    }
    Ok(())
}
