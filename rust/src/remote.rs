use crate::runtime::{default_manager, manager_for};
use crate::service::{Definition, normalize_location, validate_name};
use crate::store::{Link, Store};
use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;
use tar::{Archive, Builder, Header};
use tiny_http::{Header as HttpHeader, Method, Response, Server, StatusCode};
use url::Url;

type HttpResponse = Response<std::io::Cursor<Vec<u8>>>;
const API_KEY_HEADER: &str = "X-Burner-Key";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeployRequest {
    pub name: String,
    pub command: String,
    pub location: String,
    pub include_files: bool,
    #[serde(default)]
    pub archive: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlRequest {
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteRequest {
    pub name: String,
    #[serde(default)]
    pub force: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageResponse {
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListResponse {
    pub services: Vec<Definition>,
}

pub struct Client {
    agent: ureq::Agent,
}

impl Client {
    pub fn new() -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(30))
            .build();
        Self { agent }
    }

    pub fn ping(&self, link: &Link) -> Result<()> {
        self.authorized_get(link, &format!("{}/v1/ping", link.base_url))?
            .call()
            .map_err(http_error("ping remote burner"))?;
        Ok(())
    }

    pub fn deploy(&self, link: &Link, body: &DeployRequest) -> Result<()> {
        self.post_json::<_, MessageResponse>(link, &format!("{}/v1/deploy", link.base_url), body)
            .map(|_| ())
    }

    pub fn control(&self, link: &Link, action: &str, name: &str) -> Result<()> {
        self.post_json::<_, MessageResponse>(
            link,
            &format!(
                "{}/v1/services/{}/{}",
                link.base_url,
                urlencoding::encode(name),
                action
            ),
            &ControlRequest {
                name: name.to_string(),
            },
        )
        .map(|_| ())
    }

    pub fn delete(&self, link: &Link, name: &str, force: bool) -> Result<()> {
        self.post_json::<_, MessageResponse>(
            link,
            &format!(
                "{}/v1/services/{}/delete",
                link.base_url,
                urlencoding::encode(name),
            ),
            &DeleteRequest {
                name: name.to_string(),
                force,
            },
        )
        .map(|_| ())
    }

    pub fn list(&self, link: &Link) -> Result<Vec<Definition>> {
        let response = self
            .authorized_get(link, &format!("{}/v1/services", link.base_url))?
            .call()
            .map_err(http_error("list remote services"))?;
        let payload: ListResponse = response
            .into_json()
            .context("decode remote services")?;
        Ok(payload.services)
    }

    pub fn logs(&self, link: &Link, name: &str, lines: usize) -> Result<Vec<u8>> {
        let response = self
            .authorized_get(
                link,
                &format!(
                "{}/v1/services/{}/logs?n={}",
                link.base_url,
                urlencoding::encode(name),
                lines
                ),
            )?
            .call()
            .map_err(http_error("read remote logs"))?;
        Ok(read_body_bytes(response))
    }

    fn post_json<T, O>(&self, link: &Link, endpoint: &str, body: &T) -> Result<O>
    where
        T: Serialize,
        O: for<'de> Deserialize<'de>,
    {
        let response = self
            .authorized_post(link, endpoint)?
            .send_json(serde_json::to_value(body).context("encode request")?)
            .map_err(http_error(&format!("request {endpoint}")))?;
        response.into_json().context("decode response")
    }

    fn authorized_get(&self, link: &Link, endpoint: &str) -> Result<ureq::Request> {
        Ok(self
            .agent
            .get(endpoint)
            .set(API_KEY_HEADER, require_api_key(link)?))
    }

    fn authorized_post(&self, link: &Link, endpoint: &str) -> Result<ureq::Request> {
        Ok(self.agent.post(endpoint).set(API_KEY_HEADER, require_api_key(link)?))
    }
}

pub fn serve(listen_addr: &str) -> Result<()> {
    let root = Store::default_root()?;
    let st = Store::new(root);
    st.init()?;
    let api_key = st.api_key()?;
    let server = Server::http(listen_addr).map_err(|err| anyhow!("run daemon: {err}"))?;

    for request in server.incoming_requests() {
        let _ = handle_request(&st, &api_key, request);
    }
    Ok(())
}

#[derive(Debug)]
struct HttpFailure {
    status: u16,
    message: String,
}

fn http_failure(status: u16, message: impl Into<String>) -> HttpFailure {
    HttpFailure {
        status,
        message: message.into(),
    }
}

fn handle_request(st: &Store, api_key: &str, mut request: tiny_http::Request) -> Result<()> {
    let url = request.url().to_string();
    let (path, query) = split_url(&url);

    let auth_result = if path.starts_with("/v1/") {
        authorize_request(&request, api_key)
    } else {
        Ok(())
    };

    let response = match (request.method(), path.as_str()) {
        _ if auth_result.is_err() => Err(auth_result.err().unwrap()),
        (&Method::Get, "/v1/ping") => Ok(json_response(
            200,
            &MessageResponse {
                message: "burner-ok".into(),
            },
        )),
        (&Method::Post, "/v1/deploy") => handle_deploy(st, &mut request),
        (&Method::Get, "/v1/services") => handle_list(st),
        _ if path.starts_with("/v1/services/") => {
            handle_service_action(st, request.method(), &path, query.as_deref())
        }
        _ => Err(http_failure(404, "unknown route")),
    }
    .unwrap_or_else(|err| text_response(err.status, err.message));

    request.respond(response).context("send response")?;
    Ok(())
}

fn handle_deploy(
    st: &Store,
    request: &mut tiny_http::Request,
) -> std::result::Result<HttpResponse, HttpFailure> {
    let req: DeployRequest = read_json(request).map_err(|_| http_failure(400, "invalid deploy request"))?;
    validate_name(&req.name).map_err(|err| http_failure(400, err.to_string()))?;
    if req.command.is_empty() {
        return Err(http_failure(400, "deploy requires a command"));
    }

    let mut location = req.location.clone();
    if req.include_files {
        let deploy_root = st
            .deployments_dir()
            .join(format!("{}-{}", req.name, crate::service::timestamp().replace([':', '+'], "")));
        fs::create_dir_all(&deploy_root).map_err(|err| http_failure(500, err.to_string()))?;
        extract_directory_base64(&req.archive, &deploy_root)
            .map_err(|err| http_failure(400, err.to_string()))?;
        location = deploy_root.to_string_lossy().into_owned();
    }

    let location = normalize_location(&location).map_err(|err| http_failure(400, err.to_string()))?;
    let mut def = Definition {
        name: req.name,
        command: req.command,
        location,
        status: "pending".into(),
        ..Definition::default()
    };

    let manager = default_manager();
    def.runtime = manager.name().into();
    let exec_path = std::env::current_exe()
        .context("find executable path")
        .map_err(|err| http_failure(500, err.to_string()))?
        .to_string_lossy()
        .into_owned();
    manager
        .deploy(&mut def, Some(&exec_path))
        .map_err(|err| http_failure(400, err.to_string()))?;
    st.save(&mut def)
        .map_err(|err| http_failure(500, err.to_string()))?;

    Ok(json_response(
        200,
        &MessageResponse {
            message: format!("deployed {}", def.name),
        },
    ))
}

fn handle_list(st: &Store) -> std::result::Result<HttpResponse, HttpFailure> {
    let mut services = st.list().map_err(|err| http_failure(500, err.to_string()))?;
    for service in &mut services {
        if let Ok(manager) = manager_for(service) {
            if let Ok(status) = manager.status(service) {
                service.status = status;
                let mut saved = service.clone();
                let _ = st.save(&mut saved);
            }
        }
    }
    Ok(json_response(200, &ListResponse { services }))
}

fn handle_service_action(
    st: &Store,
    method: &Method,
    path: &str,
    query: Option<&str>,
) -> std::result::Result<HttpResponse, HttpFailure> {
    let suffix = &path["/v1/services/".len()..];
    let parts: Vec<String> = suffix
        .split('/')
        .filter(|part| !part.is_empty())
        .map(percent_decode)
        .collect();
    if parts.len() < 2 {
        return Err(http_failure(404, "unknown route"));
    }

    let name = &parts[0];
    let action = &parts[1];

    match (method, action.as_str()) {
        (&Method::Post, "start" | "stop" | "restart") => {
            let mut def = st.get(name).map_err(|err| http_failure(404, err.to_string()))?;
            let manager = manager_for(&def).map_err(|err| http_failure(400, err.to_string()))?;
            match action.as_str() {
                "start" => manager.start(&mut def).map_err(|err| http_failure(400, err.to_string()))?,
                "stop" => manager.stop(&mut def).map_err(|err| http_failure(400, err.to_string()))?,
                "restart" => manager.restart(&mut def).map_err(|err| http_failure(400, err.to_string()))?,
                _ => unreachable!(),
            }
            st.save(&mut def)
                .map_err(|err| http_failure(500, err.to_string()))?;
            Ok(json_response(
                200,
                &MessageResponse {
                    message: format!("{action} {}", def.name),
                },
            ))
        }
        (&Method::Post, "delete") => {
            let req: DeleteRequest =
                read_json_body(request_body(method, path, query)).map_err(|_| http_failure(400, "invalid delete request"))?;
            let def = st.get(name).map_err(|err| http_failure(404, err.to_string()))?;
            let backup_path = if req.force {
                None
            } else {
                Some(st.backup_service(&def).map_err(|err| http_failure(500, err.to_string()))?)
            };

            let mut def = def;
            let manager = manager_for(&def).map_err(|err| http_failure(400, err.to_string()))?;
            manager.delete(&mut def).map_err(|err| http_failure(400, err.to_string()))?;
            st.delete_service(&def.name)
                .map_err(|err| http_failure(500, err.to_string()))?;
            st.delete_log(&def)
                .map_err(|err| http_failure(500, err.to_string()))?;

            let message = match backup_path {
                Some(path) => format!("deleted {} (backup: {})", def.name, path.display()),
                None => format!("deleted {} permanently", def.name),
            };
            Ok(json_response(200, &MessageResponse { message }))
        }
        (&Method::Get, "logs") => {
            let def = st.get(name).map_err(|err| http_failure(404, err.to_string()))?;
            let lines = parse_lines(query).unwrap_or(100);
            let manager = manager_for(&def).map_err(|err| http_failure(400, err.to_string()))?;
            let output = manager
                .logs(&def, lines)
                .map_err(|err| http_failure(400, err.to_string()))?;
            Ok(Response::from_data(output).with_status_code(StatusCode(200)).with_header(
                HttpHeader::from_bytes("Content-Type", "text/plain; charset=utf-8").unwrap(),
            ))
        }
        _ => Err(http_failure(405, "method not allowed")),
    }
}

pub fn encode_directory_base64(root: &str) -> Result<String> {
    let mut buffer = Vec::new();
    {
        let encoder = GzEncoder::new(&mut buffer, Compression::default());
        let mut tar = Builder::new(encoder);
        append_directory(&mut tar, Path::new(root), Path::new(root))?;
        let encoder = tar.into_inner().context("close tar writer")?;
        encoder.finish().context("close gzip writer")?;
    }
    Ok(BASE64.encode(buffer))
}

fn append_directory<W: Write>(tar: &mut Builder<W>, root: &Path, current: &Path) -> Result<()> {
    for entry in fs::read_dir(current).with_context(|| format!("archive directory {}", current.display()))? {
        let entry = entry?;
        let path = entry.path();
        let metadata = entry.metadata()?;
        let rel = path.strip_prefix(root).unwrap();
        let rel_string = rel.to_string_lossy().replace('\\', "/");

        if metadata.is_dir() {
            let mut header = Header::new_gnu();
            header.set_metadata(&metadata);
            header.set_entry_type(tar::EntryType::Directory);
            header.set_size(0);
            header.set_cksum();
            tar.append_data(&mut header, rel_string.clone(), std::io::empty())?;
            append_directory(tar, root, &path)?;
        } else if metadata.is_file() {
            let mut file = fs::File::open(&path)?;
            tar.append_file(rel_string, &mut file)?;
        }
    }
    Ok(())
}

pub fn extract_directory_base64(encoded: &str, dest: &Path) -> Result<()> {
    let raw = BASE64.decode(encoded).context("decode archive")?;
    let decoder = GzDecoder::new(&raw[..]);
    let mut archive = Archive::new(decoder);
    for entry in archive.entries().context("read archive")? {
        let mut entry = entry.context("read archive")?;
        let path = entry.path().context("read archive path")?;
        let safe = sanitize_archive_path(&path)?;
        let target = dest.join(&safe);

        if entry.header().entry_type().is_dir() {
            fs::create_dir_all(&target).context("create directory")?;
            continue;
        }

        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).context("create parent directory")?;
        }
        entry.unpack(&target).context("write file")?;
    }
    Ok(())
}

pub fn normalize_base_url(raw: &str, port: u16) -> Result<String> {
    let mut value = raw.to_string();
    if !value.contains("://") {
        value = format!("http://{value}");
    }
    let mut parsed = Url::parse(&value).context("parse url")?;
    parsed
        .set_port(Some(port))
        .map_err(|_| anyhow!("parse url: invalid port"))?;
    Ok(parsed.to_string().trim_end_matches('/').to_string())
}

fn sanitize_archive_path(path: &Path) -> Result<PathBuf> {
    let mut clean = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => clean.push(part),
            Component::CurDir => {}
            _ => bail!("invalid archive path {:?}", path),
        }
    }
    Ok(clean)
}

fn split_url(url: &str) -> (String, Option<String>) {
    match url.split_once('?') {
        Some((path, query)) => (path.to_string(), Some(query.to_string())),
        None => (url.to_string(), None),
    }
}

fn parse_lines(query: Option<&str>) -> Option<usize> {
    let query = query?;
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=')?;
        if key == "n" {
            return value.parse().ok();
        }
    }
    None
}

fn percent_decode(value: &str) -> String {
    urlencoding::decode(value)
        .map(|decoded| decoded.into_owned())
        .unwrap_or_else(|_| value.to_string())
}

fn json_response<T: Serialize>(status: u16, body: &T) -> HttpResponse {
    let payload = serde_json::to_vec(body).unwrap_or_else(|_| b"{}".to_vec());
    Response::from_data(payload)
        .with_status_code(StatusCode(status))
        .with_header(HttpHeader::from_bytes("Content-Type", "application/json").unwrap())
}

fn text_response(status: u16, body: String) -> HttpResponse {
    Response::from_data(body.into_bytes()).with_status_code(StatusCode(status))
}

fn read_json<T: for<'de> Deserialize<'de>>(request: &mut tiny_http::Request) -> Result<T> {
    let mut body = String::new();
    request.as_reader().read_to_string(&mut body)?;
    serde_json::from_str(&body).map_err(Into::into)
}

fn read_json_body<T: for<'de> Deserialize<'de>>(body: String) -> Result<T> {
    serde_json::from_str(&body).map_err(Into::into)
}

fn request_body(_method: &Method, _path: &str, _query: Option<&str>) -> String {
    String::new()
}

fn read_body_text(response: ureq::Response) -> String {
    let mut reader = response.into_reader();
    let mut body = String::new();
    let _ = reader.read_to_string(&mut body);
    body.trim().to_string()
}

fn read_body_bytes(response: ureq::Response) -> Vec<u8> {
    let mut reader = response.into_reader();
    let mut body = Vec::new();
    let _ = reader.read_to_end(&mut body);
    body
}

fn http_error(prefix: &str) -> impl Fn(ureq::Error) -> anyhow::Error + '_ {
    move |err| match err {
        ureq::Error::Status(_, response) => anyhow!("{prefix}: {}", read_body_text(response)),
        other => anyhow!("{prefix}: {other}"),
    }
}

fn require_api_key(link: &Link) -> Result<&str> {
    if link.api_key.is_empty() {
        bail!("linked server is missing an API key: relink with -k \"<api-key>\"");
    }
    Ok(&link.api_key)
}

fn authorize_request(
    request: &tiny_http::Request,
    expected_api_key: &str,
) -> std::result::Result<(), HttpFailure> {
    let provided = request
        .headers()
        .iter()
        .find(|header| header.field.equiv(API_KEY_HEADER))
        .map(|header| header.value.as_str());

    match provided {
        Some(value) if value == expected_api_key => Ok(()),
        _ => Err(http_failure(401, "unauthorized: missing or invalid API key")),
    }
}

#[cfg(test)]
mod tests {
    use super::{encode_directory_base64, extract_directory_base64, normalize_base_url};
    use std::fs;

    #[test]
    fn normalizes_base_url() {
        assert_eq!(
            normalize_base_url("http://server", 9771).unwrap(),
            "http://server:9771"
        );
    }

    #[test]
    fn round_trips_directory_archives() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        fs::create_dir_all(src.path().join("nested")).unwrap();
        fs::write(src.path().join("nested/file.txt"), "hello").unwrap();

        let archive = encode_directory_base64(src.path().to_str().unwrap()).unwrap();
        extract_directory_base64(&archive, dst.path()).unwrap();

        assert_eq!(
            fs::read_to_string(dst.path().join("nested/file.txt")).unwrap(),
            "hello"
        );
    }
}
