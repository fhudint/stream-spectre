//! The dashboard's JSON API. Every route here needs the token when one is set
//! (see `guard`). Errors are `{"error": "..."}` with a fitting status, and
//! sometimes a `"code"` for programs to tell them apart.

use axum::Json;
use std::net::{IpAddr, SocketAddr};

use axum::extract::{ConnectInfo, Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use super::ServerState;
use crate::app::{AccountView, CredentialsError, Hello, PauseOutcome};
use crate::config::{
    BlacklistConfig, CommandConfig, DashboardConfig, ModerationConfig, TiktokConfig,
    YoutubeChatConfig,
};
use crate::i18n::catalog_json;
use crate::queue::{ItemId, QueueError};
use crate::youtube::VideoSource;

#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    message: String,
    code: Option<&'static str>,
}

impl ApiError {
    fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
            code: None,
        }
    }

    fn coded(mut self, code: &'static str) -> Self {
        self.code = Some(code);
        self
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut body = json!({ "error": self.message });
        if let Some(code) = self.code {
            body["code"] = json!(code);
        }
        (self.status, Json(body)).into_response()
    }
}

impl From<CredentialsError> for ApiError {
    fn from(error: CredentialsError) -> Self {
        let status = match error {
            CredentialsError::Empty | CredentialsError::BadFormat | CredentialsError::Rejected => {
                StatusCode::BAD_REQUEST
            }
            CredentialsError::CannotCheck(_) => StatusCode::BAD_GATEWAY,
            CredentialsError::NotSaved(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        // The message never holds the key: see `CredentialsError`.
        Self::new(status, error.to_string()).coded(error.code())
    }
}

impl From<QueueError> for ApiError {
    fn from(error: QueueError) -> Self {
        let status = match error {
            QueueError::NotFound(_) => StatusCode::NOT_FOUND,
            QueueError::NotPending(_) | QueueError::NotPlaying(_) => StatusCode::CONFLICT,
        };
        Self::new(status, error.to_string())
    }
}

type ApiResult<T> = Result<T, ApiError>;

/// `GET /api/state`: the same snapshot a WebSocket client gets as `hello`.
pub async fn state<S: VideoSource + 'static>(State(state): State<ServerState<S>>) -> Json<Hello> {
    Json(state.pipeline.hello())
}

/// `GET /api/i18n`: the UI texts in the configured language.
pub async fn catalog<S: VideoSource + 'static>(State(state): State<ServerState<S>>) -> Response {
    (
        [(header::CONTENT_TYPE, "application/json")],
        catalog_json(state.pipeline.hello().language),
    )
        .into_response()
}

/// `POST /api/queue/{id}/approve`
pub async fn approve<S: VideoSource + 'static>(
    State(state): State<ServerState<S>>,
    Path(id): Path<u64>,
) -> ApiResult<StatusCode> {
    state.pipeline.approve(ItemId(id))?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/queue/{id}/reject`
pub async fn reject<S: VideoSource + 'static>(
    State(state): State<ServerState<S>>,
    Path(id): Path<u64>,
) -> ApiResult<StatusCode> {
    state.pipeline.reject(ItemId(id))?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/skip`: `{"skipped": true}` when something was playing.
pub async fn skip<S: VideoSource + 'static>(State(state): State<ServerState<S>>) -> Json<Value> {
    Json(json!({ "skipped": state.pipeline.skip() }))
}

/// `POST /api/volume` with `{"volume": 0-100}`; answers with the volume that
/// is in effect after limiting it to the configured maximum.
pub async fn volume<S: VideoSource + 'static>(
    State(state): State<ServerState<S>>,
    Json(body): Json<Value>,
) -> ApiResult<Json<Value>> {
    let requested = body
        .get("volume")
        .and_then(Value::as_u64)
        .and_then(|volume| u8::try_from(volume).ok())
        .filter(|volume| *volume <= 100)
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::BAD_REQUEST,
                "body must be {\"volume\": a whole number from 0 to 100}",
            )
        })?;
    Ok(Json(
        json!({ "volume": state.pipeline.set_volume(requested) }),
    ))
}

/// `PATCH /api/settings`: changes the given moderation settings and keeps the
/// rest. Unknown or invalid values are refused and nothing changes.
pub async fn settings<S: VideoSource + 'static>(
    State(state): State<ServerState<S>>,
    Json(patch): Json<Value>,
) -> ApiResult<Json<Hello>> {
    let current = state.pipeline.hello().settings.moderation;
    let updated: ModerationConfig = merge_patch(&current, patch)?;
    state
        .pipeline
        .update_settings(updated)
        .map_err(|error| ApiError::new(StatusCode::BAD_REQUEST, error.to_string()))?;
    Ok(Json(state.pipeline.hello()))
}

/// `PATCH /api/blacklist`: replaces the given lists (`video_ids`, `channels`,
/// `title_words`) and keeps the others.
pub async fn blacklist<S: VideoSource + 'static>(
    State(state): State<ServerState<S>>,
    Json(patch): Json<Value>,
) -> ApiResult<Json<Hello>> {
    let current = state.pipeline.hello().settings.blacklist;
    let updated: BlacklistConfig = merge_patch(&current, patch)?;
    state
        .pipeline
        .update_blacklist(updated)
        .map_err(|error| ApiError::new(StatusCode::BAD_REQUEST, error.to_string()))?;
    Ok(Json(state.pipeline.hello()))
}

/// `PATCH /api/dashboard`: changes the given dashboard settings (for now only
/// `chat_history`) and keeps the rest.
pub async fn dashboard<S: VideoSource + 'static>(
    State(state): State<ServerState<S>>,
    Json(patch): Json<Value>,
) -> ApiResult<Json<Hello>> {
    let current = state.pipeline.hello().settings.dashboard;
    let updated: DashboardConfig = merge_patch(&current, patch)?;
    state
        .pipeline
        .update_dashboard(updated)
        .map_err(|error| ApiError::new(StatusCode::BAD_REQUEST, error.to_string()))?;
    Ok(Json(state.pipeline.hello()))
}

/// `PATCH /api/overlay/mediashare`: changes the mediashare overlay's `theme`.
/// An unknown theme is refused and nothing changes.
pub async fn mediashare<S: VideoSource + 'static>(
    State(state): State<ServerState<S>>,
    Json(patch): Json<Value>,
) -> ApiResult<Json<Hello>> {
    let current = state.pipeline.hello().settings.overlay.mediashare;
    state
        .pipeline
        .update_mediashare(merge_patch(&current, patch)?);
    Ok(Json(state.pipeline.hello()))
}

/// `PATCH /api/overlay/chat`: changes the chat overlay's `theme`. An unknown
/// theme is refused and nothing changes.
pub async fn chat_overlay<S: VideoSource + 'static>(
    State(state): State<ServerState<S>>,
    Json(patch): Json<Value>,
) -> ApiResult<Json<Hello>> {
    let current = state.pipeline.hello().settings.overlay.chat;
    state
        .pipeline
        .update_chat_overlay(merge_patch(&current, patch)?);
    Ok(Json(state.pipeline.hello()))
}

/// `PATCH /api/commands`: changes the chat commands (`prefix` and/or
/// `commands`, the whole list of `{name, source}`) and keeps the rest. Invalid
/// values are refused and nothing changes.
pub async fn commands<S: VideoSource + 'static>(
    State(state): State<ServerState<S>>,
    Json(patch): Json<Value>,
) -> ApiResult<Json<Hello>> {
    let current = state.pipeline.hello().settings.command;
    let updated: CommandConfig = merge_patch(&current, patch)?;
    state
        .pipeline
        .update_commands(updated)
        .map_err(|error| ApiError::new(StatusCode::BAD_REQUEST, error.to_string()))?;
    Ok(Json(state.pipeline.hello()))
}

/// `PUT /api/chat/connection` with `{"connected": true|false}`: disconnects
/// the chat or connects it again, for as long as the program runs.
pub async fn chat_connection<S: VideoSource + 'static>(
    State(state): State<ServerState<S>>,
    Json(body): Json<Value>,
) -> ApiResult<Json<Hello>> {
    let Some(connected) = body.get("connected").and_then(Value::as_bool) else {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "the body must be {\"connected\": true or false}",
        )
        .coded("bad_request"));
    };
    state.pipeline.set_chat_paused(!connected);
    Ok(Json(state.pipeline.hello()))
}

/// `PUT /api/playback/pause` with `{"paused": true|false}`: pauses or resumes
/// the video that is playing, YouTube or TikTok. `409` when nothing is playing.
pub async fn pause_playback<S: VideoSource + 'static>(
    State(state): State<ServerState<S>>,
    Json(body): Json<Value>,
) -> ApiResult<Json<Hello>> {
    let Some(paused) = body.get("paused").and_then(Value::as_bool) else {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "the body must be {\"paused\": true or false}",
        )
        .coded("bad_request"));
    };
    match state.pipeline.set_playback_paused(paused) {
        PauseOutcome::Done => Ok(Json(state.pipeline.hello())),
        PauseOutcome::NothingPlaying => {
            Err(ApiError::new(StatusCode::CONFLICT, "no video is playing").coded("nothing_playing"))
        }
    }
}

/// Credentials can only be changed from the computer the program runs on,
/// even when the dashboard is opened to the network with a token: a moderator
/// who has the token can approve videos but not swap the YouTube key.
pub fn may_change_credentials(peer: IpAddr) -> bool {
    peer.to_canonical().is_loopback()
}

fn only_here(peer: SocketAddr) -> Result<(), ApiError> {
    if may_change_credentials(peer.ip()) {
        Ok(())
    } else {
        Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "credentials can only be changed from the computer the program runs on",
        )
        .coded("loopback_only"))
    }
}

fn account_body(view: &AccountView, peer: SocketAddr) -> Value {
    let mut body = serde_json::to_value(view).unwrap_or_else(|_| json!({}));
    body["editable"] = json!(may_change_credentials(peer.ip()));
    body
}

/// `GET /api/account`: what is set (never the secrets themselves), and
/// whether this connection may change it.
pub async fn account<S: VideoSource + 'static>(
    State(state): State<ServerState<S>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
) -> Json<Value> {
    Json(account_body(&state.credentials.status().await, peer))
}

/// `PUT /api/account/youtube-key` with `{"key": "..."}`: checks the key with
/// one YouTube request, saves it and starts using it.
pub async fn set_youtube_key<S: VideoSource + 'static>(
    State(state): State<ServerState<S>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Json(body): Json<Value>,
) -> ApiResult<Json<Value>> {
    only_here(peer)?;
    let Some(key) = body.get("key").and_then(Value::as_str) else {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "the body must be {\"key\": \"...\"}",
        )
        .coded("bad_request"));
    };
    let (checked, view) = state
        .credentials
        .set_youtube_key(&state.pipeline, key)
        .await?;
    let mut body = account_body(&view, peer);
    body["checked"] = json!(checked);
    Ok(Json(body))
}

/// `DELETE /api/account/youtube-key`: forgets the saved key and goes back to
/// the one in the environment, if there is one.
pub async fn remove_youtube_key<S: VideoSource + 'static>(
    State(state): State<ServerState<S>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
) -> ApiResult<Json<Value>> {
    only_here(peer)?;
    let view = state
        .credentials
        .remove_saved_youtube_key(&state.pipeline)
        .await?;
    Ok(Json(account_body(&view, peer)))
}

/// `PATCH /api/chat/tiktok`: changes the TikTok chat settings: the username
/// (`username`; empty stops reading chat) and `enabled`. The chat follows the
/// change without a restart.
pub async fn tiktok<S: VideoSource + 'static>(
    State(state): State<ServerState<S>>,
    Json(patch): Json<Value>,
) -> ApiResult<Json<Hello>> {
    let current = state.pipeline.hello().settings.chat.tiktok;
    let updated: TiktokConfig = merge_patch(&current, patch)?;
    state
        .pipeline
        .update_tiktok(updated)
        .map_err(|error| ApiError::new(StatusCode::BAD_REQUEST, error.to_string()))?;
    Ok(Json(state.pipeline.hello()))
}

/// `PATCH /api/chat/youtube`: changes the YouTube chat settings: the live
/// video (`video`: its link or ID; empty stops reading chat) and `enabled`.
/// The chat follows the change without a restart.
pub async fn youtube_chat<S: VideoSource + 'static>(
    State(state): State<ServerState<S>>,
    Json(patch): Json<Value>,
) -> ApiResult<Json<Hello>> {
    let current = state.pipeline.hello().settings.chat.youtube;
    let updated: YoutubeChatConfig = merge_patch(&current, patch)?;
    state
        .pipeline
        .update_youtube_chat(updated)
        .map_err(|error| ApiError::new(StatusCode::BAD_REQUEST, error.to_string()))?;
    Ok(Json(state.pipeline.hello()))
}

/// Applies a partial JSON object on top of `current` and parses the result.
/// Settings types reject unknown fields, so a misspelt key is an error rather
/// than silently ignored.
fn merge_patch<T>(current: &T, patch: Value) -> Result<T, ApiError>
where
    T: Serialize + DeserializeOwned,
{
    let bad = |message: String| ApiError::new(StatusCode::BAD_REQUEST, message);

    let Value::Object(patch) = patch else {
        return Err(bad("body must be a JSON object".to_owned()));
    };
    let mut merged = serde_json::to_value(current)
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "cannot read settings"))?;
    let Some(fields) = merged.as_object_mut() else {
        return Err(ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "settings are not an object",
        ));
    };
    for (key, value) in patch {
        fields.insert(key, value);
    }
    serde_json::from_value(merged).map_err(|error| bad(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::Role;
    use crate::config::ModerationMode;

    #[test]
    fn a_patch_changes_only_the_given_fields() {
        let current = ModerationConfig::default();
        let updated: ModerationConfig = merge_patch(
            &current,
            json!({"min_role": "subscriber", "cooldown_secs": 5}),
        )
        .unwrap();
        assert_eq!(updated.min_role, Role::Subscriber);
        assert_eq!(updated.cooldown_secs, 5);
        assert_eq!(updated.max_queue_len, current.max_queue_len);
        assert_eq!(updated.mode, ModerationMode::Manual);
        assert_eq!(updated.max_volume, current.max_volume);
    }

    #[test]
    fn an_empty_patch_changes_nothing() {
        let current = ModerationConfig {
            cooldown_secs: 7,
            ..ModerationConfig::default()
        };
        let updated: ModerationConfig = merge_patch(&current, json!({})).unwrap();
        assert_eq!(updated, current);
    }

    #[test]
    fn bad_patches_are_client_errors() {
        let current = ModerationConfig::default();
        for bad in [
            json!({"typo_field": 1}),
            json!({"min_role": "vip"}),
            json!({"cooldown_secs": "soon"}),
            json!({"cooldown_secs": -1}),
            json!({"volume": 300}),
            json!([1, 2]),
            json!("text"),
            json!(null),
            json!(5),
        ] {
            let error = merge_patch::<ModerationConfig>(&current, bad.clone())
                .err()
                .unwrap_or_else(|| panic!("accepted {bad}"));
            assert_eq!(error.status, StatusCode::BAD_REQUEST, "{bad}");
        }
    }

    #[test]
    fn blacklist_patches_replace_whole_lists() {
        let current = BlacklistConfig {
            video_ids: vec!["aaaaaaaaaaa".to_owned()],
            channels: vec!["Chan".to_owned()],
            title_words: vec![],
        };
        let updated: BlacklistConfig =
            merge_patch(&current, json!({"title_words": ["spam"]})).unwrap();
        assert_eq!(updated.video_ids, current.video_ids);
        assert_eq!(updated.channels, current.channels);
        assert_eq!(updated.title_words, vec!["spam".to_owned()]);

        let updated: BlacklistConfig = merge_patch(&current, json!({"video_ids": []})).unwrap();
        assert!(updated.video_ids.is_empty());
    }

    #[test]
    fn queue_errors_map_to_statuses() {
        let status = |error: QueueError| ApiError::from(error).status;
        assert_eq!(
            status(QueueError::NotFound(ItemId(1))),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            status(QueueError::NotPending(ItemId(1))),
            StatusCode::CONFLICT
        );
        assert_eq!(
            status(QueueError::NotPlaying(ItemId(1))),
            StatusCode::CONFLICT
        );
    }
}
