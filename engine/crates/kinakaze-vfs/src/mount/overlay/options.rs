//! Legacy mount(2) option parsing. Escaped commas/colons belong to pathnames;
//! unsupported features and duplicate layer definitions never disappear.
use super::features::*;
use crate::{EINVAL, EOPNOTSUPP};
pub struct Options {
    pub lowerdirs: Vec<String>,
    pub upperdir: Option<String>,
    pub workdir: Option<String>,
    pub flags: u64,
}

fn split(value: &str, delimiter: char) -> Result<Vec<String>, i32> {
    let mut result = vec![String::new()];
    let mut escaped = false;
    for ch in value.chars() {
        if escaped {
            result.last_mut().unwrap().push(ch);
            escaped = false;
        } else if ch == '\\' {
            result.last_mut().unwrap().push(ch);
            escaped = true;
        } else if ch == delimiter {
            result.push(String::new());
        } else {
            result.last_mut().unwrap().push(ch);
        }
    }
    if escaped {
        return Err(EINVAL);
    }
    Ok(result)
}
fn unescape(value: &str) -> Result<String, i32> {
    let mut result = String::new();
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        result.push(if ch == '\\' {
            chars.next().ok_or(EINVAL)?
        } else {
            ch
        });
    }
    if result.is_empty() || result.contains('\0') {
        return Err(EINVAL);
    }
    Ok(result)
}

fn layers(value: &str) -> Result<(Vec<String>, usize), i32> {
    let parts = split(value, ':')?;
    let mut dirs = Vec::new();
    let mut data = 0;
    let mut cursor = 0;
    while cursor < parts.len() {
        if parts[cursor].is_empty() {
            if dirs.is_empty() {
                return Err(EINVAL);
            }
            cursor += 1;
            let part = parts.get(cursor).ok_or(EINVAL)?;
            dirs.push(unescape(part)?);
            data += 1;
        } else {
            if data != 0 {
                return Err(EINVAL);
            }
            dirs.push(unescape(&parts[cursor])?);
        }
        cursor += 1;
    }
    if dirs.is_empty() || dirs.len() > 500 {
        return Err(EINVAL);
    }
    Ok((dirs, data))
}
pub fn parse_options(data: &str, flags: u64) -> Result<Options, i32> {
    let mut result = Options {
        lowerdirs: Vec::new(),
        upperdir: None,
        workdir: None,
        flags,
    };
    let mut seen = std::collections::HashSet::new();
    for part in split(data, ',')? {
        let key = part.split('=').next().ok_or(EINVAL)?;
        if !seen.insert(key.to_owned()) {
            return Err(EINVAL);
        }
        if let Some(value) = part.strip_prefix("lowerdir=") {
            let (dirs, count) = layers(value)?;
            result.lowerdirs = dirs;
            result.flags = (result.flags & !DATA_COUNT_MASK) | ((count as u64) << 54);
        } else if let Some(value) = part.strip_prefix("upperdir=") {
            result.upperdir = Some(unescape(value)?);
        } else if let Some(value) = part.strip_prefix("workdir=") {
            result.workdir = Some(unescape(value)?);
        } else if part == "userxattr" {
            result.flags |= super::USER_XATTR_FLAG;
        } else if part == "metacopy=on" {
            result.flags |= METACOPY;
        } else if part == "redirect_dir=on" {
            result.flags |= REDIRECT | FOLLOW;
        } else if part == "redirect_dir=follow" {
            result.flags |= FOLLOW;
        } else if part == "index=on" {
            result.flags |= INDEX;
        } else if part == "xino=on" {
            result.flags |= XINO;
        } else if part == "xino=auto" {
            result.flags |= XINO_AUTO;
        } else if part == "uuid=on" {
            result.flags |= UUID_ON;
        } else if part == "uuid=null" {
            result.flags |= UUID_NULL;
        } else if part == "uuid=off" {
            result.flags |= UUID_OFF;
        } else if part == "uuid=auto" {
        } else if part == "nfs_export=on" {
            result.flags |= NFS_EXPORT;
        } else if part == "verity=on" {
            result.flags |= VERITY;
        } else if part == "verity=require" {
            result.flags |= VERITY | VERITY_REQUIRE;
        } else if part == "verity=off" {
        } else if part == "fsync=strict" {
            result.flags |= FSYNC_STRICT;
        } else if part == "fsync=auto" {
        } else if part == "volatile" || part == "fsync=volatile" {
            result.flags |= VOLATILE;
        } else if matches!(
            part.as_str(),
            "index=off"
                | "metacopy=off"
                | "redirect_dir=off"
                | "redirect_dir=nofollow"
                | "xino=off"
                | "nfs_export=off"
        ) {
        } else {
            return Err(if part.contains('=') {
                EOPNOTSUPP
            } else {
                EINVAL
            });
        }
    }
    if result.lowerdirs.is_empty() || result.upperdir.is_some() != result.workdir.is_some() {
        return Err(EINVAL);
    }
    if result.flags & VERITY != 0 {
        if seen.contains("metacopy") && result.flags & METACOPY == 0 {
            return Err(EINVAL);
        }
        result.flags |= METACOPY;
    }
    if result.flags & METACOPY != 0 {
        if seen.contains("redirect_dir")
            && result.flags & REDIRECT == 0
            && !(result.upperdir.is_none() && result.flags & FOLLOW != 0)
        {
            return Err(EINVAL);
        }
        result.flags |= REDIRECT | FOLLOW;
    }
    if result.flags & super::USER_XATTR_FLAG != 0 && result.flags & (METACOPY | FOLLOW) != 0 {
        return Err(EINVAL);
    }
    if result.flags & NFS_EXPORT != 0 {
        if result.flags & METACOPY != 0 {
            return Err(EINVAL);
        }
        if result.upperdir.is_some() {
            if seen.contains("index") && result.flags & INDEX == 0 {
                return Err(EINVAL);
            }
            result.flags |= INDEX;
        } else if result.flags & FOLLOW != 0 {
            return Err(EINVAL);
        }
    }
    result.flags = validate(result.flags, result.upperdir.is_some())?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn escaped_layer_names_and_invalid_features() {
        let options = parse_options(
            r"lowerdir=/a\:b:/c\,d,upperdir=/u\,p,workdir=/w,userxattr,index=off",
            1,
        )
        .unwrap();
        assert_eq!(options.lowerdirs, ["/a:b", "/c,d"]);
        assert_eq!(options.upperdir.as_deref(), Some("/u,p"));
        assert_eq!(options.flags, 1 | super::super::USER_XATTR_FLAG);
        for value in [
            "",
            "lowerdir=/a:",
            "lowerdir=/a,lowerdir=/b",
            "lowerdir=/a,upperdir=/u",
            "lowerdir=/a,workdir=/w",
            "lowerdir=/a\\",
        ] {
            assert!(parse_options(value, 0).is_err(), "{value}");
        }
        assert_eq!(
            parse_options("lowerdir=/a,index=on", 0).unwrap().flags & INDEX,
            0
        );
    }
    #[test]
    fn feature_dependencies_and_conflicts_are_explicit() {
        let base = "lowerdir=/l,upperdir=/u,workdir=/w";
        for (option, bit) in [
            ("index=on", INDEX),
            ("metacopy=on", METACOPY),
            ("redirect_dir=on", REDIRECT),
            ("redirect_dir=follow", FOLLOW),
            ("xino=on", XINO),
            ("xino=auto", XINO_AUTO),
            ("nfs_export=on", NFS_EXPORT),
            ("uuid=on", UUID_ON),
            ("uuid=off", UUID_OFF),
            ("uuid=null", UUID_NULL),
            ("verity=require", VERITY_REQUIRE),
        ] {
            assert_ne!(
                parse_options(&format!("{base},{option}"), 0).unwrap().flags & bit,
                0,
                "{option}"
            );
        }
        for options in [
            "metacopy=on,redirect_dir=nofollow",
            "metacopy=on,nfs_export=on",
            "index=off,nfs_export=on",
            "userxattr,metacopy=on",
            "userxattr,redirect_dir=on",
            "verity=on,metacopy=off",
        ] {
            assert!(
                matches!(parse_options(&format!("{base},{options}"), 0), Err(EINVAL)),
                "{options}"
            );
        }
    }
    #[test]
    fn data_layers_are_last_and_double_colon_delimited() {
        let options = parse_options("lowerdir=/l1:/l2::/data1::/data2,userxattr", 0).unwrap();
        assert_eq!(options.lowerdirs, ["/l1", "/l2", "/data1", "/data2"]);
        assert_eq!(data_count(options.flags), 2);
        assert_eq!(options.flags & METACOPY, 0);
        for options in [
            "lowerdir=::/d",
            "lowerdir=/l:::/d",
            "lowerdir=/l::/d:/l2",
            "lowerdir=/l::",
            "lowerdir=/l::/d,nfs_export=on",
        ] {
            assert!(
                matches!(parse_options(options, 0), Err(EINVAL)),
                "{options}"
            );
        }
    }
}
