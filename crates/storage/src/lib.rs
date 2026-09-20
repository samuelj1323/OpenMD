//! openmd-storage: a vault is a directory of `.md` files.
//!
//! Files are the source of truth. A SQLite database (with FTS5) under
//! `<vault>/.openmd/index.db` indexes page metadata and body text so the
//! UI can list and search without re-reading every file.

use openmd_core::{parse_page, serialize_page, split_frontmatter, Page};
use std::path::{Path, PathBuf};
use thiserror::Error;

/// All errors from vault / index operations.
#[derive(Debug, Error)]
pub enum Error {
    /// Filesystem failure.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// SQLite failure.
    #[error("db: {0}")]
    Db(#[from] rusqlite::Error),
}

/// A search hit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub id: String,
    pub title: String,
    pub path: PathBuf,
}

/// Lightweight page reference (no body read).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageMeta {
    pub id: String,
    pub title: String,
    pub path: PathBuf,
}

/// Turn a title into a filename-safe slug.
pub fn slugify(title: &str) -> String {
    let mut slug: String = title
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    while slug.contains("--") {
        slug = slug.replace("--", "-");
    }
    let slug = slug.trim_matches('-').to_string();
    let short: String = slug.chars().take(50).collect();
    if short.is_empty() {
        "page".to_string()
    } else {
        short
    }
}

/// Filename for a page: `<slug>-<id-prefix>.md`.
pub fn filename(page: &Page) -> String {
    let prefix: String = page.id.chars().take(8).collect();
    format!("{}-{}.md", slugify(&page.title), prefix)
}

/// A vault root on disk.
pub struct Vault {
    root: PathBuf,
}

impl Vault {
    /// Open (creating if needed) a vault at `root`.
    pub fn open(root: impl AsRef<Path>) -> Result<Self, Error> {
        let root = root.as_ref().to_path_buf();
        std::fs::create_dir_all(root.join("pages"))?;
        Ok(Self { root })
    }

    /// Vault root directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Directory holding page files.
    pub fn pages_dir(&self) -> PathBuf {
        self.root.join("pages")
    }

    /// Default SQLite index path.
    pub fn index_path(&self) -> PathBuf {
        self.root.join(".openmd/index.db")
    }

    /// List all pages (reads only frontmatter, not bodies).
    pub fn list(&self) -> Result<Vec<PageMeta>, Error> {
        let mut metas = Vec::new();
        for entry in walkdir::WalkDir::new(self.pages_dir())
            .max_depth(1)
            .into_iter()
            .filter_map(Result::ok)
        {
            let path = entry.path().to_path_buf();
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            if !entry.file_type().is_file() {
                continue;
            }
            let text = std::fs::read_to_string(&path)?;
            let (id, title, _) = split_frontmatter(&text);
            metas.push(PageMeta {
                id: id.unwrap_or_else(openmd_core::new_id),
                title: title.unwrap_or_else(|| {
                    path.file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("Untitled")
                        .to_string()
                }),
                path,
            });
        }
        metas.sort_by_key(|a| a.title.to_lowercase());
        Ok(metas)
    }

    /// Read and parse one page file.
    pub fn read(&self, path: &Path) -> Result<Page, Error> {
        Ok(parse_page(&std::fs::read_to_string(path)?))
    }

    /// Write a page to disk. Returns the file path.
    pub fn write(&self, page: &Page) -> Result<PathBuf, Error> {
        let path = self.pages_dir().join(filename(page));
        std::fs::write(&path, serialize_page(page))?;
        Ok(path)
    }
}

/// SQLite metadata + full-text index over a vault.
pub struct Index {
    conn: rusqlite::Connection,
}

impl Index {
    /// Open (creating parent dirs) the index database.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, Error> {
        if let Some(parent) = path.as_ref().parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let conn = rusqlite::Connection::open(path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS pages(
                 id TEXT PRIMARY KEY, title TEXT NOT NULL, path TEXT NOT NULL
             );
             CREATE VIRTUAL TABLE IF NOT EXISTS page_fts USING fts5(id, title, body);",
        )?;
        Ok(Self { conn })
    }

    /// Rebuild the whole index from the vault's current files.
    pub fn rebuild(&mut self, vault: &Vault) -> Result<usize, Error> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM pages", [])?;
        tx.execute("DELETE FROM page_fts", [])?;
        let mut count = 0;
        for meta in vault.list()? {
            let body = vault
                .read(&meta.path)
                .map(|p| p.body_text())
                .unwrap_or_default();
            tx.execute(
                "INSERT OR REPLACE INTO pages(id, title, path) VALUES (?1, ?2, ?3)",
                rusqlite::params![meta.id, meta.title, meta.path.to_string_lossy().to_string()],
            )?;
            tx.execute(
                "INSERT INTO page_fts(id, title, body) VALUES (?1, ?2, ?3)",
                rusqlite::params![meta.id, meta.title, body],
            )?;
            count += 1;
        }
        tx.commit()?;
        Ok(count)
    }

    /// Insert or update one page in the index.
    pub fn upsert(&mut self, meta: &PageMeta, body: &str) -> Result<(), Error> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT OR REPLACE INTO pages(id, title, path) VALUES (?1, ?2, ?3)",
            rusqlite::params![meta.id, meta.title, meta.path.to_string_lossy().to_string()],
        )?;
        tx.execute(
            "DELETE FROM page_fts WHERE id = ?1",
            rusqlite::params![meta.id],
        )?;
        tx.execute(
            "INSERT INTO page_fts(id, title, body) VALUES (?1, ?2, ?3)",
            rusqlite::params![meta.id, meta.title, body],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Full-text search. Falls back to LIKE when the query is not valid
    /// FTS5 syntax.
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<Hit>, Error> {
        let stmt = self.conn.prepare(
            "SELECT p.id, p.title, p.path FROM page_fts f
             JOIN pages p ON p.id = f.id
             WHERE page_fts MATCH ?1 LIMIT ?2",
        );
        match stmt {
            Ok(mut q) => {
                let rows = q.query_map(rusqlite::params![query, limit as i64], |row| {
                    Ok(Hit {
                        id: row.get(0)?,
                        title: row.get(1)?,
                        path: PathBuf::from(row.get::<_, String>(2)?),
                    })
                });
                match rows {
                    Ok(mapped) => mapped.collect::<Result<Vec<_>, _>>().map_err(Error::Db),
                    Err(e) => Err(Error::Db(e)),
                }
            }
            Err(_) => self.like_search(query, limit),
        }
        .or_else(|e| match e {
            Error::Db(rusqlite::Error::SqliteFailure(..)) => self.like_search(query, limit),
            other => Err(other),
        })
    }

    fn like_search(&self, query: &str, limit: usize) -> Result<Vec<Hit>, Error> {
        let mut stmt = self.conn.prepare(
            "SELECT p.id, p.title, p.path FROM page_fts f
             JOIN pages p ON p.id = f.id
             WHERE f.title LIKE ?1 OR f.body LIKE ?1 LIMIT ?2",
        )?;
        let like = format!("%{query}%");
        let rows = stmt.query_map(rusqlite::params![like, limit as i64], |row| {
            Ok(Hit {
                id: row.get(0)?,
                title: row.get(1)?,
                path: PathBuf::from(row.get::<_, String>(2)?),
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Error::Db)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_vault(name: &str) -> (TempDir, Vault) {
        let dir = tempdir(name);
        let vault = Vault::open(dir.path()).expect("open vault");
        (dir, vault)
    }

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.path).ok();
        }
    }

    fn tempdir(name: &str) -> TempDir {
        let path = std::env::temp_dir().join(format!(
            "openmd-test-{}-{}",
            name,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).expect("tempdir");
        TempDir { path }
    }

    #[test]
    fn write_list_read_round_trip() {
        let (_dir, vault) = test_vault("basic");
        let mut page = Page::new("My Note");
        page.blocks.push(openmd_core::Block::Paragraph {
            id: openmd_core::new_id(),
            text: "hello **world**".to_string(),
        });
        let path = vault.write(&page).expect("write");
        assert!(path.exists());

        let metas = vault.list().expect("list");
        assert_eq!(metas.len(), 1);
        assert_eq!(metas[0].title, "My Note");

        let back = vault.read(&metas[0].path).expect("read");
        assert_eq!(back.title, "My Note");
        assert_eq!(back.id, page.id);
        assert_eq!(back.blocks.len(), 1);
    }

    #[test]
    fn index_rebuild_and_search() {
        let (_dir, vault) = test_vault("search");
        let mut a = Page::new("Shopping");
        a.blocks.push(openmd_core::Block::Paragraph {
            id: openmd_core::new_id(),
            text: "buy oat milk".to_string(),
        });
        let mut b = Page::new("Work");
        b.blocks.push(openmd_core::Block::Paragraph {
            id: openmd_core::new_id(),
            text: "ship the roadmap".to_string(),
        });
        vault.write(&a).expect("write a");
        vault.write(&b).expect("write b");

        let mut index = Index::open(vault.index_path()).expect("open index");
        let n = index.rebuild(&vault).expect("rebuild");
        assert_eq!(n, 2);

        let hits = index.search("oat", 10).expect("search");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "Shopping");

        let hits = index.search("ship", 10).expect("search");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "Work");
    }

    #[test]
    fn slugify_is_filename_safe() {
        assert_eq!(slugify("Hello, World!"), "hello-world");
        assert_eq!(slugify("  "), "page");
        assert!(!slugify("Notes: weekly/review").contains('/'));
    }
}
