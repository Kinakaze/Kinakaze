use crate::Result;
use serde::Deserialize;

const DEBIAN: &str = "https://deb.debian.org/debian";
const SECURITY: &str = "https://deb.debian.org/debian-security";

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Settings {
    debian_mirror: Option<String>,
    security_mirror: Option<String>,
    proxy: Option<String>,
}

#[derive(Debug, PartialEq)]
pub(crate) enum Proxy {
    System,
    Direct,
    // HTTP forward proxy authority, without a scheme or credentials.
    Http(String),
}

pub(crate) struct Resolved {
    debian_mirror: String,
    security_mirror: String,
    pub(crate) proxy: Proxy,
}

pub(crate) fn validate_url(value: &str) -> Result<http::Uri> {
    let uri: http::Uri = value.parse()?;
    if uri.host().is_none()
        || uri.authority().is_some_and(|a| a.as_str().contains('@'))
        || value.contains('#')
        || !(uri.scheme_str() == Some("https")
            || (uri.scheme_str() == Some("http") && uri.host() == Some("127.0.0.1")))
    {
        return Err("Debian download URLs require HTTPS (HTTP is allowed for 127.0.0.1)".into());
    }
    Ok(uri)
}

fn mirror(value: String) -> Result<String> {
    if validate_url(&value)?.query().is_some() {
        return Err("Debian mirror URL must not contain a query".into());
    }
    Ok(value.trim_end_matches('/').to_owned())
}

impl Proxy {
    fn parse(value: &str) -> Result<Self> {
        match value {
            "system" => return Ok(Self::System),
            "direct" => return Ok(Self::Direct),
            _ => {}
        }
        // WinHTTP accepts an HTTP proxy for both HTTP requests and HTTPS CONNECT.
        // Reject unsupported schemes/authentication instead of silently bypassing it.
        let uri: http::Uri = value.parse().map_err(|_| "invalid download proxy URL")?;
        if uri.scheme_str() != Some("http")
            || uri.host().is_none()
            || uri.authority().is_some_and(|a| a.as_str().contains('@'))
            || uri.query().is_some()
            || uri.path() != "/"
            || value.contains('#')
        {
            return Err(
                "download proxy must be system, direct or http://host:port (no credentials)".into(),
            );
        }
        Ok(Self::Http(uri.authority().unwrap().to_string()))
    }
}

impl Settings {
    pub(crate) fn resolve(&self) -> Result<Resolved> {
        self.resolve_with(|key| match std::env::var(key) {
            Ok(value) => Ok(Some(value)),
            Err(std::env::VarError::NotPresent) => Ok(None),
            Err(_) => Err(format!("{key} must contain valid Unicode").into()),
        })
    }

    fn resolve_with(&self, env: impl Fn(&str) -> Result<Option<String>>) -> Result<Resolved> {
        let setting = |key, configured: &Option<String>, default: &str| -> Result<String> {
            Ok(env(key)?
                .filter(|v| !v.is_empty())
                .or_else(|| configured.clone())
                .unwrap_or_else(|| default.to_owned()))
        };
        Ok(Resolved {
            debian_mirror: mirror(setting(
                "KINAKAZE_DEBIAN_MIRROR",
                &self.debian_mirror,
                DEBIAN,
            )?)?,
            security_mirror: mirror(setting(
                "KINAKAZE_DEBIAN_SECURITY_MIRROR",
                &self.security_mirror,
                SECURITY,
            )?)?,
            proxy: Proxy::parse(&setting("KINAKAZE_DOWNLOAD_PROXY", &self.proxy, "system")?)?,
        })
    }
}

impl Resolved {
    pub(crate) fn url(&self, original: &str) -> Result<String> {
        for (source, target) in [
            (SECURITY, &self.security_mirror),
            (DEBIAN, &self.debian_mirror),
        ] {
            if let Some(suffix) = original.strip_prefix(source)
                && suffix.starts_with('/')
            {
                let url = format!("{target}{suffix}");
                validate_url(&url)?;
                return Ok(url);
            }
        }
        validate_url(original)?;
        Ok(original.to_owned())
    }
}

#[cfg(test)]
mod tests;
