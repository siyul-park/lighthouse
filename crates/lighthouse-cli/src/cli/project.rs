//! The commands that set up and tend a project: `init`, `trust`, `cache clean`.

use std::{
    env, fs,
    io::{IsTerminal, Write},
    path::Path,
};

use lighthouse_session::{DEFAULT_CONFIG, FILE_NAME, Session};

use crate::cli::{Result, command::Agent, setup};

/// What `init` keeps out of version control: the store is a local cache of
/// one machine's checks and reviews.
const STORE_IGNORE: &str = ".lighthouse/*.db*";

/// The decision log is committed; branches that both appended merge by
/// keeping the lines of both.
const LOG_ATTRIBUTES: &str = ".lighthouse/decisions.jsonl merge=union";

pub fn cache_clean() -> Result<u8> {
    let root = lighthouse_session::project_root()?;
    let dir = lighthouse_session::cache_dir(&root);
    let had = lighthouse_session::clean_cache(&root)?;
    println!(
        "{} {}",
        if had {
            "deleted"
        } else {
            "nothing to delete at"
        },
        dir.display()
    );
    Ok(0)
}

pub fn trust(config: Option<&Path>, revoke: bool, yes: bool) -> Result<u8> {
    if revoke {
        let root = lighthouse_session::project_root()?;
        let had = lighthouse_session::revoke(&root)?;
        println!(
            "{} {}",
            if had {
                "no longer trusting"
            } else {
                "was not trusted:"
            },
            root.display()
        );
        return Ok(0);
    }
    let session = Session::load(config)?;
    println!("{} runs:", session.root.display());
    for command in &session.basis.commands {
        println!("  {command}");
    }
    if session.basis.commands.is_empty() {
        println!("  no formatter or fixer command");
    }
    if !yes && !confirmed()? {
        return Err(
            "not trusted: no confirmation (use a terminal, or `--yes` after reading the list)"
                .into(),
        );
    }
    let file = lighthouse_session::trust(&session.root, &session.basis)?;
    println!(
        "trusting {} as listed (recorded in {}); programs outside the project are trusted by command line only",
        session.root.display(),
        file.display()
    );
    Ok(0)
}

/// Asks on the terminal whether to trust; false without one.
fn confirmed() -> Result<bool> {
    if !std::io::stdin().is_terminal() {
        return Ok(false);
    }
    print!("Trust this project to run them? [y/N] ");
    std::io::stdout().flush()?;
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    Ok(matches!(answer.trim(), "y" | "Y" | "yes"))
}

pub fn init(agent: Option<Agent>) -> Result<u8> {
    let dir = env::current_dir()?;
    let path = dir.join(FILE_NAME);
    if path.exists() {
        if agent.is_none() {
            return Err(format!("{} already exists", path.display()).into());
        }
        println!("kept {}", path.display());
    } else {
        fs::write(&path, DEFAULT_CONFIG)?;
        println!("wrote {}", path.display());
        if ensure_line(&path.with_file_name(".gitignore"), STORE_IGNORE)? {
            println!("added {STORE_IGNORE} to .gitignore");
        }
        if ensure_line(&path.with_file_name(".gitattributes"), LOG_ATTRIBUTES)? {
            println!("added the decision log to .gitattributes (merge=union)");
        }
    }
    if let Some(Agent::ClaudeCode) = agent {
        setup::claude_code(&dir)?;
    }
    Ok(0)
}

/// Adds `line` to the file, creating it if needed; false when the file
/// already has it.
fn ensure_line(path: &Path, line: &str) -> Result<bool> {
    let mut text = fs::read_to_string(path).unwrap_or_default();
    if text.lines().any(|existing| existing.trim() == line) {
        return Ok(false);
    }
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(line);
    text.push('\n');
    fs::write(path, text)?;
    Ok(true)
}
