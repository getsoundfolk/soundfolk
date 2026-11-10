// SPDX-License-Identifier: MPL-2.0
//! Files the app writes for the user: free names, the Downloads folder, the file manager.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::Serialize;
use tauri::{AppHandle, Manager};
use tauri_plugin_opener::OpenerExt;

use crate::error::{AppError, AppResult, IoContext};

/// `base`, then `base-2`, `base-3`…: names to try until one is free.
pub(crate) fn numbered(base: &str) -> impl Iterator<Item = String> + '_ {
    (1..).map(move |n| {
        if n == 1 {
            base.to_owned()
        } else {
            format!("{base}-{n}")
        }
    })
}

/// Writes `content` to the first of `paths` that does not exist yet. The file is created with
/// `create_new`, so two writers racing for the same name never overwrite each other: the loser
/// moves on to the next name.
pub(crate) fn write_new(
    paths: impl IntoIterator<Item = PathBuf>,
    content: &str,
) -> AppResult<PathBuf> {
    for path in paths {
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => {
                file.write_all(content.as_bytes()).at(&path)?;
                return Ok(path);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e).at(&path),
        }
    }
    Err("no free file name".to_owned().into())
}

#[derive(Debug, Serialize, specta::Type)]
pub struct ExportedFile {
    pub path: String,
    /// File name alone, for a short notice.
    pub file_name: String,
    pub count: usize,
}

/// Writes an export to a new file `<stem>-<date>.<extension>` in the Downloads folder (`-2`, `-3`…
/// when it exists), then shows it in the file manager.
pub(crate) fn save_download(
    app: &AppHandle,
    stem: &str,
    extension: &str,
    content: &str,
    count: usize,
) -> AppResult<ExportedFile> {
    let dir = app
        .path()
        .download_dir()
        .or_else(|_| app.path().home_dir())
        .map_err(|e| format!("no folder to save the export in: {e}"))?;
    std::fs::create_dir_all(&dir).at(&dir)?;
    let date = jiff::Timestamp::now().strftime("%Y-%m-%d").to_string();
    let base = format!("{stem}-{date}");
    let path = write_new(
        numbered(&base).map(|name| dir.join(format!("{name}.{extension}"))),
        content,
    )?;
    // The file is saved either way; a file manager that cannot be opened is not an error.
    if let Err(e) = app.opener().reveal_item_in_dir(&path) {
        tracing::warn!("could not show {} in the file manager: {e}", path.display());
    }
    Ok(ExportedFile {
        file_name: path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        path: path.display().to_string(),
        count,
    })
}

/// Opens a file in the user's editor, or a folder in the file manager.
pub(crate) fn open_path(app: &AppHandle, path: &Path) -> AppResult<()> {
    app.opener()
        .open_path(path.display().to_string(), None::<&str>)
        .map_err(|e| AppError::from(format!("could not open {}: {e}", path.display())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbered_names_start_with_the_base() {
        let names: Vec<String> = numbered("fip").take(3).collect();
        assert_eq!(names, ["fip", "fip-2", "fip-3"]);
    }

    #[test]
    fn write_new_skips_names_taken_and_keeps_their_content() {
        let dir = std::env::temp_dir().join(format!("soundfolk-write-new-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("export.txt"), "first").unwrap();
        let names = numbered("export").map(|n| dir.join(format!("{n}.txt")));
        let path = write_new(names, "second").unwrap();
        assert_eq!(path, dir.join("export-2.txt"));
        assert_eq!(
            std::fs::read_to_string(dir.join("export.txt")).unwrap(),
            "first"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "second");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
