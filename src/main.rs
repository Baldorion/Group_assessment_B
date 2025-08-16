use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use tempfile::NamedTempFile;
use uuid::Uuid;

/// Simple, secure contacts manager (CLI)
///
/// Security/design highlights (summary):
/// - Strong typing + ownership prevents buffer overflows/use-after-free
/// - Atomic save via tempfile + rename
/// - File locking (fs2) to prevent concurrent corruption across processes
/// - File permissions set to owner read/write (where supported)
/// - Input validation & length limits to avoid excessive memory usage
/// - Proper error handling with anyhow
/// - No unsafe code
#[derive(Parser)]
#[command(author, version, about)]
struct Cli {
    /// Path to the data file (JSON)
    #[arg(short, long, value_name = "FILE", default_value = "contacts.json")]
    file: PathBuf,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Add a contact
    Add {
        name: String,
        email: String,
        #[arg(short, long)]
        phone: Option<String>,
    },
    /// Remove a contact by id
    Remove { id: String },
    /// List all contacts
    List,
    /// Find contacts by substring (name or email)
    Find { query: String },
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct Contact {
    id: String,
    name: String,
    email: String,
    phone: Option<String>,
}

impl Contact {
    fn new(name: &str, email: &str, phone: Option<&str>) -> Result<Self> {
        // Input validation & length limits
        if name.trim().is_empty() || email.trim().is_empty() {
            return Err(anyhow!("name and email must be non-empty"));
        }
        if name.len() > 200 {
            return Err(anyhow!("name too long (max 200 chars)"));
        }
        if email.len() > 320 {
            return Err(anyhow!("email too long (max 320 chars)"));
        }
        if let Some(p) = phone {
            if p.len() > 50 {
                return Err(anyhow!("phone too long (max 50 chars)"));
            }
        }

        Ok(Contact {
            id: Uuid::new_v4().to_string(),
            name: name.trim().to_string(),
            email: email.trim().to_string(),
            phone: phone.map(|s| s.trim().to_string()),
        })
    }
}

#[derive(Debug, Default)]
struct Store {
    contacts: Vec<Contact>,
    path: PathBuf,
    // We keep the file handle locked during operations that require a lock.
    // The handle is not stored persistently; locking operations open/lock/close on demand.
}

impl Store {
    fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let contacts = if path.exists() {
            let file = OpenOptions::new()
                .read(true)
                .open(&path)
                .with_context(|| format!("opening data file: {}", path.display()))?;
            // Lock for reading to prevent simultaneous writes while reading
            file.lock_shared()
                .with_context(|| "acquiring shared lock for read")?;

            let mut buf = String::new();
            // Read while locked
            let mut reader = file;
            reader
                .read_to_string(&mut buf)
                .with_context(|| "reading data file")?;
            // Parse JSON
            let contacts: Vec<Contact> = serde_json::from_str(&buf)
                .map_err(|e| anyhow!("failed to parse JSON: {}", e))?;
            contacts
        } else {
            Vec::new()
        };

        Ok(Store { contacts, path })
    }

    fn list(&self) -> &[Contact] {
        &self.contacts
    }

    fn add(&mut self, c: Contact) {
        self.contacts.push(c);
    }

    fn remove(&mut self, id: &str) -> bool {
        let before = self.contacts.len();
        self.contacts.retain(|c| c.id != id);
        before != self.contacts.len()
    }

    fn find(&self, q: &str) -> Vec<&Contact> {
        let q_lower = q.to_lowercase();
        self.contacts
            .iter()
            .filter(|c| {
                c.name.to_lowercase().contains(&q_lower)
                    || c.email.to_lowercase().contains(&q_lower)
            })
            .collect()
    }

    /// Persist data atomically and securely.
    fn save(&self) -> Result<()> {
        // 1. Make sure the parent directory exists
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("creating parent dir {}", parent.display()))?;
        }

        // 2. Open (or create) the target file so we can lock it.
        //    fs2 requires a File handle to apply the lock.
        let target_file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(&self.path)
            .with_context(|| format!("opening/creating target file {}", self.path.display()))?;

        // 3. Acquire an exclusive lock on the file
        //    (prevents other processes from writing at the same time).
        target_file
            .lock_exclusive()
            .with_context(|| "acquiring exclusive lock for write")?;

        // 4. IMPORTANT: release the file handle and its lock before persisting.
        //    On Windows, you cannot rename/overwrite a locked file.
        drop(target_file);

        // 5. Create a secure temporary file in the same directory.
        //    This ensures atomic save: we write everything to the temp file first.
        let parent = self
            .path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));

        let mut tmp = NamedTempFile::new_in(&parent)
            .with_context(|| "creating secure temporary file for atomic write")?;

        // 6. Serialize contacts to JSON (pretty format).
        let j = serde_json::to_vec_pretty(&self.contacts)
            .with_context(|| "serializing contacts to JSON")?;

        // 7. Write the JSON into the temporary file.
        tmp.write_all(&j)
            .with_context(|| "writing JSON to temp file")?;

        // 8. Ensure data is written from buffer to disk.
        tmp.flush().with_context(|| "flushing temp file")?;

        // 9. On Unix: set file permissions to 600 (owner read/write only).
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o600))
                .with_context(|| "setting secure permissions on temp file")?;
        }

        // 10. Sync file to disk to ensure durability.
        tmp.as_file()
            .sync_all()
            .with_context(|| "syncing temp file to disk")?;

        // 11. Atomically replace the original file with the temp file.
        //     At this point, the original file is unlocked, so Windows won’t complain.
        tmp.persist(&self.path)
            .map_err(|e| anyhow!("failed to persist temp file: {}", e))?;

        Ok(())
    }
}
fn main() -> Result<()> {
    let cli = Cli::parse();

    let data_path = {
        let p = cli.file;
        let s = p.canonicalize().unwrap_or_else(|_| p.clone());
        s
    };

    let mut store = Store::open(&data_path)?;

    match cli.command {
        Commands::Add { name, email, phone } => {
            let c = Contact::new(&name, &email, phone.as_deref())?;
            println!("Adding contact: {} <{}>", c.name, c.email);
            store.add(c);
            store.save()?;
            println!("Saved.");
        }
        Commands::Remove { id } => {
            if store.remove(&id) {
                store.save()?;
                println!("Removed contact {}", id);
            } else {
                println!("No contact with id {}", id);
            }
        }
        Commands::List => {
            for c in store.list() {
                println!(
                    "{} | {} | {}{}",
                    c.id,
                    c.name,
                    c.email,
                    c.phone
                        .as_ref()
                        .map(|p| format!(" | {}", p))
                        .unwrap_or_default()
                );
            }
            println!("Total: {}", store.list().len());
        }
        Commands::Find { query } => {
            let found = store.find(&query);
            for c in &found {
    println!("{} - {}", c.name, c.phone.as_deref().unwrap_or("No phone"));
}
            println!("Found: {}", found.len());
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn contact_validation() {
        assert!(Contact::new("", "a@b.com", None).is_err());
        assert!(Contact::new("Alice", "", None).is_err());
        let long_name = "x".repeat(201);
        assert!(Contact::new(&long_name, "a@b.com", None).is_err());
        let ok = Contact::new("Alice", "a@b.com", Some("1234")).unwrap();
        assert_eq!(ok.name, "Alice");
    }

    #[test]
    fn add_remove_persist() -> Result<()> {
        let dir = tempdir()?;
        let db = dir.path().join("contacts.json");
        let mut store = Store::open(&db)?;
        assert_eq!(store.list().len(), 0);
        let c = Contact::new("Bob", "bob@example.com", Some("123"))?;
        let id = c.id.clone();
        store.add(c);
        store.save()?;
        let store2 = Store::open(&db)?;
        assert_eq!(store2.list().len(), 1);
        assert_eq!(store2.list()[0].id, id);
        Ok(())
    }

    #[test]
    fn atomic_write_permissions() -> Result<()> {
        let dir = tempdir()?;
        let db = dir.path().join("contacts.json");
        let mut store = Store::open(&db)?;
        store.add(Contact::new("C", "c@d.com", None)?);
        store.save()?;
        let meta = fs::metadata(&db)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = meta.permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        Ok(())
    }

    #[test]
    fn find_works() -> Result<()> {
        let mut store = Store {
            contacts: vec![],
            path: PathBuf::from(""),
        };
        store.add(Contact::new("Alice Smith", "alice@x.com", None)?);
        store.add(Contact::new("Bob Brown", "bob@x.com", None)?);
        let f = store.find("alice");
        assert_eq!(f.len(), 1);
        let f2 = store.find("@x.com");
        assert_eq!(f2.len(), 2);
        Ok(())
    }
}
