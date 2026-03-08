use percent_encoding::percent_decode_str;
use rust_embed::Embed;
use serde::Serialize;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;
use wstd::http::body::BoundedBody;
use wstd::http::body::IncomingBody;
use wstd::http::server::{Finished, Responder};
use wstd::http::{IntoBody, Method, Request, Response, StatusCode};
use wstd::io::{copy, Cursor};

const STORAGE_ROOT: &str = "data";
const RESOURCES_PREFIX: &str = "/api/resources";
const RAW_PREFIX: &str = "/api/raw";

type AppBody = BoundedBody<Vec<u8>>;
type AppResponse = Response<AppBody>;

#[derive(Embed)]
#[folder = "../frontend/dist"]
struct Assets;

#[derive(Serialize)]
struct Sorting {
    by: &'static str,
    asc: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ResourceItem {
    path: String,
    name: String,
    size: u64,
    extension: String,
    modified: String,
    mode: u32,
    is_dir: bool,
    is_symlink: bool,
    #[serde(rename = "type")]
    resource_type: String,
    url: String,
    index: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Resource {
    path: String,
    name: String,
    size: u64,
    extension: String,
    modified: String,
    mode: u32,
    is_dir: bool,
    is_symlink: bool,
    #[serde(rename = "type")]
    resource_type: String,
    url: String,
    index: usize,
    items: Vec<ResourceItem>,
    num_dirs: usize,
    num_files: usize,
    sorting: Sorting,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
}

#[derive(Serialize)]
struct StatusResponse<'a> {
    status: &'a str,
    message: &'a str,
}

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }
}

#[wstd::http_server]
async fn main(request: Request<IncomingBody>, responder: Responder) -> Finished {
    let result = route(request).await;

    match result {
        Ok(response) => responder.respond(response).await,
        Err(err) => responder.respond(json_error(err)).await,
    }
}

async fn route(mut request: Request<IncomingBody>) -> Result<AppResponse, ApiError> {
    let path = request.uri().path().to_string();
    let method = request.method().clone();

    match (method, path.as_str()) {
        (Method::GET, "/config.js") => Ok(config_js_response()),
        (Method::GET, "/api/health") => Ok(json_response(
            StatusCode::OK,
            &StatusResponse {
                status: "ok",
                message: "filebrowser-lite-wasi is running",
            },
        )),
        _ if path == RESOURCES_PREFIX
            || path == format!("{RESOURCES_PREFIX}/")
            || path.starts_with(&(RESOURCES_PREFIX.to_string() + "/")) => {
            handle_resources(&mut request).await
        }
        _ if path == RAW_PREFIX
            || path == format!("{RAW_PREFIX}/")
            || path.starts_with(&(RAW_PREFIX.to_string() + "/")) => handle_raw(&request).await,
        _ => serve_asset_route(&path),
    }
}

fn serve_asset_route(path: &str) -> Result<AppResponse, ApiError> {
    let asset_path = normalize_asset_path(path);
    let fallback = if asset_path == "index.html" {
        None
    } else {
        Some("index.html")
    };

    if let Some(asset) = Assets::get(&asset_path) {
        return Ok(asset_response(&asset_path, asset.data.as_ref()));
    }

    if let Some(fallback_path) = fallback {
        if let Some(asset) = Assets::get(fallback_path) {
            return Ok(asset_response(fallback_path, asset.data.as_ref()));
        }
    }

    Err(ApiError::new(StatusCode::NOT_FOUND, "route not found"))
}

async fn handle_resources(
    request: &mut Request<IncomingBody>,
) -> Result<AppResponse, ApiError> {
    let uri_path = request.uri().path().to_string();
    let resource_path = extract_route_path(&uri_path, RESOURCES_PREFIX);
    let path_info = resolve_storage_path(&resource_path)?;
    let query = request.uri().query().unwrap_or("");
    let method = request.method().clone();
    let dir_request = uri_path.ends_with('/');

    match method {
        Method::GET => {
            let resource = read_resource(&path_info.guest_path, &path_info.host_path)?;
            Ok(json_response(StatusCode::OK, &resource))
        }
        Method::POST => {
            if dir_request {
                fs::create_dir_all(&path_info.host_path)
                    .map_err(|err| io_error(StatusCode::INTERNAL_SERVER_ERROR, err))?;
                let resource = read_resource(&path_info.guest_path, &path_info.host_path)?;
                return Ok(json_response(StatusCode::OK, &resource));
            }

            let override_existing = query_flag(query, "override");
            if path_info.host_path.exists() && !override_existing {
                return Err(ApiError::new(
                    StatusCode::CONFLICT,
                    "target already exists; pass override=true to replace it",
                ));
            }

            write_request_body(request, &path_info.host_path).await?;
            let resource = read_resource(&path_info.guest_path, &path_info.host_path)?;
            Ok(json_response(StatusCode::OK, &resource))
        }
        Method::PUT => {
            if !path_info.host_path.exists() {
                return Err(ApiError::new(StatusCode::NOT_FOUND, "target file does not exist"));
            }

            if path_info.host_path.is_dir() {
                return Err(ApiError::new(
                    StatusCode::METHOD_NOT_ALLOWED,
                    "PUT only supports files",
                ));
            }

            write_request_body(request, &path_info.host_path).await?;
            let resource = read_resource(&path_info.guest_path, &path_info.host_path)?;
            Ok(json_response(StatusCode::OK, &resource))
        }
        Method::PATCH => handle_patch(query, &path_info).await,
        Method::DELETE => {
            if path_info.guest_path == "/" {
                return Err(ApiError::new(
                    StatusCode::FORBIDDEN,
                    "refusing to delete storage root",
                ));
            }

            delete_path(&path_info.host_path)?;
            Ok(json_response(
                StatusCode::OK,
                &StatusResponse {
                    status: "ok",
                    message: "deleted",
                },
            ))
        }
        _ => Err(ApiError::new(
            StatusCode::METHOD_NOT_ALLOWED,
            "method not allowed",
        )),
    }
}

async fn handle_patch(
    query: &str,
    source: &ResolvedPath,
) -> Result<AppResponse, ApiError> {
    if source.guest_path == "/" {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "refusing to modify storage root",
        ));
    }

    let action = query_value(query, "action")
        .ok_or_else(|| ApiError::new(StatusCode::BAD_REQUEST, "missing action query param"))?;
    let destination_raw = query_value(query, "destination").ok_or_else(|| {
        ApiError::new(StatusCode::BAD_REQUEST, "missing destination query param")
    })?;
    let destination = resolve_storage_path(&destination_raw)?;
    let override_existing = query_flag(query, "override");

    if destination.host_path.exists() && !override_existing {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "destination exists; pass override=true to replace it",
        ));
    }

    if override_existing && destination.host_path.exists() {
        delete_path(&destination.host_path)?;
    }

    match action.as_str() {
        "rename" => rename_path(&source.host_path, &destination.host_path)?,
        "copy" => copy_path(&source.host_path, &destination.host_path)?,
        _ => {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "unsupported action; use rename or copy",
            ));
        }
    }

    let resource = read_resource(&destination.guest_path, &destination.host_path)?;
    Ok(json_response(StatusCode::OK, &resource))
}

async fn handle_raw(request: &Request<IncomingBody>) -> Result<AppResponse, ApiError> {
    if request.method() != Method::GET {
        return Err(ApiError::new(
            StatusCode::METHOD_NOT_ALLOWED,
            "method not allowed",
        ));
    }

    let uri_path = request.uri().path().to_string();
    let resource_path = extract_route_path(&uri_path, RAW_PREFIX);
    let path_info = resolve_storage_path(&resource_path)?;
    let metadata = fs::metadata(&path_info.host_path).map_err(|err| match err.kind() {
        std::io::ErrorKind::NotFound => ApiError::new(StatusCode::NOT_FOUND, "file not found"),
        _ => io_error(StatusCode::INTERNAL_SERVER_ERROR, err),
    })?;

    if metadata.is_dir() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "raw download only supports files",
        ));
    }

    let bytes = fs::read(&path_info.host_path)
        .map_err(|err| io_error(StatusCode::INTERNAL_SERVER_ERROR, err))?;
    let filename = file_name_from_guest_path(&path_info.guest_path);
    let inline = query_flag(request.uri().query().unwrap_or(""), "inline");
    let content_disposition = if inline {
        format!("inline; filename=\"{}\"", filename)
    } else {
        format!("attachment; filename=\"{}\"", filename)
    };

    let response = Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", content_type_for(&path_info.host_path))
        .header("Content-Disposition", content_disposition)
        .body(bytes.into_body())
        .unwrap();

    Ok(response)
}

fn read_resource(guest_path: &str, host_path: &Path) -> Result<Resource, ApiError> {
    let metadata = fs::metadata(host_path).map_err(|err| match err.kind() {
        std::io::ErrorKind::NotFound => ApiError::new(StatusCode::NOT_FOUND, "path not found"),
        _ => io_error(StatusCode::INTERNAL_SERVER_ERROR, err),
    })?;

    let name = file_name_from_guest_path(guest_path);
    let modified = format_system_time(metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH));

    if metadata.is_dir() {
        let mut items = Vec::new();
        let mut num_dirs = 0usize;
        let mut num_files = 0usize;

        let entries = fs::read_dir(host_path)
            .map_err(|err| io_error(StatusCode::INTERNAL_SERVER_ERROR, err))?;

        for entry in entries {
            let entry = entry.map_err(|err| io_error(StatusCode::INTERNAL_SERVER_ERROR, err))?;
            let child_name = entry.file_name().to_string_lossy().to_string();
            let child_guest_path = join_guest_path(guest_path, &child_name);
            let child_item = read_resource_item(&child_guest_path, &entry.path())?;
            if child_item.is_dir {
                num_dirs += 1;
            } else {
                num_files += 1;
            }
            items.push(child_item);
        }

        items.sort_by(|left, right| match (left.is_dir, right.is_dir) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => left.name.to_lowercase().cmp(&right.name.to_lowercase()),
        });

        for (index, item) in items.iter_mut().enumerate() {
            item.index = index;
        }

        return Ok(Resource {
            path: guest_path.to_string(),
            name,
            size: 0,
            extension: String::new(),
            modified,
            mode: 0,
            is_dir: true,
            is_symlink: false,
            resource_type: "dir".to_string(),
            url: String::new(),
            index: 0,
            items,
            num_dirs,
            num_files,
            sorting: default_sorting(),
            content: None,
        });
    }

    let extension = extension_for_name(&name);
    let resource_type = detect_file_type(&name);
    let content = if is_text_resource(&resource_type) {
        Some(
            fs::read_to_string(host_path)
                .map_err(|err| io_error(StatusCode::INTERNAL_SERVER_ERROR, err))?,
        )
    } else {
        None
    };

    Ok(Resource {
        path: guest_path.to_string(),
        name: name.clone(),
        size: metadata.len(),
        modified,
        extension,
        mode: 0,
        is_dir: false,
        is_symlink: false,
        resource_type,
        url: String::new(),
        index: 0,
        items: Vec::new(),
        num_dirs: 0,
        num_files: 0,
        sorting: default_sorting(),
        content,
    })
}

fn read_resource_item(guest_path: &str, host_path: &Path) -> Result<ResourceItem, ApiError> {
    let metadata = fs::metadata(host_path).map_err(|err| match err.kind() {
        std::io::ErrorKind::NotFound => ApiError::new(StatusCode::NOT_FOUND, "path not found"),
        _ => io_error(StatusCode::INTERNAL_SERVER_ERROR, err),
    })?;

    let name = file_name_from_guest_path(guest_path);
    let extension = extension_for_name(&name);

    Ok(ResourceItem {
        path: guest_path.to_string(),
        name: name.clone(),
        size: if metadata.is_dir() { 0 } else { metadata.len() },
        extension,
        modified: format_system_time(metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH)),
        mode: 0,
        is_dir: metadata.is_dir(),
        is_symlink: false,
        resource_type: if metadata.is_dir() {
            "dir".to_string()
        } else {
            detect_file_type(&name)
        },
        url: String::new(),
        index: 0,
    })
}

async fn write_request_body(
    request: &mut Request<IncomingBody>,
    host_path: &Path,
) -> Result<(), ApiError> {
    if let Some(parent) = host_path.parent() {
        fs::create_dir_all(parent).map_err(|err| io_error(StatusCode::INTERNAL_SERVER_ERROR, err))?;
    }

    let mut body = Vec::new();
    copy(request.body_mut(), &mut Cursor::new(&mut body))
        .await
        .map_err(|err| ApiError::new(StatusCode::BAD_REQUEST, err.to_string()))?;

    let mut file = File::create(host_path)
        .map_err(|err| io_error(StatusCode::INTERNAL_SERVER_ERROR, err))?;
    file.write_all(&body)
        .map_err(|err| io_error(StatusCode::INTERNAL_SERVER_ERROR, err))?;
    file.sync_all()
        .map_err(|err| io_error(StatusCode::INTERNAL_SERVER_ERROR, err))?;
    Ok(())
}

fn rename_path(source: &Path, destination: &Path) -> Result<(), ApiError> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).map_err(|err| io_error(StatusCode::INTERNAL_SERVER_ERROR, err))?;
    }

    if fs::rename(source, destination).is_ok() {
        return Ok(());
    }

    copy_path(source, destination)?;
    delete_path(source)?;
    Ok(())
}

fn copy_path(source: &Path, destination: &Path) -> Result<(), ApiError> {
    let metadata = fs::metadata(source).map_err(|err| match err.kind() {
        std::io::ErrorKind::NotFound => ApiError::new(StatusCode::NOT_FOUND, "source not found"),
        _ => io_error(StatusCode::INTERNAL_SERVER_ERROR, err),
    })?;

    if metadata.is_dir() {
        fs::create_dir_all(destination).map_err(|err| io_error(StatusCode::INTERNAL_SERVER_ERROR, err))?;
        let entries =
            fs::read_dir(source).map_err(|err| io_error(StatusCode::INTERNAL_SERVER_ERROR, err))?;
        for entry in entries {
            let entry = entry.map_err(|err| io_error(StatusCode::INTERNAL_SERVER_ERROR, err))?;
            copy_path(&entry.path(), &destination.join(entry.file_name()))?;
        }
        return Ok(());
    }

    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).map_err(|err| io_error(StatusCode::INTERNAL_SERVER_ERROR, err))?;
    }

    fs::copy(source, destination).map_err(|err| io_error(StatusCode::INTERNAL_SERVER_ERROR, err))?;
    Ok(())
}

fn delete_path(target: &Path) -> Result<(), ApiError> {
    let metadata = fs::metadata(target).map_err(|err| match err.kind() {
        std::io::ErrorKind::NotFound => ApiError::new(StatusCode::NOT_FOUND, "path not found"),
        _ => io_error(StatusCode::INTERNAL_SERVER_ERROR, err),
    })?;

    if metadata.is_dir() {
        fs::remove_dir_all(target).map_err(|err| io_error(StatusCode::INTERNAL_SERVER_ERROR, err))?;
    } else {
        fs::remove_file(target).map_err(|err| io_error(StatusCode::INTERNAL_SERVER_ERROR, err))?;
    }

    Ok(())
}

fn extract_route_path(path: &str, prefix: &str) -> String {
    match path.strip_prefix(prefix) {
        Some("") | None => "/".to_string(),
        Some(rest) => {
            if rest.is_empty() {
                "/".to_string()
            } else {
                rest.to_string()
            }
        }
    }
}

struct ResolvedPath {
    guest_path: String,
    host_path: PathBuf,
}

fn resolve_storage_path(raw_path: &str) -> Result<ResolvedPath, ApiError> {
    let decoded = percent_decode_str(raw_path)
        .decode_utf8()
        .map_err(|_| ApiError::new(StatusCode::BAD_REQUEST, "invalid UTF-8 in path"))?;

    let mut segments = Vec::new();
    for segment in decoded.split('/') {
        if segment.is_empty() || segment == "." {
            continue;
        }

        if segment == ".." {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "path traversal is not allowed",
            ));
        }

        segments.push(segment.to_string());
    }

    let guest_path = if segments.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", segments.join("/"))
    };

    let mut host_path = PathBuf::from(STORAGE_ROOT);
    for segment in &segments {
        host_path.push(segment);
    }

    Ok(ResolvedPath {
        guest_path,
        host_path,
    })
}

fn join_guest_path(parent: &str, child: &str) -> String {
    if parent == "/" {
        format!("/{child}")
    } else {
        format!("{parent}/{child}")
    }
}

fn file_name_from_guest_path(path: &str) -> String {
    if path == "/" {
        return String::new();
    }

    path.rsplit('/').next().unwrap_or_default().to_string()
}

fn extension_for_name(name: &str) -> String {
    Path::new(name)
        .extension()
        .map(|value| format!(".{}", value.to_string_lossy()))
        .unwrap_or_default()
}

fn detect_file_type(name: &str) -> String {
    match Path::new(name)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "mp4" | "webm" | "mov" | "mkv" => "video".to_string(),
        "mp3" | "wav" | "flac" | "ogg" | "m4a" => "audio".to_string(),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" => "image".to_string(),
        "pdf" => "pdf".to_string(),
        "md" | "txt" | "json" | "toml" | "yaml" | "yml" | "rs" | "go" | "js"
        | "ts" | "tsx" | "jsx" | "html" | "css" | "csv" | "xml" | "sh" | "py"
        | "java" | "c" | "cc" | "cpp" | "h" | "hpp" => "text".to_string(),
        _ => "blob".to_string(),
    }
}

fn is_text_resource(resource_type: &str) -> bool {
    matches!(resource_type, "text" | "textImmutable")
}

fn default_sorting() -> Sorting {
    Sorting {
        by: "name",
        asc: true,
    }
}

fn format_system_time(value: SystemTime) -> String {
    OffsetDateTime::from(value)
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

fn content_type_for(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "html" => "text/html; charset=utf-8",
        "css" => "text/css",
        "js" => "application/javascript",
        "json" => "application/json",
        "md" | "txt" | "rs" | "go" | "toml" | "yaml" | "yml" => "text/plain; charset=utf-8",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        _ => "application/octet-stream",
    }
}

fn query_value(query: &str, key: &str) -> Option<String> {
    query.split('&').find_map(|pair| {
        let (current_key, current_value) = pair.split_once('=')?;
        if current_key != key {
            return None;
        }

        percent_decode_str(current_value).decode_utf8().ok().map(|value| value.to_string())
    })
}

fn query_flag(query: &str, key: &str) -> bool {
    query_value(query, key)
        .map(|value| value.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

fn io_error(status: StatusCode, err: std::io::Error) -> ApiError {
    ApiError::new(status, err.to_string())
}

fn normalize_asset_path(path: &str) -> String {
    let trimmed = path.trim_start_matches('/');
    if trimmed.is_empty() {
        return "index.html".to_string();
    }

    if trimmed.ends_with('/') {
        return format!("{}index.html", trimmed);
    }

    trimmed.to_string()
}

fn config_js_response() -> AppResponse {
        let body = r#"window.__FILEBROWSER_CONFIG__ = {
    AuthMethod: "json",
    BaseURL: "",
    CSS: false,
    Color: "",
    DisableExternal: false,
    DisableUsedPercentage: true,
    EnableExec: false,
    EnableThumbs: false,
    LogoutPage: "",
    LoginPage: false,
    Name: "File Browser Lite",
    NoAuth: true,
    ReCaptcha: false,
    ResizePreview: false,
    Signup: false,
    StaticURL: "",
    Theme: "",
    TusSettings: null,
    Version: "lite-wasi",
    LiteMode: true,
};"#;

        Response::builder()
                .status(StatusCode::OK)
                .header("Content-Type", "application/javascript; charset=utf-8")
                .body(body.as_bytes().to_vec().into_body())
                .unwrap()
}

fn asset_response(path: &str, bytes: &[u8]) -> AppResponse {
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", content_type_for_asset(path))
        .body(bytes.to_vec().into_body())
        .unwrap()
}

fn content_type_for_asset(path: &str) -> &'static str {
    match Path::new(path)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "html" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" => "application/javascript; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ico" => "image/x-icon",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        _ => "application/octet-stream",
    }
}

fn json_response<T: Serialize>(status: StatusCode, value: &T) -> AppResponse {
    let body = serde_json::to_string(value).unwrap_or_else(|_| "{}".to_string());
    Response::builder()
        .status(status)
        .header("Content-Type", "application/json")
        .body(body.into_body())
        .unwrap()
}

fn json_error(err: ApiError) -> AppResponse {
    let payload = serde_json::json!({
        "status": err.status.as_u16(),
        "error": err.message,
    });
    json_response(err.status, &payload)
}
