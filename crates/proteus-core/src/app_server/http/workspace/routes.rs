//! HTTP endpoints `/workspace/*` of an explicitly addressed live session.
use std::path::Path;

use anyhow::{Context, Result};
use bytes::Bytes;
use hyper::{Request, StatusCode, body::Body};
use serde::{Serialize, de::DeserializeOwned};

use super::{
    super::{
        HttpResponse, read_json,
        responses::{error_response, json_response},
        sessions::{server_for_query, server_for_session},
        state::HttpAppState,
    },
    files, git, query_param, query_params, query_path,
    requests::{
        Addressed, CheckoutRequest, CommitRequest, CreateRequest, DeleteRequest, IgnoreRequest,
        PathsRequest, PullRequest, PushRequest, RenameRequest, SessionRequest, StashRequest,
    },
    search,
};

pub(crate) async fn get(
    state: &HttpAppState,
    path: &str,
    query: Option<&str>,
) -> Result<HttpResponse> {
    let value = match path {
        "/workspace/list" => {
            let relative = query_path(query)?;
            in_session_root(server_for_query(state, query).await?, move |root| {
                super::list(root, relative)
            })
            .await?
        }
        "/workspace/file" => {
            let relative = query_path(query)?;
            in_session_root(server_for_query(state, query).await?, move |root| {
                super::read(root, relative)
            })
            .await?
        }
        "/workspace/search" => {
            let text = search::query_text(query)?;
            in_session_root(server_for_query(state, query).await?, move |root| {
                search::search(root, text)
            })
            .await?
        }
        "/workspace/diff" => {
            let relative = query_path(query)?;
            in_session_root(server_for_query(state, query).await?, move |root| {
                git::diff(root, relative)
            })
            .await?
        }
        "/workspace/changes" | "/workspace/branches" => {
            query_param(query, None)?;
            let server = server_for_query(state, query).await?;
            if path == "/workspace/changes" {
                in_session_root(server, git::changes).await?
            } else {
                in_session_root(server, git::branches).await?
            }
        }
        "/workspace/log" => {
            let log = log_query(query)?;
            in_session_root(server_for_query(state, query).await?, move |root| {
                git::log(root, log)
            })
            .await?
        }
        "/workspace/commit" => {
            let [hash] = query_params(query, ["hash"])?;
            let hash = hash.context("hash is required")?;
            in_session_root(server_for_query(state, query).await?, move |root| {
                git::commit_details(root, &hash)
            })
            .await?
        }
        "/workspace/commit/diff" => {
            let [hash, path] = query_params(query, ["hash", "path"])?;
            let hash = hash.context("hash is required")?;
            let path = path.context("path is required")?;
            in_session_root(server_for_query(state, query).await?, move |root| {
                git::commit_diff(root, &hash, path)
            })
            .await?
        }
        _ => {
            return Ok(error_response(
                StatusCode::NOT_FOUND,
                "unknown app-server HTTP endpoint",
            ));
        }
    };
    Ok(json_response(StatusCode::OK, &value))
}

fn log_query(query: Option<&str>) -> Result<git::LogQuery> {
    let [limit, skip, all] = query_params(query, ["limit", "skip", "all"])?;
    Ok(git::LogQuery {
        limit: match limit {
            Some(limit) => limit.parse().context("log limit must be an integer")?,
            None => git::DEFAULT_LOG_ENTRIES,
        },
        skip: match skip {
            Some(skip) => skip.parse().context("log skip must be an integer")?,
            None => 0,
        },
        all: match all.as_deref() {
            None | Some("false") => false,
            Some("true") => true,
            Some(_) => anyhow::bail!("log all must be true or false"),
        },
    })
}

pub(crate) async fn post<B>(state: &HttpAppState, path: &str, request: Request<B>) -> HttpResponse
where
    B: Body<Data = Bytes> + Send + 'static,
    B::Error: std::error::Error + Send + Sync + 'static,
{
    let result = match path {
        "/workspace/stage" | "/workspace/unstage" | "/workspace/discard" => {
            let action = match path {
                "/workspace/stage" => git::stage,
                "/workspace/unstage" => git::unstage,
                _ => git::discard,
            };
            act(state, request, move |root, body: PathsRequest| {
                action(root, git::Selection::new(body.paths, body.all)?)
            })
            .await
        }
        "/workspace/commit" => {
            act(state, request, |root, body: CommitRequest| {
                let options = git::CommitOptions {
                    amend: body.amend,
                    signoff: body.signoff,
                    no_verify: body.no_verify,
                    all_tracked: body.all_tracked,
                };
                git::commit(root, &body.message, options)
            })
            .await
        }
        "/workspace/checkout" => {
            act(state, request, |root, body: CheckoutRequest| {
                git::checkout(root, &body.branch, body.create)
            })
            .await
        }
        "/workspace/fetch" => act(state, request, |root, _: SessionRequest| git::fetch(root)).await,
        "/workspace/pull" => {
            act(state, request, |root, body: PullRequest| {
                git::pull(root, body.rebase)
            })
            .await
        }
        "/workspace/push" => {
            act(state, request, |root, body: PushRequest| {
                git::push(root, body.force, body.set_upstream)
            })
            .await
        }
        "/workspace/uncommit" => {
            act(state, request, |root, _: SessionRequest| {
                git::uncommit(root)
            })
            .await
        }
        "/workspace/stash" => {
            act(state, request, |root, body: StashRequest| {
                git::stash(root, body.message.as_deref(), body.staged_only)
            })
            .await
        }
        "/workspace/stash/pop" => {
            act(state, request, |root, _: SessionRequest| {
                git::stash_pop(root)
            })
            .await
        }
        "/workspace/ignore" => {
            act(state, request, |root, body: IgnoreRequest| {
                git::ignore(root, &body.paths, body.target)
            })
            .await
        }
        "/workspace/create" => {
            act(state, request, |root, body: CreateRequest| {
                files::create(root, &body.path, body.kind)
            })
            .await
        }
        "/workspace/rename" => {
            act(state, request, |root, body: RenameRequest| {
                files::rename(root, &body.from, &body.to)
            })
            .await
        }
        "/workspace/delete" => {
            act(state, request, |root, body: DeleteRequest| {
                files::delete(root, &body.paths)
            })
            .await
        }
        _ => return error_response(StatusCode::NOT_FOUND, "unknown app-server HTTP endpoint"),
    };
    match result {
        Ok(value) => json_response(StatusCode::OK, &value),
        Err(error) => error_response(StatusCode::BAD_REQUEST, &format!("{error:#}")),
    }
}

/// Parses a strict body, resolves its live session and runs `work` in its cwd.
async fn act<R, T, B, F>(
    state: &HttpAppState,
    request: Request<B>,
    work: F,
) -> Result<serde_json::Value>
where
    R: DeserializeOwned + Addressed + Send + 'static,
    T: Serialize,
    B: Body<Data = Bytes> + Send + 'static,
    B::Error: std::error::Error + Send + Sync + 'static,
    F: FnOnce(&Path, R) -> Result<T> + Send + 'static,
{
    let body = read_json::<R, _>(request).await?;
    let server = server_for_session(state, body.session_dir()).await?;
    in_session_root(server, move |root| work(root, body)).await
}

/// Runs blocking filesystem/Git work against the live session cwd.
async fn in_session_root<T, F>(
    server: crate::app_server::AppServerHandle,
    work: F,
) -> Result<serde_json::Value>
where
    T: Serialize,
    F: FnOnce(&Path) -> Result<T> + Send + 'static,
{
    let root = server.cwd_path().to_path_buf();
    let value = tokio::task::spawn_blocking(move || -> Result<serde_json::Value> {
        Ok(serde_json::to_value(work(&root)?)?)
    })
    .await??;
    Ok(value)
}
