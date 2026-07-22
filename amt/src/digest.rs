use anyhow::{Context, Result, anyhow};
use md5::{Digest as Md5Digest, Md5};
use rand::{Rng, distributions::Alphanumeric};
use reqwest::blocking::{Client, RequestBuilder, Response};
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue, WWW_AUTHENTICATE};
use reqwest::{Method, StatusCode, Url};
use sha2::Sha256;
use std::collections::BTreeMap;

#[derive(Debug)]
pub struct DigestClient {
    client: Client,
    username: String,
    password: String,
}

impl DigestClient {
    pub fn new(
        username: impl Into<String>,
        password: impl Into<String>,
        accept_invalid_certs: bool,
    ) -> Result<Self> {
        let client = Client::builder()
            .danger_accept_invalid_certs(accept_invalid_certs)
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .context("failed to build HTTP client")?;
        Ok(Self {
            client,
            username: username.into(),
            password: password.into(),
        })
    }

    pub fn post(&self, url: &Url, body: String, headers: HeaderMap) -> Result<Response> {
        let first = self
            .request(Method::POST, url, body.clone(), headers.clone(), None)?
            .send()
            .context("failed to send AMT request")?;
        if first.status() != StatusCode::UNAUTHORIZED {
            return Ok(first);
        }

        let challenge = first
            .headers()
            .get(WWW_AUTHENTICATE)
            .and_then(|value| value.to_str().ok())
            .ok_or_else(|| anyhow!("AMT returned 401 without WWW-Authenticate"))?;
        let auth = self.authorization(Method::POST.as_str(), url, challenge)?;
        self.request(Method::POST, url, body, headers, Some(auth))?
            .send()
            .context("failed to send authenticated AMT request")
    }

    fn request(
        &self,
        method: Method,
        url: &Url,
        body: String,
        headers: HeaderMap,
        authorization: Option<String>,
    ) -> Result<RequestBuilder> {
        let mut req = self
            .client
            .request(method, url.clone())
            .headers(headers)
            .body(body);
        if let Some(value) = authorization {
            req = req.header(AUTHORIZATION, HeaderValue::from_str(&value)?);
        }
        Ok(req)
    }

    fn authorization(&self, method: &str, url: &Url, challenge: &str) -> Result<String> {
        let fields = parse_challenge(challenge)?;
        let realm = required(&fields, "realm")?;
        let nonce = required(&fields, "nonce")?;
        let qop = fields
            .get("qop")
            .and_then(|value| value.split(',').map(str::trim).find(|item| *item == "auth"))
            .unwrap_or("auth");
        let algorithm = fields
            .get("algorithm")
            .map(|value| value.to_ascii_uppercase())
            .unwrap_or_else(|| "MD5".to_string());
        let uri = match url.query() {
            Some(query) => format!("{}?{query}", url.path()),
            None => url.path().to_string(),
        };
        let cnonce: String = rand::thread_rng()
            .sample_iter(&Alphanumeric)
            .take(16)
            .map(char::from)
            .collect();
        let nc = "00000001";
        let ha1 = digest_hex(
            &algorithm,
            &format!("{}:{}:{}", self.username, realm, self.password),
        )?;
        let ha2 = digest_hex(&algorithm, &format!("{method}:{uri}"))?;
        let response = digest_hex(
            &algorithm,
            &format!("{ha1}:{nonce}:{nc}:{cnonce}:{qop}:{ha2}"),
        )?;
        let opaque = fields
            .get("opaque")
            .map(|value| format!(r#", opaque="{value}""#))
            .unwrap_or_default();

        Ok(format!(
            r#"Digest username="{}", realm="{realm}", nonce="{nonce}", uri="{uri}", algorithm={algorithm}, response="{response}", qop={qop}, nc={nc}, cnonce="{cnonce}"{opaque}"#,
            self.username
        ))
    }
}

fn parse_challenge(challenge: &str) -> Result<BTreeMap<String, String>> {
    let challenge = challenge
        .strip_prefix("Digest ")
        .ok_or_else(|| anyhow!("unsupported WWW-Authenticate challenge"))?;
    let mut fields = BTreeMap::new();
    for part in split_quoted(challenge) {
        let (key, value) = part
            .split_once('=')
            .ok_or_else(|| anyhow!("malformed digest challenge field: {part}"))?;
        fields.insert(key.trim().to_ascii_lowercase(), unquote(value.trim()));
    }
    Ok(fields)
}

fn split_quoted(input: &str) -> Vec<&str> {
    let mut fields = Vec::new();
    let mut start = 0;
    let mut quoted = false;
    for (index, byte) in input.bytes().enumerate() {
        match byte {
            b'"' => quoted = !quoted,
            b',' if !quoted => {
                fields.push(input[start..index].trim());
                start = index + 1;
            }
            _ => {}
        }
    }
    fields.push(input[start..].trim());
    fields
}

fn unquote(value: &str) -> String {
    value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .unwrap_or(value)
        .to_string()
}

fn required<'a>(fields: &'a BTreeMap<String, String>, key: &str) -> Result<&'a str> {
    fields
        .get(key)
        .map(String::as_str)
        .ok_or_else(|| anyhow!("digest challenge missing {key}"))
}

fn digest_hex(algorithm: &str, input: &str) -> Result<String> {
    match algorithm {
        "MD5" => Ok(format!("{:x}", Md5::digest(input.as_bytes()))),
        "SHA-256" | "SHA256" => Ok(format!("{:x}", Sha256::digest(input.as_bytes()))),
        _ => Err(anyhow!("unsupported digest algorithm {algorithm}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::CONTENT_TYPE;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::mpsc;
    use std::thread;

    #[derive(Debug)]
    struct CapturedRequest {
        headers: Vec<String>,
        body: String,
    }

    #[test]
    fn parses_digest_challenge() {
        let parsed = parse_challenge(
            r#"Digest realm="Digest:ABC", nonce="xyz", qop="auth,auth-int", algorithm=MD5"#,
        )
        .unwrap();
        assert_eq!(parsed.get("realm").map(String::as_str), Some("Digest:ABC"));
        assert_eq!(parsed.get("qop").map(String::as_str), Some("auth,auth-int"));
    }

    #[test]
    fn retries_post_after_digest_challenge() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = mpsc::channel();
        let server = thread::spawn(move || {
            let (first_stream, _) = listener.accept().unwrap();
            let (first, mut first_stream) = read_request(first_stream);
            respond_unauthorized(&mut first_stream);

            let (second_stream, _) = listener.accept().unwrap();
            let (second, mut second_stream) = read_request(second_stream);
            respond_ok(&mut second_stream);
            tx.send((first, second)).unwrap();
        });

        let client = DigestClient::new("admin", "secret", false).unwrap();
        let url = Url::parse(&format!("http://{addr}/wsman")).unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("application/soap+xml;charset=UTF-8"),
        );
        let response = client
            .post(&url, "<s:Envelope/>".to_string(), headers)
            .unwrap();
        assert!(response.status().is_success());
        assert_eq!(response.text().unwrap(), "<ok/>");

        let (first, second) = rx.recv().unwrap();
        server.join().unwrap();
        assert!(
            !first
                .headers
                .iter()
                .any(|line| line.starts_with("authorization:")),
            "first request must not pre-send credentials"
        );
        let authorization = second
            .headers
            .iter()
            .find(|line| line.starts_with("authorization:"))
            .expect("second request should include Authorization header");
        assert!(authorization.contains("digest username=\"admin\""));
        assert!(authorization.contains("realm=\"amt\""));
        assert!(authorization.contains("uri=\"/wsman\""));
        assert!(authorization.contains("qop=auth"));
        assert_eq!(second.body, "<s:Envelope/>");
    }

    fn read_request(stream: TcpStream) -> (CapturedRequest, TcpStream) {
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut headers = Vec::new();
        let mut content_length = 0usize;
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let line = line.trim_end_matches(['\r', '\n']).to_string();
            if line.is_empty() {
                break;
            }
            let lower = line.to_ascii_lowercase();
            if let Some(value) = lower.strip_prefix("content-length: ") {
                content_length = value.parse().unwrap();
            }
            headers.push(lower);
        }

        let mut body = vec![0; content_length];
        reader.read_exact(&mut body).unwrap();
        (
            CapturedRequest {
                headers,
                body: String::from_utf8(body).unwrap(),
            },
            stream,
        )
    }

    fn respond_unauthorized(stream: &mut TcpStream) {
        let response = concat!(
            "HTTP/1.1 401 Unauthorized\r\n",
            "WWW-Authenticate: Digest realm=\"amt\", nonce=\"abcdef\", qop=\"auth\", algorithm=MD5\r\n",
            "Content-Length: 0\r\n",
            "Connection: close\r\n",
            "\r\n"
        );
        stream.write_all(response.as_bytes()).unwrap();
    }

    fn respond_ok(stream: &mut TcpStream) {
        let response = concat!(
            "HTTP/1.1 200 OK\r\n",
            "Content-Type: application/soap+xml\r\n",
            "Content-Length: 5\r\n",
            "Connection: close\r\n",
            "\r\n",
            "<ok/>"
        );
        stream.write_all(response.as_bytes()).unwrap();
    }
}
