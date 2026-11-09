// SPDX-License-Identifier: MPL-2.0
//! Files the app writes for the user: free names, the Downloads folder, the file manager.

use std::path::Path;

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
    let path = numbered(&base)
        .map(|name| dir.join(format!("{name}.{extension}")))
        .find(|path| !path.exists())
        .expect("an unused name");
    std::fs::write(&path, content).at(&path)?;
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
}
