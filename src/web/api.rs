use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Json, Router, routing::get};
use serde::{Deserialize, Serialize};

use super::AppState;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/stats", get(get_stats))
        .route("/archives", get(get_archives))
        .route("/localities", get(get_localities))
        .route("/manifests", get(get_manifests))
        .route("/manifests/{id}", get(get_manifest))
        .route("/manifests/{id}/pages", get(get_manifest_pages))
        .route("/manifests/{id}/metadata", get(get_manifest_metadata))
        .route("/persons", get(get_persons))
        .route("/persons/{id}", get(get_person))
        .route("/search/ocr", get(search_ocr))
        .route("/facets/doc_types", get(get_facet_doc_types))
        .route("/facets/years", get(get_facet_years))
        .route("/downloads/{id}/ocr", get(get_download_ocr))
        .route("/downloads/{id}/tags", get(get_download_tags))
}

// ─── Query parameter structs ─────────────────────────────────────────────

#[derive(Deserialize)]
struct ManifestQuery {
    doc_type: Option<String>,
    year: Option<String>,
    archive: Option<String>,
    locality: Option<String>,
    page: Option<usize>,
    per_page: Option<usize>,
}

#[derive(Deserialize)]
struct LocalityQuery {
    q: Option<String>,
}

#[derive(Deserialize)]
struct PersonQuery {
    surname: Option<String>,
    name: Option<String>,
    page: Option<usize>,
    per_page: Option<usize>,
}

#[derive(Deserialize)]
struct OcrQuery {
    q: String,
    limit: Option<usize>,
}

// ─── Response wrappers ───────────────────────────────────────────────────

#[derive(Serialize)]
struct PaginatedResponse<T: Serialize> {
    data: Vec<T>,
    total: usize,
    page: usize,
    per_page: usize,
}

// ─── Handlers ────────────────────────────────────────────────────────────

async fn get_stats(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let db = state.db.lock().unwrap();
    match db.get_extended_stats() {
        Ok(stats) => Json(stats).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn get_archives(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let db = state.db.lock().unwrap();
    match db.list_archives() {
        Ok(archives) => Json(archives).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn get_localities(
    State(state): State<Arc<AppState>>,
    Query(params): Query<LocalityQuery>,
) -> impl IntoResponse {
    let db = state.db.lock().unwrap();
    let pattern = params.q.as_deref().unwrap_or("");
    match db.search_localities(pattern) {
        Ok(localities) => Json(localities).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn get_manifests(
    State(state): State<Arc<AppState>>,
    Query(params): Query<ManifestQuery>,
) -> impl IntoResponse {
    let db = state.db.lock().unwrap();
    let page = params.page.unwrap_or(1).max(1);
    let per_page = params.per_page.unwrap_or(50).min(200);
    let offset = (page - 1) * per_page;

    match db.search_manifests_paginated(
        params.doc_type.as_deref(),
        params.year.as_deref(),
        params.archive.as_deref(),
        params.locality.as_deref(),
        offset,
        per_page,
    ) {
        Ok((data, total)) => Json(PaginatedResponse {
            data,
            total,
            page,
            per_page,
        })
        .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn get_manifest(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let db = state.db.lock().unwrap();
    match db.get_manifest_by_id(&id) {
        Ok(Some(manifest)) => Json(manifest).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn get_manifest_pages(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let db = state.db.lock().unwrap();
    match db.get_all_downloads_for_manifest(&id) {
        Ok(pages) => Json(pages).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn get_manifest_metadata(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let db = state.db.lock().unwrap();
    match db.get_manifest_metadata(&id) {
        Ok(metadata) => Json(metadata).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn get_persons(
    State(state): State<Arc<AppState>>,
    Query(params): Query<PersonQuery>,
) -> impl IntoResponse {
    let db = state.db.lock().unwrap();
    let page = params.page.unwrap_or(1).max(1);
    let per_page = params.per_page.unwrap_or(50).min(200);
    let offset = (page - 1) * per_page;

    match db.search_persons_paginated(
        params.surname.as_deref(),
        params.name.as_deref(),
        offset,
        per_page,
    ) {
        Ok((data, total)) => Json(PaginatedResponse {
            data,
            total,
            page,
            per_page,
        })
        .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn get_person(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    let db = state.db.lock().unwrap();
    match db.get_person_records(id) {
        Ok(records) => Json(records).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn search_ocr(
    State(state): State<Arc<AppState>>,
    Query(params): Query<OcrQuery>,
) -> impl IntoResponse {
    let db = state.db.lock().unwrap();
    let limit = params.limit.unwrap_or(50).min(200);
    match db.search_ocr_text(&params.q, limit) {
        Ok(results) => Json(results).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn get_facet_doc_types(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let db = state.db.lock().unwrap();
    match db.get_distinct_doc_types() {
        Ok(types) => Json(types).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn get_facet_years(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let db = state.db.lock().unwrap();
    match db.get_distinct_years() {
        Ok(years) => Json(years).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn get_download_ocr(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    let db = state.db.lock().unwrap();
    match db.get_ocr_for_download(id) {
        Ok(results) => Json(results).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn get_download_tags(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    let db = state.db.lock().unwrap();
    match db.get_tags_for_download(id) {
        Ok(tags) => Json(tags).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}
