use ai::project_context::model::{ProjectContextModel, ProjectRule};
use remote_server::proto::{file_context_proto, ReadFileContextFile, ReadFileContextRequest};
use repo_metadata::{
    local_model::GetContentsArgs, RepoContent, RepoMetadataModel, RepositoryIdentifier,
};
use warp_util::{local_or_remote_path::LocalOrRemotePath, remote_path::RemotePath};
use warpui::{AppContext, Entity, ModelContext, SingletonEntity};

use crate::remote_server::manager::RemoteServerManager;

pub(crate) struct RemoteProjectRulesModel;

impl RemoteProjectRulesModel {
    pub(crate) fn new(ctx: &mut ModelContext<Self>) -> Self {
        let repo_metadata = RepoMetadataModel::handle(ctx);
        ctx.subscribe_to_model(&repo_metadata, |me, event, ctx| {
            me.handle_repo_metadata_event(event, ctx);
        });

        let remote_repo_ids = RepoMetadataModel::as_ref(ctx)
            .remote_repository_ids(ctx)
            .cloned()
            .map(RepositoryIdentifier::Remote)
            .collect::<Vec<_>>();
        let mut model = Self;
        for remote_repo_id in remote_repo_ids {
            model.refresh_remote_repository(remote_repo_id, ctx);
        }
        model
    }

    fn handle_repo_metadata_event(
        &mut self,
        event: &repo_metadata::wrapper_model::RepoMetadataEvent,
        ctx: &mut ModelContext<Self>,
    ) {
        use repo_metadata::wrapper_model::RepoMetadataEvent;

        match event {
            RepoMetadataEvent::RepositoryUpdated {
                id: remote_id @ RepositoryIdentifier::Remote(_),
            }
            | RepoMetadataEvent::FileTreeEntryUpdated {
                id: remote_id @ RepositoryIdentifier::Remote(_),
            } => {
                self.refresh_remote_repository(remote_id.clone(), ctx);
            }
            RepoMetadataEvent::FileTreeUpdated { ids } => {
                for remote_id in ids.iter().filter_map(|id| match id {
                    RepositoryIdentifier::Remote(_) => Some(id.clone()),
                    RepositoryIdentifier::Local(_) => None,
                }) {
                    self.refresh_remote_repository(remote_id, ctx);
                }
            }
            RepoMetadataEvent::RepositoryRemoved {
                id: remote_id @ RepositoryIdentifier::Remote(_),
            } => {
                if let Some(root_path) = remote_id.to_local_or_remote_path() {
                    ProjectContextModel::handle(ctx).update(ctx, |model, ctx| {
                        model.replace_rules_for_remote_root(root_path, Vec::new(), ctx);
                    });
                }
            }
            RepoMetadataEvent::RepositoryUpdated {
                id: RepositoryIdentifier::Local(_),
            }
            | RepoMetadataEvent::RepositoryRemoved {
                id: RepositoryIdentifier::Local(_),
            }
            | RepoMetadataEvent::FileTreeEntryUpdated {
                id: RepositoryIdentifier::Local(_),
            }
            | RepoMetadataEvent::UpdatingRepositoryFailed { .. }
            | RepoMetadataEvent::IncrementalUpdateReady { .. } => {}
        }
    }

    fn refresh_remote_repository(
        &mut self,
        repo_id: RepositoryIdentifier,
        ctx: &mut ModelContext<Self>,
    ) {
        let Some(root_path) = repo_id.to_local_or_remote_path() else {
            return;
        };
        let rule_paths =
            find_remote_project_rule_files_in_tree(&repo_id, RepoMetadataModel::as_ref(ctx), ctx);

        if rule_paths.is_empty() {
            ProjectContextModel::handle(ctx).update(ctx, |model, ctx| {
                model.replace_rules_for_remote_root(root_path, Vec::new(), ctx);
            });
            return;
        }

        let RepositoryIdentifier::Remote(remote_root) = repo_id else {
            return;
        };
        let Some(client) = RemoteServerManager::as_ref(ctx)
            .client_for_host(&remote_root.host_id)
            .cloned()
        else {
            return;
        };

        ctx.spawn(
            async move {
                let request = ReadFileContextRequest {
                    files: rule_paths
                        .iter()
                        .filter_map(|path| match path {
                            LocalOrRemotePath::Remote(remote) => Some(ReadFileContextFile {
                                path: remote.path.as_str().to_string(),
                                line_ranges: Vec::new(),
                            }),
                            LocalOrRemotePath::Local(_) => None,
                        })
                        .collect(),
                    max_file_bytes: None,
                    max_batch_bytes: None,
                };
                let response = client.read_file_context(request).await?;
                let rules = rule_paths
                    .into_iter()
                    .zip(response.file_contexts)
                    .filter_map(|(path, file_context)| {
                        let file_context_proto::Content::TextContent(content) =
                            file_context.content?
                        else {
                            return None;
                        };
                        Some(ProjectRule { path, content })
                    })
                    .collect();
                Ok::<(LocalOrRemotePath, Vec<ProjectRule>), anyhow::Error>((root_path, rules))
            },
            |_, hydrated_rules, ctx| match hydrated_rules {
                Ok((root_path, rules)) => {
                    ProjectContextModel::handle(ctx).update(ctx, |model, ctx| {
                        model.replace_rules_for_remote_root(root_path, rules, ctx);
                    });
                }
                Err(err) => log::warn!("Failed to read remote project rules: {err}"),
            },
        );
    }
}

impl Entity for RemoteProjectRulesModel {
    type Event = ();
}

impl SingletonEntity for RemoteProjectRulesModel {}

fn find_remote_project_rule_files_in_tree(
    repo_id: &RepositoryIdentifier,
    repo_metadata: &RepoMetadataModel,
    ctx: &AppContext,
) -> Vec<LocalOrRemotePath> {
    let RepositoryIdentifier::Remote(remote_root) = repo_id else {
        return Vec::new();
    };
    let remote_root = remote_root.clone();
    let args = GetContentsArgs::default().with_filter(move |content| {
        let RepoContent::File(file) = content else {
            return false;
        };
        matches_remote_rule_file(file.path.file_name())
    });

    repo_metadata
        .get_repo_contents(repo_id, args, ctx)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|content| {
            let RepoContent::File(file) = content else {
                return None;
            };
            Some(LocalOrRemotePath::Remote(RemotePath::new(
                remote_root.host_id.clone(),
                file.path.as_ref().clone(),
            )))
        })
        .collect()
}

fn matches_remote_rule_file(file_name: Option<&str>) -> bool {
    file_name.is_some_and(|file_name| {
        file_name.eq_ignore_ascii_case("WARP.md") || file_name.eq_ignore_ascii_case("AGENTS.md")
    })
}
