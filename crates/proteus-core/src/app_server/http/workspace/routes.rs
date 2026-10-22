//! HTTP endpoints `/workspace/*` of an explicitly addressed live session.
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use bytes::Bytes;
use hyper::{Request, StatusCode, body::Body};
use serde::{Deserialize, Serialize};

use super::{
    super::{
        HttpResponse, read_json,
        responses::{error_response, json_response},
        sessions::{server_for_query, server_for_session},
        state::HttpAppState,
    },
    git, query_param, query_path, search,
};

const DEFAULT_LOG_ENTRIES: usize = 20;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PathsRequest {
    session_dir: PathBuf,
    #[serde(default)]
    paths: Vec<String>,
    #[serde(default)]
    all: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CommitRequest {
    session_dir: PathBuf,
    message: String,
    #[serde(default)]
    amend: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CheckoutRequest {
    session_dir: PathBuf,
    branch: String,
}

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
            let limit = match query_param(query, Some("limit"))? {
                Some(limit) => limit.parse().context("log limit must be an integer")?,
                None => DEFAULT_LOG_ENTRIES,
            };
            in_session_root(server_for_query(state, query).await?, move |root| {
                git::log(root, limit)
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

pub(crate) async fn post<B>(state: &HttpAppState, path: &str, request: Request<B>) -> HttpResponse
where
    B: Body<Data = Bytes> + Send + 'static,
    B::Error: std::error::Error + Send + Sync + 'static,
{
    let result = match path {
        "/workspace/stage" | "/workspace/unstage" | "/workspace/discard" => {
            paths_action(state, path, request).await
        }
        "/workspace/commit" => commit(state, request).await,
        "/workspace/checkout" => checkout(state, request).await,
        _ => return error_response(StatusCode::NOT_FOUND, "unknown app-server HTTP endpoint"),
    };
    match result {
        Ok(value) => json_response(StatusCode::OK, &value),
        Err(error) => error_response(StatusCode::BAD_REQUEST, &format!("{error:#}")),
    }
}

async fn paths_action<B>(
    state: &HttpAppState,
    path: &str,
    request: Request<B>,
) -> Result<serde_json::Value>
where
    B: Body<Data = Bytes> + Send + 'static,
    B::Error: std::error::Error + Send + Sync + 'static,
{
    let command = read_json::<PathsRequest, _>(request).await?;
    let selection = git::Selection::new(command.paths, command.all)?;
    let server = server_for_session(state, command.session_dir).await?;
    let action = match path {
        "/workspace/stage" => git::stage,
        "/workspace/unstage" => git::unstage,
        _ => git::discard,
    };
    in_session_root(server, move |root| action(root, selection)).await
}

async fn commit<B>(state: &HttpAppState, request: Request<B>) -> Result<serde_json::Value>
where
    B: Body<Data = Bytes> + Send + 'static,
    B::Error: std::error::Error + Send + Sync + 'static,
{
    let command = read_json::<CommitRequest, _>(request).await?;
    let server = server_for_session(state, command.session_dir).await?;
    in_session_root(server, move |root| {
        git::commit(root, &command.message, command.amend)
    })
    .await
}

async fn checkout<B>(state: &HttpAppState, request: Request<B>) -> Result<serde_json::Value>
where
    B: Body<Data = Bytes> + Send + 'static,
    B::Error: std::error::Error + Send + Sync + 'static,
{
    let command = read_json::<CheckoutRequest, _>(request).await?;
    let server = server_for_session(state, command.session_dir).await?;
    in_session_root(server, move |root| git::checkout(root, &command.branch)).await
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
