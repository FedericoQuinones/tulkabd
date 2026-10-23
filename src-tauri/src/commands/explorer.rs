use tauri::State;
use tulka_app::tulka_core::{ColumnInfo, DbObject, ObjectKind, TableStructure};
use tulka_app::tulka_store::CachedDatabase;
use tulka_app::{AppState, CompletionMetadata, ExplorerInfo, SearchResponse};

#[tauri::command]
pub async fn connect_explorer(
    state: State<'_, AppState>,
    connection_id: String,
    password: Option<String>,
) -> Result<ExplorerInfo, String> {
    tulka_app::connect_explorer(&state, connection_id, password).await
}

#[tauri::command]
pub async fn disconnect_explorer(
    state: State<'_, AppState>,
    connection_id: String,
) -> Result<(), String> {
    tulka_app::disconnect_explorer(&state, connection_id).await
}

#[tauri::command]
pub async fn refresh_metadata(
    state: State<'_, AppState>,
    connection_id: String,
    database: Option<String>,
) -> Result<(), String> {
    tulka_app::refresh_metadata(&state, connection_id, database).await
}

#[tauri::command]
pub async fn explorer_databases(
    state: State<'_, AppState>,
    connection_id: String,
) -> Result<Vec<CachedDatabase>, String> {
    Ok(tulka_app::explorer_databases(&state, connection_id).await)
}

#[tauri::command]
pub async fn explorer_objects(
    state: State<'_, AppState>,
    connection_id: String,
    database: String,
    kind: ObjectKind,
) -> Result<Vec<DbObject>, String> {
    Ok(tulka_app::explorer_objects(&state, connection_id, database, kind).await)
}

#[tauri::command]
pub async fn explorer_columns(
    state: State<'_, AppState>,
    connection_id: String,
    database: String,
    schema: String,
    name: String,
) -> Result<Vec<ColumnInfo>, String> {
    Ok(tulka_app::explorer_columns(&state, connection_id, database, schema, name).await)
}

#[tauri::command]
pub async fn table_structure(
    state: State<'_, AppState>,
    connection_id: String,
    database: String,
    schema: String,
    name: String,
) -> Result<TableStructure, String> {
    tulka_app::table_structure(&state, connection_id, database, schema, name).await
}

#[tauri::command]
pub async fn object_definition(
    state: State<'_, AppState>,
    connection_id: String,
    database: String,
    schema: String,
    name: String,
) -> Result<String, String> {
    tulka_app::object_definition(&state, connection_id, database, schema, name).await
}

#[tauri::command]
pub async fn search_objects(
    state: State<'_, AppState>,
    query: String,
    limit: usize,
) -> Result<SearchResponse, String> {
    Ok(tulka_app::search_objects(&state, query, limit).await)
}

#[tauri::command]
pub async fn completion_metadata(
    state: State<'_, AppState>,
    connection_id: String,
    database: String,
) -> Result<CompletionMetadata, String> {
    Ok(tulka_app::completion_metadata(&state, connection_id, database).await)
}
