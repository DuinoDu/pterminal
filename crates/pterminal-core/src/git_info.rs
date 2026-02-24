use std::path::{Path, PathBuf};

/// Resolve the current git branch name by walking up from `start_dir`.
/// Returns short commit SHA when in detached HEAD state.
pub fn current_branch(start_dir: &Path) -> Option<String> {
    let git_dir = find_git_dir(start_dir)?;
    read_head_ref(&git_dir)
}

fn find_git_dir(start_dir: &Path) -> Option<PathBuf> {
    let mut cur = Some(start_dir);
    while let Some(dir) = cur {
        let dot_git = dir.join(".git");
        if dot_git.is_dir() {
            return Some(dot_git);
        }
        if dot_git.is_file() {
            let content = std::fs::read_to_string(&dot_git).ok()?;
            if let Some(gitdir) = content.trim().strip_prefix("gitdir:") {
                let gitdir = gitdir.trim();
                let path = Path::new(gitdir);
                return Some(if path.is_absolute() {
                    path.to_path_buf()
                } else {
                    dir.join(path)
                });
            }
        }
        cur = dir.parent();
    }
    None
}

fn read_head_ref(git_dir: &Path) -> Option<String> {
    let head = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let head = head.trim();
    if let Some(reference) = head.strip_prefix("ref: ") {
        return reference.rsplit('/').next().map(str::to_string);
    }
    if head.len() >= 7 {
        Some(head[..7].to_string())
    } else if !head.is_empty() {
        Some(head.to_string())
    } else {
        None
    }
}

