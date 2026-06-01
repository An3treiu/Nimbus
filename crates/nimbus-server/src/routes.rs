use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use nimbus_core::DriveFile;
use nimbus_storage::StorageEngine;
use std::sync::Arc;

pub type AppState = Arc<StorageEngine>;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/files", get(list_files))
        .route("/api/files/*path", get(download_file).post(upload_file))
        .route("/api/sync", post(sync_drive))
        .with_state(state)
}

async fn sync_drive(State(engine): State<AppState>) -> Result<StatusCode, StatusCode> {
    engine
        .sync()
        .await
        .map(|_| StatusCode::NO_CONTENT)
        .map_err(|_| StatusCode::BAD_GATEWAY)
}

async fn list_files(State(engine): State<AppState>) -> Result<Json<Vec<DriveFile>>, StatusCode> {
    engine
        .list()
        .await
        .map(Json)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

async fn upload_file(
    State(engine): State<AppState>,
    Path(path): Path<String>,
    body: axum::body::Bytes,
) -> Result<Json<DriveFile>, StatusCode> {
    engine
        .upload(&path, &body)
        .await
        .map(Json)
        .map_err(|_| StatusCode::BAD_GATEWAY)
}

async fn download_file(
    State(engine): State<AppState>,
    Path(path): Path<String>,
) -> Result<axum::body::Bytes, StatusCode> {
    match engine.download(&path).await {
        Ok(bytes) => Ok(bytes.into()),
        Err(nimbus_core::NimbusError::NotFound(_)) => Err(StatusCode::NOT_FOUND),
        Err(_) => Err(StatusCode::BAD_GATEWAY),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use serde_json::json;
    use tower::ServiceExt;
    use wiremock::matchers::{method, path as wpath};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn test_engine(gh_uri: String) -> AppState {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!("../../migrations").run(&pool).await.unwrap();
        let gh = nimbus_github::GitHubClient::new("tok", gh_uri);
        Arc::new(StorageEngine::new(gh, pool, "me", "drive", "main"))
    }

    /// Mount every endpoint an upload touches (blob + the commit dance).
    async fn mount_upload(server: &MockServer) {
        Mock::given(method("POST"))
            .and(wpath("/repos/me/drive/git/blobs"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({"sha":"s1"})))
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(wpath("/repos/me/drive/git/ref/heads/main"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"object":{"sha":"head1"}})))
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(wpath("/repos/me/drive/git/commits/head1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"tree":{"sha":"base"}})))
            .mount(server)
            .await;
        Mock::given(method("POST"))
            .and(wpath("/repos/me/drive/git/trees"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({"sha":"tree1"})))
            .mount(server)
            .await;
        Mock::given(method("POST"))
            .and(wpath("/repos/me/drive/git/commits"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({"sha":"commit1"})))
            .mount(server)
            .await;
        Mock::given(method("PATCH"))
            .and(wpath("/repos/me/drive/git/refs/heads/main"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ref":"refs/heads/main"})))
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn upload_then_list_roundtrip() {
        let server = MockServer::start().await;
        mount_upload(&server).await;

        let app = router(test_engine(server.uri()).await);

        // upload
        let up = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/files/hello.txt")
                    .body(Body::from("hi"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(up.status(), StatusCode::OK);

        // list
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/files")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let files: Vec<DriveFile> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "hello.txt");
    }

    #[tokio::test]
    async fn download_missing_returns_404() {
        let server = MockServer::start().await;
        let app = router(test_engine(server.uri()).await);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/files/nope.txt")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
