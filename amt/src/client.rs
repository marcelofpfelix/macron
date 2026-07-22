use crate::digest::DigestClient;
use crate::wsman::{
    self, BootDevice, CIM_ASSOCIATED_POWER_MANAGEMENT_SERVICE, CIM_COMPUTER_SYSTEM_PACKAGE,
    CIM_POWER_MANAGEMENT_SERVICE, PowerState,
};
use anyhow::{Context, Result, anyhow};
use reqwest::Url;
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderValue, SERVER};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Protocol {
    Http,
    Https,
}

impl Protocol {
    pub fn port(self) -> u16 {
        match self {
            Self::Http => 16992,
            Self::Https => 16993,
        }
    }

    pub fn scheme(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Https => "https",
        }
    }
}

impl std::str::FromStr for Protocol {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "http" => Ok(Self::Http),
            "https" => Ok(Self::Https),
            _ => Err(anyhow!("protocol must be http or https")),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ClientOptions {
    pub host: String,
    pub username: String,
    pub password: String,
    pub protocol: Protocol,
    pub accept_invalid_certs: bool,
}

#[derive(Debug)]
pub struct AmtClient {
    url: Url,
    to: String,
    http: DigestClient,
}

impl AmtClient {
    pub fn new(options: ClientOptions) -> Result<Self> {
        let url = endpoint_url(&options.host, options.protocol)
            .with_context(|| format!("invalid AMT host {}", options.host))?;
        let to = url.path().to_string();
        let http = DigestClient::new(
            options.username,
            options.password,
            options.accept_invalid_certs,
        )?;
        Ok(Self { url, to, http })
    }

    pub fn power(&self, state: PowerState) -> Result<()> {
        let body = wsman::power_state_request(&self.to, state);
        self.post_expect_return(body, CIM_POWER_MANAGEMENT_SERVICE)
    }

    pub fn set_next_boot(&self, device: BootDevice) -> Result<()> {
        self.post_raw(wsman::change_boot_order_request(&self.to, device))?;
        self.post_raw(wsman::enable_boot_config_request(&self.to))?;
        Ok(())
    }

    pub fn pxe_boot(&self) -> Result<()> {
        self.set_next_boot(BootDevice::Pxe)?;
        self.power(PowerState::Reboot)
    }

    pub fn power_status(&self) -> Result<String> {
        let xml = self.post_raw(wsman::get_request(
            &self.to,
            CIM_ASSOCIATED_POWER_MANAGEMENT_SERVICE,
        ))?;
        let value = wsman::extract_text(&xml, "PowerState")
            .ok_or_else(|| anyhow!("AMT response did not include PowerState"))?;
        let code: u8 = value.parse().context("invalid AMT PowerState value")?;
        Ok(PowerState::friendly(code).to_string())
    }

    pub fn uuid(&self) -> Result<String> {
        let xml = self.post_raw(wsman::get_request(&self.to, CIM_COMPUTER_SYSTEM_PACKAGE))?;
        wsman::extract_text(&xml, "PlatformGUID")
            .ok_or_else(|| anyhow!("AMT response did not include PlatformGUID"))
    }

    pub fn version(&self) -> Result<Option<String>> {
        let response =
            self.post_response(wsman::get_request(&self.to, CIM_COMPUTER_SYSTEM_PACKAGE))?;
        Ok(response
            .headers()
            .get(SERVER)
            .and_then(|value| value.to_str().ok())
            .map(ToOwned::to_owned))
    }

    fn post_expect_return(&self, body: String, resource: &str) -> Result<()> {
        let xml = self.post_raw(body)?;
        match wsman::return_value(&xml) {
            Some(0) => Ok(()),
            Some(code) => Err(anyhow!("{resource} returned AMT error code {code}")),
            None => Ok(()),
        }
    }

    fn post_raw(&self, body: String) -> Result<String> {
        self.post_response(body)?
            .text()
            .context("failed to read AMT response")
    }

    fn post_response(&self, body: String) -> Result<reqwest::blocking::Response> {
        let response = self.http.post(&self.url, body, soap_headers())?;
        let status = response.status();
        if !status.is_success() {
            return Err(anyhow!("AMT request failed with HTTP {status}"));
        }
        Ok(response)
    }
}

fn endpoint_url(host: &str, protocol: Protocol) -> Result<Url> {
    if host.starts_with("http://") || host.starts_with("https://") {
        let mut url = Url::parse(host)?;
        if url.path().is_empty() || url.path() == "/" {
            url.set_path("/wsman");
        }
        return Ok(url);
    }

    Url::parse(&format!(
        "{}://{}:{}/wsman",
        protocol.scheme(),
        host,
        protocol.port()
    ))
    .map_err(Into::into)
}

fn soap_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/soap+xml;charset=UTF-8"),
    );
    headers
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(protocol: Protocol) -> ClientOptions {
        ClientOptions {
            host: "192.0.2.10".to_string(),
            username: "admin".to_string(),
            password: "secret".to_string(),
            protocol,
            accept_invalid_certs: true,
        }
    }

    #[test]
    fn constructs_http_endpoint_with_wsman_target() {
        let client = AmtClient::new(options(Protocol::Http)).unwrap();
        assert_eq!(client.url.as_str(), "http://192.0.2.10:16992/wsman");
        assert_eq!(client.to, "/wsman");
    }

    #[test]
    fn accepts_full_amt_url_and_normalizes_root_path() {
        let mut options = options(Protocol::Https);
        options.host = "http://nuc-amt.internal.bandonga.com:16992/".to_string();
        let client = AmtClient::new(options).unwrap();
        assert_eq!(
            client.url.as_str(),
            "http://nuc-amt.internal.bandonga.com:16992/wsman"
        );
        assert_eq!(client.to, "/wsman");
    }

    #[test]
    fn constructs_https_endpoint_with_wsman_target() {
        let client = AmtClient::new(options(Protocol::Https)).unwrap();
        assert_eq!(client.url.as_str(), "https://192.0.2.10:16993/wsman");
        assert_eq!(client.to, "/wsman");
    }

    #[test]
    fn parses_protocol_names() {
        assert_eq!("http".parse::<Protocol>().unwrap(), Protocol::Http);
        assert_eq!("https".parse::<Protocol>().unwrap(), Protocol::Https);
        assert!("ftp".parse::<Protocol>().is_err());
    }
}
