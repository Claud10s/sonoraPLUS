use std::io::Write as _;
use std::path::Path;
use std::time::Duration;

use anyhow::{Result, ensure};
use music::Lyrics;
use sha2::{Digest as _, Sha256};

use super::BookData;

/// Materializes immutable lyric copies without replacing files the user may have edited.
/// JSON retains every timing, translation and voice; TXT/LRC provide portable text copies.
pub(super) fn save(data: &BookData) -> Result<()> {
    let Some(folder) = &data.lyrics_folder else {
        return Ok(());
    };
    std::fs::create_dir_all(folder)?;
    for revision in data.revisions.values() {
        let identity = serde_json::to_vec(&(&revision.key, &revision.source, &revision.sheet))?;
        let hash = format!("{:x}", Sha256::digest(identity));
        let name = format!(
            "{} - {} - {} - {}",
            component(&revision.track.artists),
            component(&revision.track.name),
            component(&revision.source),
            hash
        );
        let (extension, text) = portable(&revision.sheet);
        write_new(&folder.join(format!("{name}.{extension}")), text.as_bytes())?;
        write_new(
            &folder.join(format!("{name}.json")),
            &serde_json::to_vec_pretty(&revision.sheet)?,
        )?;
    }
    Ok(())
}

/// Publishes a complete file atomically, leaving any existing file untouched.
fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    if path.try_exists()? {
        ensure!(path.is_file(), "lyrics destination is not a file");
        return Ok(());
    }
    let mut file = tempfile::NamedTempFile::new_in(path.parent().unwrap())?;
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    match file.persist_noclobber(path) {
        Ok(_) => Ok(()),
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(error.into()),
    }
}

/// Keeps filenames short and safe across Windows, macOS and Linux.
fn component(value: &str) -> String {
    let mut text: String = value
        .chars()
        .take(24)
        .map(|c| {
            if c.is_control() || "<>:\"/\\|?*".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    while text.len() > 24 {
        text.pop();
    }
    let text = text.trim_matches([' ', '.']);
    if text.is_empty() {
        "_".into()
    } else {
        text.into()
    }
}

/// Enhanced LRC includes word starts; the accompanying JSON is the lossless copy.
fn portable(sheet: &Lyrics) -> (&'static str, String) {
    match sheet {
        Lyrics::Plain { text, .. } => ("txt", text.to_string()),
        Lyrics::Synced { lines } => {
            let mut text = String::new();
            for line in lines.iter() {
                text.push_str(&format!("[{}]", stamp(line.start)));
                match line.words.as_ref().filter(|words| !words.is_empty()) {
                    Some(words) => {
                        for word in words {
                            text.push_str(&format!("<{}>{}", stamp(word.start), word.text));
                        }
                        if let Some(last) = words.last() {
                            text.push_str(&format!("<{}>", stamp(last.end)));
                        }
                    }
                    None => text.push_str(&line.text),
                }
                text.push('\n');
            }
            ("lrc", text)
        }
    }
}

fn stamp(at: Duration) -> String {
    format!(
        "{:02}:{:02}.{:03}",
        at.as_secs() / 60,
        at.as_secs() % 60,
        at.subsec_millis()
    )
}
