use std::collections::{HashMap, HashSet};
use std::fs;

use ai::skills::{ParsedSkill, SkillProvider, SkillScope};
use remote_server::proto::{file_context_proto, FileContextProto};
use repo_metadata::{
    repositories::DetectedRepositories, DirectoryWatcher, RepoMetadataModel, RepositoryUpdate,
    TargetFile,
};
use tempfile::TempDir;
use warp_util::{
    host_id::HostId, local_or_remote_path::LocalOrRemotePath, remote_path::RemotePath,
    standardized_path::StandardizedPath,
};
use warpui::App;

use crate::ai::skills::skill_manager::SkillWatcherEvent;
use super::{parse_remote_skill_file_contexts, SkillWatcher};

/// Helper function for creating a single skill file
fn create_skill_file(dir: &TempDir, name: &str, description: &str, content: &str) -> ParsedSkill {
    let skill_content = format!(
        r#"---
name: {}
description: {}
---
{}
"#,
        name, description, content
    );
    let skills_path = dir.path().join(".agents").join("skills");
    let skill_dir_path = skills_path.join(name);
    let skill_file_path = skill_dir_path.join("SKILL.md");

    fs::create_dir_all(&skill_dir_path).unwrap();
    fs::write(&skill_file_path, skill_content.clone()).unwrap();
    let line_range_start = skill_content.clone().lines().count() - content.lines().count() + 1;
    let line_range_end = skill_content.clone().lines().count() + 1;
    ParsedSkill {
        path: LocalOrRemotePath::Local(skill_file_path),
        name: name.to_string(),
        description: description.to_string(),
        content: skill_content.clone(),
        line_range: Some(line_range_start..line_range_end),
        provider: SkillProvider::Agents,
        scope: SkillScope::Project,
    }
}

fn remote_skill_path(host_id: &HostId, name: &str) -> LocalOrRemotePath {
    LocalOrRemotePath::Remote(RemotePath::new(
        host_id.clone(),
        StandardizedPath::try_new(format!("/repo/.agents/skills/{name}/SKILL.md").as_str())
            .unwrap(),
    ))
}

fn remote_skill_content(name: &str, description: &str, body: &str) -> String {
    format!(
        r#"---
name: {name}
description: {description}
---
{body}
"#
    )
}

fn remote_skill_file_context(path: &LocalOrRemotePath, content: &str) -> FileContextProto {
    let LocalOrRemotePath::Remote(remote) = path else {
        panic!("Expected a remote skill path");
    };

    FileContextProto {
        file_name: remote.path.as_str().to_string(),
        content: Some(file_context_proto::Content::TextContent(
            content.to_string(),
        )),
        line_range_start: None,
        line_range_end: None,
        last_modified_epoch_millis: None,
        line_count: content.lines().count() as u32,
    }
}

#[test]
fn parse_remote_skill_file_contexts_matches_reordered_responses_by_path() {
    let host = HostId::new("test-host".to_string());
    let first_path = remote_skill_path(&host, "first");
    let second_path = remote_skill_path(&host, "second");
    let first_content = remote_skill_content("first", "First skill", "First body");
    let second_content = remote_skill_content("second", "Second skill", "Second body");

    let skills = parse_remote_skill_file_contexts(
        vec![first_path.clone(), second_path.clone()],
        vec![
            remote_skill_file_context(&second_path, &second_content),
            remote_skill_file_context(&first_path, &first_content),
        ],
    );

    assert_eq!(skills.len(), 2);
    assert_eq!(skills[0].path, first_path);
    assert_eq!(skills[0].name, "first");
    assert_eq!(skills[0].content, first_content);
    assert_eq!(skills[1].path, second_path);
    assert_eq!(skills[1].name, "second");
    assert_eq!(skills[1].content, second_content);
}

#[test]
fn parse_remote_skill_file_contexts_keeps_paths_aligned_after_missing_reads() {
    let host = HostId::new("test-host".to_string());
    let missing_path = remote_skill_path(&host, "missing");
    let present_path = remote_skill_path(&host, "present");
    let present_content = remote_skill_content("present", "Present skill", "Present body");

    let skills = parse_remote_skill_file_contexts(
        vec![missing_path, present_path.clone()],
        vec![remote_skill_file_context(&present_path, &present_content)],
    );

    assert_eq!(skills.len(), 1);
    assert_eq!(skills[0].path, present_path);
    assert_eq!(skills[0].name, "present");
    assert_eq!(skills[0].content, present_content);
}

// ============================================================================
// Tests for handle_repository_update
// ============================================================================

#[test]
fn test_handle_repository_update_single_skill_added() {
    let (tx, rx) = async_channel::unbounded();

    App::test((), |mut app| async move {
        app.add_singleton_model(DirectoryWatcher::new_for_testing);
        app.add_singleton_model(|_| DetectedRepositories::default());
        app.add_singleton_model(RepoMetadataModel::new);
        let skill_watcher_handle = app.add_model(|ctx| SkillWatcher::new_for_testing(ctx, tx));

        let temp_dir = TempDir::new().unwrap();
        let skill = create_skill_file(&temp_dir, "test", "Test skill", "Test content");

        let update = RepositoryUpdate {
            added: HashSet::from([TargetFile::new(
                skill.path.to_local_path().unwrap().to_path_buf(),
                false,
            )]),
            modified: HashSet::new(),
            deleted: HashSet::new(),
            moved: HashMap::new(),
            commit_updated: false,
            index_lock_detected: false,
            remote_ref_updated: false,
        };

        skill_watcher_handle.update(&mut app, |skill_watcher, ctx| {
            skill_watcher.handle_repository_update(&update, ctx);
        });

        let event = rx.recv().await.unwrap();
        assert_eq!(
            event,
            SkillWatcherEvent::SkillsAdded {
                skills: vec![skill]
            }
        );
    });
}

#[test]
fn test_handle_repository_update_skill_modified() {
    let (tx, rx) = async_channel::unbounded();

    App::test((), |mut app| async move {
        app.add_singleton_model(DirectoryWatcher::new_for_testing);
        app.add_singleton_model(|_| DetectedRepositories::default());
        app.add_singleton_model(RepoMetadataModel::new);
        let skill_watcher_handle = app.add_model(|ctx| SkillWatcher::new_for_testing(ctx, tx));

        let temp_dir = TempDir::new().unwrap();
        let skill = create_skill_file(&temp_dir, "test", "Test skill", "Test content");

        let update = RepositoryUpdate {
            added: HashSet::new(),
            modified: HashSet::from([TargetFile::new(
                skill.path.to_local_path().unwrap().to_path_buf(),
                false,
            )]),
            deleted: HashSet::new(),
            moved: HashMap::new(),
            commit_updated: false,
            index_lock_detected: false,
            remote_ref_updated: false,
        };

        skill_watcher_handle.update(&mut app, |skill_watcher, ctx| {
            skill_watcher.handle_repository_update(&update, ctx);
        });

        let event = rx.recv().await.unwrap();
        assert_eq!(
            event,
            SkillWatcherEvent::SkillsAdded {
                skills: vec![skill]
            }
        );
    });
}

#[test]
fn test_handle_repository_update_skill_deleted() {
    let (tx, rx) = async_channel::unbounded();

    App::test((), |mut app| async move {
        app.add_singleton_model(DirectoryWatcher::new_for_testing);
        app.add_singleton_model(|_| DetectedRepositories::default());
        app.add_singleton_model(RepoMetadataModel::new);
        let skill_watcher_handle = app.add_model(|ctx| SkillWatcher::new_for_testing(ctx, tx));

        let temp_dir = TempDir::new().unwrap();
        let skill = create_skill_file(&temp_dir, "test", "Test skill", "Test content");

        let update = RepositoryUpdate {
            added: HashSet::new(),
            modified: HashSet::new(),
            deleted: HashSet::from([TargetFile::new(
                skill.path.to_local_path().unwrap().to_path_buf(),
                false,
            )]),
            moved: HashMap::new(),
            commit_updated: false,
            index_lock_detected: false,
            remote_ref_updated: false,
        };

        skill_watcher_handle.update(&mut app, |skill_watcher, ctx| {
            skill_watcher.handle_repository_update(&update, ctx);
        });

        let event = rx.recv().await.unwrap();
        assert_eq!(
            event,
            SkillWatcherEvent::SkillsDeleted {
                paths: vec![skill.path]
            }
        );
    });
}

#[test]
fn test_handle_repository_update_multiple_skills_deleted() {
    let (tx, rx) = async_channel::unbounded();

    App::test((), |mut app| async move {
        app.add_singleton_model(DirectoryWatcher::new_for_testing);
        app.add_singleton_model(|_| DetectedRepositories::default());
        app.add_singleton_model(RepoMetadataModel::new);
        let skill_watcher_handle = app.add_model(|ctx| SkillWatcher::new_for_testing(ctx, tx));

        let temp_dir = TempDir::new().unwrap();
        let skill_a = create_skill_file(&temp_dir, "skill-a", "Skill A", "Content A");
        let skill_b = create_skill_file(&temp_dir, "skill-b", "Skill B", "Content B");

        let update = RepositoryUpdate {
            added: HashSet::new(),
            modified: HashSet::new(),
            deleted: HashSet::from([
                TargetFile::new(skill_a.path.to_local_path().unwrap().to_path_buf(), false),
                TargetFile::new(skill_b.path.to_local_path().unwrap().to_path_buf(), false),
            ]),
            moved: HashMap::new(),
            commit_updated: false,
            index_lock_detected: false,
            remote_ref_updated: false,
        };

        skill_watcher_handle.update(&mut app, |skill_watcher, ctx| {
            skill_watcher.handle_repository_update(&update, ctx);
        });

        let event = rx.recv().await.unwrap();
        let SkillWatcherEvent::SkillsDeleted { mut paths } = event else {
            panic!("Expected SkillsDeleted event");
        };
        paths.sort_by_key(LocalOrRemotePath::display_path);
        let mut expected = vec![skill_a.path, skill_b.path];
        expected.sort_by_key(LocalOrRemotePath::display_path);
        assert_eq!(paths, expected);
    });
}

#[test]
fn test_handle_repository_update_skill_moved() {
    let (tx, rx) = async_channel::unbounded();

    App::test((), |mut app| async move {
        app.add_singleton_model(DirectoryWatcher::new_for_testing);
        app.add_singleton_model(|_| DetectedRepositories::default());
        app.add_singleton_model(RepoMetadataModel::new);
        let skill_watcher_handle = app.add_model(|ctx| SkillWatcher::new_for_testing(ctx, tx));

        let temp_dir = TempDir::new().unwrap();
        let old_skill = create_skill_file(&temp_dir, "old-skill", "Old skill", "Old content");
        let new_skill = create_skill_file(&temp_dir, "new-skill", "New skill", "New content");

        // moved is HashMap<to_target, from_target>
        let update = RepositoryUpdate {
            added: HashSet::new(),
            modified: HashSet::new(),
            deleted: HashSet::new(),
            moved: HashMap::from([(
                TargetFile::new(new_skill.path.to_local_path().unwrap().to_path_buf(), false),
                TargetFile::new(old_skill.path.to_local_path().unwrap().to_path_buf(), false),
            )]),
            commit_updated: false,
            index_lock_detected: false,
            remote_ref_updated: false,
        };

        skill_watcher_handle.update(&mut app, |skill_watcher, ctx| {
            skill_watcher.handle_repository_update(&update, ctx);
        });

        // Collect both events: SkillsAdded for the new location and SkillsDeleted for the old
        let event1 = rx.recv().await.unwrap();
        let event2 = rx.recv().await.unwrap();

        let added_event = SkillWatcherEvent::SkillsAdded {
            skills: vec![new_skill],
        };
        let deleted_event = SkillWatcherEvent::SkillsDeleted {
            paths: vec![old_skill.path],
        };
        assert!(
            (event1 == added_event && event2 == deleted_event)
                || (event1 == deleted_event && event2 == added_event),
            "Expected one SkillsAdded and one SkillsDeleted event; got: {event1:?} and {event2:?}"
        );
    });
}

// ============================================================================
// Tests for handle_repository_update - directory addition
// ============================================================================

/// When a non-skill directory is added within a known repo, `handle_repository_update` should
/// queue the repo root in `queued_project_directory_creations` for a later skill scan.
#[test]
fn test_handle_repository_update_non_skill_directory_added_queues_project_directory() {
    App::test((), |mut app| async move {
        app.add_singleton_model(DirectoryWatcher::new_for_testing);
        let detected_repos_handle = app.add_singleton_model(|_| DetectedRepositories::default());
        app.add_singleton_model(RepoMetadataModel::new);
        let (tx, _rx) = async_channel::unbounded();
        let skill_watcher_handle = app.add_model(|ctx| SkillWatcher::new_for_testing(ctx, tx));

        let temp_dir = TempDir::new().unwrap();
        let canonical_repo = StandardizedPath::from_local_canonicalized(temp_dir.path()).unwrap();

        // Register the temp dir as a known repo root so get_root_for_path resolves it.
        detected_repos_handle.update(&mut app, |repos, _| {
            repos.insert_test_repo_root(canonical_repo.clone());
        });

        // Seed watched_repos so get_watched_repo_path can resolve the temp dir to this root.
        // Use the canonicalized path to match what CanonicalizedPath::try_from resolves on macOS
        // (where /var is a symlink to /private/var).
        skill_watcher_handle.update(&mut app, |watcher, _| {
            watcher
                .watched_repos
                .insert(canonical_repo.to_local_path().unwrap());
        });

        // The added path must exist on disk for CanonicalizedPath resolution.
        let new_dir = canonical_repo.to_local_path().unwrap().join("new-feature");
        fs::create_dir_all(&new_dir).unwrap();

        let update = RepositoryUpdate {
            added: HashSet::from([TargetFile::new(new_dir, false)]),
            modified: HashSet::new(),
            deleted: HashSet::new(),
            moved: HashMap::new(),
            commit_updated: false,
            index_lock_detected: false,
            remote_ref_updated: false,
        };

        skill_watcher_handle.update(&mut app, |skill_watcher, ctx| {
            skill_watcher.handle_repository_update(&update, ctx);
        });

        // The repo root should be queued for a skill scan.
        skill_watcher_handle.read(&app, |watcher, _| {
            assert_eq!(watcher.queued_project_directory_creations.len(), 1);
            assert_eq!(
                watcher.queued_project_directory_creations[0].path,
                canonical_repo.to_local_path().unwrap()
            );
        });
    });
}

/// A modified non-skill file in a known repo should NOT queue anything in
/// `queued_project_directory_creations`; only directory additions can introduce new skill files.
#[test]
fn test_handle_repository_update_non_skill_file_modified_in_repo_does_not_queue_project_directory()
{
    App::test((), |mut app| async move {
        app.add_singleton_model(DirectoryWatcher::new_for_testing);
        let detected_repos_handle = app.add_singleton_model(|_| DetectedRepositories::default());
        app.add_singleton_model(RepoMetadataModel::new);
        let (tx, _rx) = async_channel::unbounded();
        let skill_watcher_handle = app.add_model(|ctx| SkillWatcher::new_for_testing(ctx, tx));

        let temp_dir = TempDir::new().unwrap();
        let canonical_repo = StandardizedPath::from_local_canonicalized(temp_dir.path()).unwrap();

        detected_repos_handle.update(&mut app, |repos, _| {
            repos.insert_test_repo_root(canonical_repo.clone());
        });

        // Create the file on disk so CanonicalizedPath resolution succeeds.
        let readme = temp_dir.path().join("README.md");
        fs::write(&readme, "# Project").unwrap();

        let update = RepositoryUpdate {
            added: HashSet::new(),
            modified: HashSet::from([TargetFile::new(readme, false)]),
            deleted: HashSet::new(),
            moved: HashMap::new(),
            commit_updated: false,
            index_lock_detected: false,
            remote_ref_updated: false,
        };

        skill_watcher_handle.update(&mut app, |skill_watcher, ctx| {
            skill_watcher.handle_repository_update(&update, ctx);
        });

        // Modifying a plain file must NOT queue a project directory scan.
        skill_watcher_handle.read(&app, |watcher, _| {
            assert_eq!(watcher.queued_project_directory_creations.len(), 0);
        });
    });
}

/// When a regular (non-skill) file is added within a known repo, `handle_repository_update`
/// should NOT queue anything in `queued_project_directory_creations` because only directory
/// additions may introduce new skill files.
#[test]
fn test_handle_repository_update_non_skill_file_added_does_not_queue_project_directory() {
    App::test((), |mut app| async move {
        app.add_singleton_model(DirectoryWatcher::new_for_testing);
        let detected_repos_handle = app.add_singleton_model(|_| DetectedRepositories::default());
        app.add_singleton_model(RepoMetadataModel::new);
        let (tx, _rx) = async_channel::unbounded();
        let skill_watcher_handle = app.add_model(|ctx| SkillWatcher::new_for_testing(ctx, tx));

        let temp_dir = TempDir::new().unwrap();
        let canonical_repo = StandardizedPath::from_local_canonicalized(temp_dir.path()).unwrap();

        detected_repos_handle.update(&mut app, |repos, _| {
            repos.insert_test_repo_root(canonical_repo.clone());
        });

        // Create a regular file (not a directory, not a skill file) on disk.
        let readme = temp_dir.path().join("README.md");
        fs::write(&readme, "# Project").unwrap();

        let update = RepositoryUpdate {
            added: HashSet::from([TargetFile::new(readme, false)]),
            modified: HashSet::new(),
            deleted: HashSet::new(),
            moved: HashMap::new(),
            commit_updated: false,
            index_lock_detected: false,
            remote_ref_updated: false,
        };

        skill_watcher_handle.update(&mut app, |skill_watcher, ctx| {
            skill_watcher.handle_repository_update(&update, ctx);
        });

        // A plain file being added must NOT queue a project directory scan.
        skill_watcher_handle.read(&app, |watcher, _| {
            assert_eq!(watcher.queued_project_directory_creations.len(), 0);
        });
    });
}

// ============================================================================
// Tests for handle_queued_project_directory_creations
// ============================================================================
