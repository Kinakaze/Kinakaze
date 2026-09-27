//! Host HTTP transport. Windows resolves the current user's system proxy itself.
mod settings;
pub(crate) use settings::{Proxy, Settings, validate_url};
#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub(super) use windows::Client;

#[cfg(not(windows))]
pub(super) struct Client(ureq::Agent);

#[cfg(not(windows))]
impl Client {
    pub(super) fn new(proxy: &Proxy) -> super::Result<Self> {
        let config =
            ureq::Agent::config_builder().timeout_global(Some(std::time::Duration::from_secs(60)));
        let config = match proxy {
            Proxy::System => config,
            Proxy::Direct => config.proxy(None),
            Proxy::Http(authority) => {
                config.proxy(Some(ureq::Proxy::new(&format!("http://{authority}"))?))
            }
        };
        Ok(Self(config.build().into()))
    }

    pub(super) fn read(
        &self,
        url: &str,
        limit: u64,
        mut progress: impl FnMut(u64),
    ) -> super::Result<Vec<u8>> {
        use std::io::Read;
        let mut response = self
            .0
            .get(url)
            .config()
            .https_only(url.starts_with("https://"))
            .build()
            .call()?;
        let mut bytes = Vec::new();
        let mut reader = response.body_mut().as_reader().take(limit);
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let count = reader.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            bytes.extend_from_slice(&buffer[..count]);
            progress(bytes.len() as u64);
        }
        Ok(bytes)
    }
}
