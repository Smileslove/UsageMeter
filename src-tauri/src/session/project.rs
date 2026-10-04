//! Git metadata establishes repository identity independently of agent directory layouts.
use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct GitProject {
    pub common_dir: String,
    pub path: String,
    pub name: String,
}

enum Repository {
    Found {
        common_dir: PathBuf,
        primary_path: Option<PathBuf>,
    },
    NotGit,
    Unavailable,
}

fn referenced_path(base: &Path, value: &str) -> Option<PathBuf> {
    let path = Path::new(value.trim_end_matches(['\r', '\n']));
    if path.as_os_str().is_empty() {
        return None;
    }
    fs::canonicalize(base.join(path)).ok()
}

fn repository(cwd: &Path) -> Repository {
    let Ok(cwd) = fs::canonicalize(cwd) else {
        return Repository::Unavailable;
    };
    if !cwd.is_dir() {
        return Repository::Unavailable;
    }
    for root in cwd.ancestors() {
        let marker = root.join(".git");
        match marker.try_exists() {
            Ok(false) => continue,
            Err(_) => return Repository::Unavailable,
            Ok(true) => {}
        }
        let git_dir = if marker.is_dir() {
            fs::canonicalize(&marker).ok()
        } else {
            fs::read_to_string(&marker)
                .ok()
                .and_then(|contents| referenced_path(root, contents.strip_prefix("gitdir: ")?))
        };
        let Some(git_dir) = git_dir.filter(|path| path.is_dir() && path.join("HEAD").is_file())
        else {
            return Repository::Unavailable;
        };
        let common = git_dir.join("commondir");
        let mut primary_path = None;
        let common_dir = match common.try_exists() {
            Ok(false) => {
                primary_path = Some(root.to_path_buf());
                Some(git_dir)
            }
            Ok(true) => fs::read_to_string(common)
                .ok()
                .and_then(|value| referenced_path(&git_dir, &value)),
            Err(_) => None,
        };
        return match common_dir.filter(|path| path.is_dir() && path.join("objects").is_dir()) {
            Some(common_dir) => Repository::Found {
                common_dir,
                primary_path,
            },
            None => Repository::Unavailable,
        };
    }
    Repository::NotGit
}

fn primary_worktree(cwd: &Path) -> Option<(PathBuf, bool)> {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(cwd)
        .args(["worktree", "list", "--porcelain", "-z"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    // Do not inherit the agent's repository overrides when inspecting another cwd.
    for key in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_COMMON_DIR",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_INDEX_FILE",
        "GIT_NAMESPACE",
        "GIT_CEILING_DIRECTORIES",
        "GIT_CONFIG",
        "GIT_CONFIG_COUNT",
        "GIT_CONFIG_PARAMETERS",
    ] {
        command.env_remove(key);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = command.spawn().ok()?;
    let stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.take(1_048_576).read_to_end(&mut bytes).ok()?;
        Some(bytes)
    });
    let deadline = Instant::now() + Duration::from_secs(2);
    let success = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break false;
            }
        }
    };
    let bytes = reader.join().ok()??;
    if !success {
        return None;
    }
    let first = bytes.split(|byte| *byte == 0).next()?;
    let path = std::str::from_utf8(first).ok()?.strip_prefix("worktree ")?;
    let bare = bytes
        .split(|byte| *byte == 0)
        .skip(1)
        .take_while(|field| !field.is_empty())
        .any(|field| field == b"bare");
    Some((fs::canonicalize(path).ok()?, bare))
}

#[derive(Default)]
pub(crate) struct ProjectResolver {
    repositories: HashMap<PathBuf, Option<GitProject>>,
}

impl ProjectResolver {
    fn valid_project(project: &GitProject) -> bool {
        match repository(Path::new(&project.path)) {
            Repository::Found {
                common_dir,
                primary_path: Some(root),
            } => common_dir == Path::new(&project.common_dir) && root == Path::new(&project.path),
            // A bare repository is its own common directory and has no .git marker.
            _ => {
                project.path == project.common_dir
                    && Path::new(&project.path).join("HEAD").is_file()
                    && Path::new(&project.path).join("objects").is_dir()
            }
        }
    }

    pub fn remember(&mut self, project: &GitProject) {
        if Self::valid_project(project) {
            self.repositories
                .insert(PathBuf::from(&project.common_dir), Some(project.clone()));
        }
    }

    pub fn resolve(&mut self, cwd: &str, previous: Option<&GitProject>) -> Option<GitProject> {
        let (common_dir, primary_path) = match repository(Path::new(cwd)) {
            Repository::Found {
                common_dir,
                primary_path,
            } => (common_dir, primary_path),
            Repository::NotGit => return None,
            Repository::Unavailable => {
                return previous.and_then(|previous| {
                    self.repositories
                        .get(Path::new(&previous.common_dir))
                        .and_then(|cached| cached.clone())
                        .filter(Self::valid_project)
                        .or_else(|| {
                            // Retain history when its directory disappeared, but do not
                            // trust a former main directory now occupied by another repo.
                            (!Path::new(&previous.path).exists() || Self::valid_project(previous))
                                .then(|| previous.clone())
                        })
                });
            }
        };
        if let Some(path) = primary_path {
            let project = GitProject {
                common_dir: common_dir.to_str()?.to_string(),
                name: path.file_name()?.to_str()?.to_string(),
                path: path.to_str()?.to_string(),
            };
            self.repositories.insert(common_dir, Some(project.clone()));
            return Some(project);
        }
        if let Some(project) = self
            .repositories
            .get(&common_dir)
            .and_then(|cached| cached.as_ref())
            .filter(|project| Self::valid_project(project))
        {
            return Some(project.clone());
        }
        if self
            .repositories
            .get(&common_dir)
            .is_some_and(Option::is_some)
        {
            self.repositories.remove(&common_dir);
        }
        let previous = previous.filter(|project| {
            Path::new(&project.common_dir) == common_dir && Self::valid_project(project)
        });
        if let Some(project) = previous {
            self.repositories.insert(common_dir, Some(project.clone()));
            return Some(project.clone());
        }
        let project = self
            .repositories
            .entry(common_dir.clone())
            .or_insert_with(|| {
                let (path, bare) = primary_worktree(Path::new(cwd))?;
                // Validate the listed checkout too. With separate-git-dir Git may
                // list its metadata directory, which is not a source project.
                match repository(&path) {
                    Repository::Found {
                        common_dir: listed, ..
                    } if listed == common_dir => {}
                    _ if bare && path == common_dir => {}
                    _ => return None,
                }
                Some(GitProject {
                    common_dir: common_dir.to_str()?.to_string(),
                    name: path.file_name()?.to_str()?.to_string(),
                    path: path.to_str()?.to_string(),
                })
            });
        project.clone().or_else(|| previous.cloned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(path: &Path, args: &[&str]) {
        assert!(Command::new("git")
            .arg("-C")
            .arg(path)
            .args(args)
            .output()
            .unwrap()
            .status
            .success());
    }

    fn committed_repo(main: &Path, metadata: Option<&Path>) {
        fs::create_dir(main).unwrap();
        if let Some(metadata) = metadata {
            git(
                main,
                &["init", "--separate-git-dir", metadata.to_str().unwrap()],
            );
        } else {
            git(main, &["init"]);
        }
        git(
            main,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "--allow-empty",
                "-m",
                "init",
            ],
        );
    }

    #[test]
    fn moved_primary_checkout_overrides_reoccupied_old_path() {
        let tmp = tempfile::tempdir().unwrap();
        let main = tmp.path().join("main");
        let moved = tmp.path().join("moved");
        let metadata = tmp.path().join("metadata");
        let linked = tmp.path().join("linked");
        committed_repo(&main, Some(&metadata));
        git(
            &main,
            &["worktree", "add", "--detach", linked.to_str().unwrap()],
        );
        let mut resolver = ProjectResolver::default();
        let previous = resolver.resolve(main.to_str().unwrap(), None).unwrap();
        fs::rename(&main, &moved).unwrap();
        fs::create_dir(&main).unwrap();
        assert_eq!(
            resolver.resolve(linked.to_str().unwrap(), Some(&previous)),
            None
        );
        let current = resolver.resolve(moved.to_str().unwrap(), None).unwrap();
        assert_ne!(previous.path, current.path);
        assert_eq!(
            resolver.resolve(linked.to_str().unwrap(), Some(&previous)),
            Some(current)
        );
        let mut fresh = ProjectResolver::default();
        fresh.remember(&previous);
        assert_eq!(fresh.resolve(linked.to_str().unwrap(), None), None);
    }

    #[test]
    fn different_repository_must_not_fall_back_to_previous_identity() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a");
        let b = tmp.path().join("b");
        let metadata = tmp.path().join("metadata-b");
        let linked = tmp.path().join("linked");
        committed_repo(&a, None);
        committed_repo(&b, Some(&metadata));
        git(
            &a,
            &["worktree", "add", "--detach", linked.to_str().unwrap()],
        );
        let previous = ProjectResolver::default()
            .resolve(linked.to_str().unwrap(), None)
            .unwrap();
        git(&a, &["worktree", "remove", linked.to_str().unwrap()]);
        git(
            &b,
            &["worktree", "add", "--detach", linked.to_str().unwrap()],
        );
        let mut resolver = ProjectResolver::default();
        assert_eq!(
            resolver.resolve(linked.to_str().unwrap(), Some(&previous)),
            None
        );
        let current = resolver.resolve(b.to_str().unwrap(), None).unwrap();
        assert_eq!(
            resolver.resolve(linked.to_str().unwrap(), Some(&previous)),
            Some(current)
        );
    }

    #[test]
    fn worktree_and_subdirectory_share_project_without_merging_clones() {
        let tmp = tempfile::tempdir().unwrap();
        let main = tmp.path().join("repo");
        fs::create_dir(&main).unwrap();
        git(&main, &["init"]);
        git(
            &main,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "--allow-empty",
                "-m",
                "init",
            ],
        );
        let worktree = tmp.path().join("feature with spaces");
        git(
            &main,
            &["worktree", "add", "--detach", worktree.to_str().unwrap()],
        );
        fs::create_dir(worktree.join("sub")).unwrap();
        let mut resolver = ProjectResolver::default();
        let project = resolver.resolve(main.to_str().unwrap(), None).unwrap();
        assert_eq!(
            resolver.resolve(worktree.to_str().unwrap(), None),
            Some(project.clone())
        );
        assert_eq!(
            resolver.resolve(worktree.join("sub").to_str().unwrap(), None),
            Some(project.clone())
        );

        // Relative gitfiles and commondir paths are resolved against their own directories.
        // Git sanitizes the administrative directory name; use its actual location.
        let admin = fs::read_dir(main.join(".git/worktrees"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        fs::write(
            worktree.join(".git"),
            format!(
                "gitdir: ../repo/.git/worktrees/{}\n",
                admin.file_name().unwrap().to_str().unwrap()
            ),
        )
        .unwrap();
        assert_eq!(
            resolver.resolve(worktree.to_str().unwrap(), None),
            Some(project.clone())
        );
        fs::remove_dir_all(&worktree).unwrap();
        assert_eq!(
            resolver.resolve(worktree.to_str().unwrap(), Some(&project)),
            Some(project.clone())
        );
        fs::create_dir(&worktree).unwrap();
        assert_eq!(
            resolver.resolve(worktree.to_str().unwrap(), Some(&project)),
            None
        );
        git(&worktree, &["init"]);
        assert_ne!(
            resolver
                .resolve(worktree.to_str().unwrap(), None)
                .unwrap()
                .common_dir,
            project.common_dir
        );
    }

    #[test]
    fn separate_git_dir_is_not_mistaken_for_the_project_root() {
        let tmp = tempfile::tempdir().unwrap();
        let main = tmp.path().join("project");
        let metadata = tmp.path().join("metadata");
        fs::create_dir(&main).unwrap();
        git(
            &main,
            &["init", "--separate-git-dir", metadata.to_str().unwrap()],
        );
        let project = ProjectResolver::default()
            .resolve(main.to_str().unwrap(), None)
            .unwrap();
        assert_eq!(Path::new(&project.path), fs::canonicalize(main).unwrap());
        assert_eq!(
            Path::new(&project.common_dir),
            fs::canonicalize(metadata).unwrap()
        );
    }

    #[test]
    fn worktrees_of_bare_repository_share_its_project_path() {
        let tmp = tempfile::tempdir().unwrap();
        let main = tmp.path().join("source");
        let bare = tmp.path().join("repo.git");
        let first = tmp.path().join("first");
        let second = tmp.path().join("second");
        fs::create_dir(&main).unwrap();
        git(&main, &["init"]);
        git(
            &main,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "--allow-empty",
                "-m",
                "init",
            ],
        );
        git(&main, &["clone", "--bare", ".", bare.to_str().unwrap()]);
        git(
            &bare,
            &["worktree", "add", "--detach", first.to_str().unwrap()],
        );
        git(
            &bare,
            &["worktree", "add", "--detach", second.to_str().unwrap()],
        );
        let mut resolver = ProjectResolver::default();
        let project = resolver.resolve(first.to_str().unwrap(), None).unwrap();
        assert_eq!(Path::new(&project.path), fs::canonicalize(&bare).unwrap());
        assert_eq!(
            resolver.resolve(second.to_str().unwrap(), None),
            Some(project)
        );
    }

    #[test]
    fn nested_repository_and_broken_gitfile_do_not_fall_through_to_parent() {
        let tmp = tempfile::tempdir().unwrap();
        git(tmp.path(), &["init"]);
        let sub = tmp.path().join("nested");
        fs::create_dir(&sub).unwrap();
        git(&sub, &["init"]);
        let mut resolver = ProjectResolver::default();
        assert_ne!(
            resolver.resolve(tmp.path().to_str().unwrap(), None),
            resolver.resolve(sub.to_str().unwrap(), None)
        );
        fs::remove_dir_all(sub.join(".git")).unwrap();
        fs::write(sub.join(".git"), "gitdir: missing\n").unwrap();
        assert_eq!(resolver.resolve(sub.to_str().unwrap(), None), None);
    }
}
