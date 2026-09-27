use super::*;
use serde_json::json;

#[test]
fn mirror_replacement_preserves_package_paths_and_repository_boundaries() {
    let settings: Settings = serde_json::from_value(json!({
        "debian_mirror": "https://mirror.example/custom/debian/",
        "security_mirror": "https://security.example/security/"
    }))
    .unwrap();
    let settings = settings.resolve_with(|_| Ok(None)).unwrap();
    assert_eq!(settings.proxy, Proxy::System);
    for (url, expected) in [
        (
            "https://deb.debian.org/debian/pool/a.deb?version=1",
            "https://mirror.example/custom/debian/pool/a.deb?version=1",
        ),
        (
            "https://deb.debian.org/debian-security/pool/a.deb",
            "https://security.example/security/pool/a.deb",
        ),
        (
            "https://deb.debian.org/debian-extra/a.deb",
            "https://deb.debian.org/debian-extra/a.deb",
        ),
        (
            "https://deb.debian.org.evil.example/debian/a.deb",
            "https://deb.debian.org.evil.example/debian/a.deb",
        ),
    ] {
        assert_eq!(settings.url(url).unwrap(), expected);
    }
}

#[test]
fn environment_overrides_manifest_and_empty_environment_uses_manifest() {
    let settings: Settings = serde_json::from_value(json!({
        "debian_mirror": "https://manifest.example/debian",
        "security_mirror": "https://manifest.example/security",
        "proxy": "direct"
    }))
    .unwrap();
    let resolved = settings
        .resolve_with(|key| {
            Ok(Some(
                match key {
                    "KINAKAZE_DEBIAN_MIRROR" => "https://env.example/debian",
                    "KINAKAZE_DEBIAN_SECURITY_MIRROR" => "https://env.example/security",
                    "KINAKAZE_DOWNLOAD_PROXY" => "http://127.0.0.1:7890",
                    _ => panic!("unexpected environment lookup: {key}"),
                }
                .into(),
            ))
        })
        .unwrap();
    assert_eq!(resolved.debian_mirror, "https://env.example/debian");
    assert_eq!(resolved.security_mirror, "https://env.example/security");
    assert_eq!(resolved.proxy, Proxy::Http("127.0.0.1:7890".into()));
    let resolved = settings.resolve_with(|_| Ok(Some(String::new()))).unwrap();
    assert_eq!(resolved.debian_mirror, "https://manifest.example/debian");
    assert_eq!(resolved.proxy, Proxy::Direct);
    let resolved = settings
        .resolve_with(|key| Ok((key == "KINAKAZE_DOWNLOAD_PROXY").then(|| "system".into())))
        .unwrap();
    assert_eq!(resolved.proxy, Proxy::System);
}

#[test]
fn invalid_settings_are_rejected_instead_of_falling_back_to_system_proxy() {
    for proxy in [
        "",
        "auto",
        "socks5://localhost:1080",
        "https://localhost:8080",
        "http://user:secret@localhost:8080",
        "http://localhost:8080/path",
        "http://localhost:8080?query",
        "http://localhost:8080#fragment",
    ] {
        assert!(Proxy::parse(proxy).is_err(), "accepted {proxy}");
    }
    for url in [
        "http://mirror.example/debian",
        "https:///",
        "https://mirror.example/debian?query=1",
        "https://user:secret@mirror.example/debian",
        "https://mirror.example/debian#fragment",
    ] {
        assert!(mirror(url.into()).is_err(), "accepted {url}");
    }
    assert_eq!(
        Proxy::parse("http://[::1]:7890/").unwrap(),
        Proxy::Http("[::1]:7890".into())
    );
    assert_eq!(
        Settings::default()
            .resolve_with(|_| Ok(None))
            .unwrap()
            .proxy,
        Proxy::System
    );
}
